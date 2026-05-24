/*
 *   Copyright (c) 2026. caoccao.com Sam Cao
 *   All rights reserved.

 *   Licensed under the Apache License, Version 2.0 (the "License");
 *   you may not use this file except in compliance with the License.
 *   You may obtain a copy of the License at

 *   http://www.apache.org/licenses/LICENSE-2.0

 *   Unless required by applicable law or agreed to in writing, software
 *   distributed under the License is distributed on an "AS IS" BASIS,
 *   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *   See the License for the specific language governing permissions and
 *   limitations under the License.
 */

//! Top-level `OggReader`.

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::codecs;
use super::comments;
use super::identify::{self, BitstreamState};
use super::page;

/// Cap the number of pages we walk per parse — protects against pathological
/// streams while still leaving plenty of room to collect VorbisComment blocks
/// from every bitstream.
const MAX_PAGES: usize = 2048;
const PAGE_PAYLOAD_CAP: u64 = 256 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct OggReader;

impl Reader for OggReader {
    fn name(&self) -> &'static str {
        "ogg"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 4];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read == 4 && &head == b"OggS")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        src.seek_to(0)?;
        let stream_end = src.length();

        // Map serial → state.  Preserve insertion order via a parallel Vec.
        let mut states: Vec<BitstreamState> = Vec::new();
        let mut serial_to_index: HashMap<u32, usize> = HashMap::new();
        let mut pages_consumed = 0usize;

        loop {
            deadline.check("ogg::reader")?;
            if pages_consumed >= MAX_PAGES {
                break;
            }
            if let Some(end) = stream_end {
                if src.position() >= end {
                    break;
                }
            }
            let pos = src.position();
            let header = match page::read_page_header(src) {
                Ok(h) => h,
                Err(ParseError::UnexpectedEof { .. }) => break,
                Err(ParseError::Malformed { .. }) => break,
                Err(e) => return Err(e),
            };
            pages_consumed += 1;

            let payload = page::read_page_payload(src, &header, PAGE_PAYLOAD_CAP)?;
            handle_page(&header, &payload, &mut states, &mut serial_to_index);

            // Stop once every BOS stream has at least two packets (BOS + comment)
            // and the next page would be a continuation cluster.  This keeps
            // identification fast for huge files.
            if all_streams_have_comments(&states) && pages_consumed > 4 {
                break;
            }

            // Defensive: ensure progress.
            if src.position() <= pos {
                break;
            }
        }

        identify::finalise(states, out);
        Ok(())
    }
}

fn handle_page(
    header: &page::PageHeader,
    payload: &[u8],
    states: &mut Vec<BitstreamState>,
    serial_to_index: &mut HashMap<u32, usize>,
) {
    if header.is_beginning_of_stream() {
        let idx = states.len();
        let mut state = BitstreamState {
            serial: header.bitstream_serial,
            first_packet: Vec::new(),
            metadata: None,
            vorbis_tags: Vec::new(),
            comment_language: None,
            vendor: None,
        };
        // The BOS packet must be wholly contained in this page.
        if let Some(first_span) = header.packet_layout().first() {
            let end = (first_span.bytes as usize).min(payload.len());
            state.first_packet = payload[..end].to_vec();
            state.metadata = codecs::sniff_first_packet(&state.first_packet);
        }
        states.push(state);
        serial_to_index.insert(header.bitstream_serial, idx);
        return;
    }

    // Non-BOS page: look up the bitstream and try to extract a comment block
    // from its second packet (the VorbisComment / OpusTags / Theora comments).
    let Some(&idx) = serial_to_index.get(&header.bitstream_serial) else {
        return;
    };
    let state = &mut states[idx];
    if state.metadata.is_none() {
        return;
    }
    if !state.vorbis_tags.is_empty() {
        return; // already populated
    }
    if let Some(first_span) = header.packet_layout().first() {
        let end = (first_span.bytes as usize).min(payload.len());
        let packet = &payload[..end];
        let codec_id = state
            .metadata
            .as_ref()
            .map(|m| m.codec_id)
            .unwrap_or_default();
        if let Some(comments) = decode_comment_packet(packet, codec_id) {
            state.vendor = Some(comments.vendor);
            state.comment_language = comments::extract_language(&comments.entries);
            state.vorbis_tags = comments.entries;
        }
    }
}

fn decode_comment_packet(packet: &[u8], codec_id: &str) -> Option<comments::VorbisComments> {
    match codec_id {
        "A_VORBIS" => {
            if packet.len() > 7 && packet[0] == 0x03 && &packet[1..7] == b"vorbis" {
                comments::parse(&packet[7..])
            } else {
                None
            }
        }
        "A_OPUS" => {
            if packet.len() > 8 && &packet[..8] == b"OpusTags" {
                comments::parse(&packet[8..])
            } else {
                None
            }
        }
        "V_THEORA" => {
            if packet.len() > 7 && packet[0] == 0x81 && &packet[1..7] == b"theora" {
                comments::parse(&packet[7..])
            } else {
                None
            }
        }
        _ => None,
    }
}

fn all_streams_have_comments(states: &[BitstreamState]) -> bool {
    !states.is_empty()
        && states
            .iter()
            .filter(|s| s.metadata.is_some())
            .all(|s| !s.vorbis_tags.is_empty() || s.vendor.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::model::container::ContainerFormat;
    use crate::media_metadata::model::track::TrackType;
    use crate::media_metadata::ogg::codecs::{opus, theora, vorbis};
    use crate::media_metadata::ogg::comments::build_block;
    use crate::media_metadata::ogg::page::{build_page, HEADER_FLAG_BEGINNING_OF_STREAM};
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn build_vorbis_stream(serial: u32, language: Option<&str>) -> Vec<u8> {
        let bos = vorbis::build_identification_packet(2, 44100);
        let page_bos = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, serial, 0, &[&bos]);

        // VorbisComment packet: 0x03 + "vorbis" + comment block + framing bit (0x01).
        let mut comments_pkt = vec![0x03];
        comments_pkt.extend_from_slice(b"vorbis");
        let tags: Vec<(&str, &str)> = match language {
            Some(l) => vec![("TITLE", "Track"), ("LANGUAGE", l)],
            None => vec![("TITLE", "Track")],
        };
        comments_pkt.extend(build_block("libvorbis 1.3.7", &tags));
        comments_pkt.push(0x01); // framing bit
        let page_comments = build_page(0, 0, serial, 1, &[&comments_pkt]);

        let mut bytes = page_bos;
        bytes.extend(page_comments);
        bytes
    }

    #[test]
    fn probe_accepts_ogg_signature() {
        let bytes = build_vorbis_stream(0xCAFE, None);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(OggReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_other_magic() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
        assert!(!OggReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_vorbis_track_with_comments() {
        let bytes = build_vorbis_stream(0xCAFE, Some("fra"));
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ogg", 0);
        OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Ogg);
        assert_eq!(out.tracks.len(), 1);
        let t = &out.tracks[0];
        assert_eq!(t.track_type, TrackType::Audio);
        assert_eq!(t.codec.id, "A_VORBIS");
        // TITLE + LANGUAGE + VENDOR
        assert_eq!(t.properties.tags.len(), 3);
        let lang = t.properties.common.language.as_ref().unwrap();
        assert_eq!(lang.iso639_2, "fra");
    }

    #[test]
    fn read_headers_handles_opus_stream() {
        let bos = opus::build_identification_packet(2, 48000);
        let mut comments_pkt = b"OpusTags".to_vec();
        comments_pkt.extend(build_block("libopus 1.4", &[("ARTIST", "X")]));
        let page_bos = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
        let page_comments = build_page(0, 0, 1, 1, &[&comments_pkt]);
        let mut bytes = page_bos;
        bytes.extend(page_comments);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.opus", 0);
        OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.id, "A_OPUS");
    }

    #[test]
    fn read_headers_handles_two_independent_streams() {
        let v = build_vorbis_stream(1, None);
        let mut t_bos = vec![0x80];
        t_bos.extend_from_slice(b"theora");
        t_bos.extend(theora::build_identification_packet(640, 480, 24, 1)[7..].iter().copied());
        let theora_full = theora::build_identification_packet(640, 480, 24, 1);
        let theora_page = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 2, 0, &[&theora_full]);
        let mut bytes = v;
        bytes.extend(theora_page);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ogv", 0);
        OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert!(out.tracks.iter().any(|t| t.codec.id == "A_VORBIS"));
        assert!(out.tracks.iter().any(|t| t.codec.id == "V_THEORA"));
    }

    #[test]
    fn malformed_first_page_returns_no_tracks() {
        let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[b"junk"]);
        // Corrupt the magic.
        bytes[0..4].copy_from_slice(b"FAKE");
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ogg", 0);
        OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert!(out.tracks.is_empty());
        // Reader still claims the container as recognised since we got past
        // probe; identify::finalise stamps recognized=true.
        assert_eq!(out.container.format, ContainerFormat::Ogg);
    }

    #[test]
    fn non_bos_page_without_known_serial_is_ignored() {
        // Just two non-BOS pages.
        let p1 = build_page(0, 0, 999, 0, &[b"data"]);
        let p2 = build_page(0, 0, 998, 0, &[b"more"]);
        let mut bytes = p1;
        bytes.extend(p2);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ogg", 0);
        OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert!(out.tracks.is_empty());
    }
}
