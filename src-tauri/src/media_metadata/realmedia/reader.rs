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

//! RealMediaReader — walks the top-level chunk hierarchy and populates the
//! MediaMetadata model.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::chunks::{
    ChunkHeader, ContChunk, MdprChunk, PropChunk, COMMON_HEADER_LEN, ID_CONT, ID_DATA, ID_MDPR,
    ID_PROP, RMF_MAGIC,
};
use super::stream_props::{AudioProps, VideoProps};

const PROBE_BYTES: usize = 4;

#[derive(Debug, Default, Clone, Copy)]
pub struct RealMediaReader;

impl Reader for RealMediaReader {
    fn name(&self) -> &'static str {
        "realmedia"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = [0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        Ok(read >= PROBE_BYTES && buf == RMF_MAGIC)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        // Parse the file header (.RMF object header + format_version + num_headers).
        src.seek_to(0)?;
        let mut head = [0u8; COMMON_HEADER_LEN];
        src.read_exact(&mut head)?;
        let header = ChunkHeader::parse(&head).ok_or(ParseError::Unrecognised)?;
        if header.id != RMF_MAGIC {
            return Err(ParseError::Unrecognised);
        }
        // format_version + num_headers (8 bytes); we don't need them but the
        // .RMF body is part of the chunk, so seek past them.
        src.skip(8)?;

        out.container.format = ContainerFormat::RealMedia;
        out.container.recognized = true;
        out.container.supported = true;

        let mut prop: Option<PropChunk> = None;
        let mut tracks: Vec<MdprChunk> = Vec::new();

        // Walk top-level chunks until DATA (or EOF).  Identification only
        // requires header chunks — DATA is intentionally never entered.
        loop {
            deadline.check("realmedia-chunk")?;
            let mut hdr = [0u8; COMMON_HEADER_LEN];
            if src.read_at_most(&mut hdr)? < COMMON_HEADER_LEN {
                break;
            }
            let chunk = match ChunkHeader::parse(&hdr) {
                Some(c) => c,
                None => break,
            };
            if (chunk.size as usize) < COMMON_HEADER_LEN {
                break;
            }
            let payload_len = chunk.size as usize - COMMON_HEADER_LEN;
            let next_pos = src.position() + payload_len as u64;
            if chunk.id == ID_PROP {
                let payload = read_payload(src, payload_len)?;
                prop = PropChunk::parse(&payload);
            } else if chunk.id == ID_CONT {
                let payload = read_payload(src, payload_len)?;
                if let Some(c) = ContChunk::parse(&payload) {
                    apply_content_metadata(out, &c);
                }
            } else if chunk.id == ID_MDPR {
                let payload = read_payload(src, payload_len)?;
                if let Some(m) = MdprChunk::parse(&payload) {
                    tracks.push(m);
                }
            } else if chunk.id == ID_DATA {
                // First chunk of frame data — header walk is complete.
                break;
            }
            src.seek_to(next_pos)?;
        }

        if let Some(p) = &prop {
            out.container.properties.duration =
                Some(DurationValue::from_ns(p.duration_ms as u64 * 1_000_000));
            out.container.properties.bitrate_bps = Some(p.avg_bit_rate as u64);
        }

        for (idx, track) in tracks.iter().enumerate() {
            push_track(out, idx as i64, track);
        }
        Ok(())
    }
}

fn read_payload(src: &mut FileSource, len: usize) -> Result<Vec<u8>, ParseError> {
    let mut buf = vec![0u8; len];
    src.read_exact(&mut buf)?;
    Ok(buf)
}

fn apply_content_metadata(out: &mut MediaMetadata, c: &ContChunk) {
    if !c.title.is_empty() {
        out.container.properties.title = Some(c.title.clone());
    }
    if !c.author.is_empty() {
        out.container.properties.writing_app = Some(c.author.clone());
    }
}

fn fourcc_string(fourcc: &[u8; 4]) -> String {
    String::from_utf8_lossy(fourcc).trim_end_matches('\0').to_string()
}

fn push_track(out: &mut MediaMetadata, id: i64, track: &MdprChunk) {
    let mut common = CommonTrackProperties::default();
    common.number = Some(track.stream_number as u64 + 1);
    if !track.stream_name.is_empty() {
        common.track_name = Some(track.stream_name.clone());
    }

    match track.mime_type.as_str() {
        "video/x-pn-realvideo" => {
            if let Some(v) = VideoProps::parse(&track.type_specific_data) {
                let fourcc = fourcc_string(&v.fourcc);
                let codec_id = format!("V_REAL/{}", fourcc);
                let dims = Some(Dimensions2D {
                    width: v.width as u32,
                    height: v.height as u32,
                });
                let fps = v.fps();
                let default_duration_ns = if fps > 0.0 {
                    Some((1_000_000_000.0 / fps).round() as u64)
                } else {
                    None
                };
                out.tracks.push(Track {
                    id,
                    track_type: TrackType::Video,
                    codec: CodecInfo {
                        id: codec_id,
                        name: Some(real_video_display_name(&fourcc)),
                        codec_private: None,
                    },
                    properties: TrackProperties {
                        common,
                        video: Some(VideoTrackProperties {
                            pixel_dimensions: dims,
                            display_dimensions: dims,
                            default_duration_ns,
                            ..VideoTrackProperties::default()
                        }),
                        ..TrackProperties::default()
                    },
                });
            }
        }
        "audio/x-pn-realaudio" => {
            if let Some(a) = AudioProps::parse(&track.type_specific_data) {
                let fourcc = fourcc_string(&a.fourcc);
                let (codec_id, name) = real_audio_codec_id(&fourcc);
                out.tracks.push(Track {
                    id,
                    track_type: TrackType::Audio,
                    codec: CodecInfo {
                        id: codec_id.to_string(),
                        name: Some(name.to_string()),
                        codec_private: None,
                    },
                    properties: TrackProperties {
                        common,
                        audio: Some(AudioTrackProperties {
                            sampling_frequency: Some(a.sample_rate as f64),
                            channels: Some(a.channels as u32),
                            bit_depth: Some(a.sample_size as u32),
                            ..AudioTrackProperties::default()
                        }),
                        ..TrackProperties::default()
                    },
                });
            }
        }
        _ => {
            // Unknown MIME — surface as an Unknown track so the count stays
            // consistent with the container's `num_streams`.
        }
    }
}

fn real_video_display_name(fourcc: &str) -> String {
    match fourcc {
        "RV10" => "RealVideo 1".to_string(),
        "RV20" => "RealVideo G2 / 2.0".to_string(),
        "RV30" => "RealVideo 8".to_string(),
        "RV40" => "RealVideo 9 / 10".to_string(),
        "RV60" | "RVHD" => "RealVideo HD".to_string(),
        other => format!("RealVideo ({})", other),
    }
}

fn real_audio_codec_id(fourcc: &str) -> (&'static str, &'static str) {
    match fourcc {
        "14_4" => ("A_REAL/14_4", "RealAudio 14.4"),
        "28_8" => ("A_REAL/28_8", "RealAudio 28.8"),
        "dnet" => ("A_AC3", "AC-3 (RealAudio dnet)"),
        "sipr" => ("A_REAL/SIPR", "Sipro Lab Telecom"),
        "cook" => ("A_REAL/COOK", "Cook (RealAudio G2)"),
        "atrc" => ("A_REAL/ATRC", "Sony ATRAC3"),
        "raac" | "racp" => ("A_AAC", "AAC (RealAudio)"),
        "ralf" => ("A_REAL/LF", "RealAudio Lossless"),
        _ => ("A_REAL/UNKNOWN", "RealAudio"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::realmedia::chunks::{build_chunk, ID_DATA};
    use crate::media_metadata::realmedia::stream_props::{
        build_audio_v3, build_audio_v4, build_audio_v5, build_video_props,
    };
    use std::io::Cursor;

    fn build_rmf_header() -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // format_version
        payload.extend_from_slice(&5u32.to_be_bytes()); // num_headers
        build_chunk(RMF_MAGIC, 0, &payload)
    }

    fn build_prop_chunk(duration_ms: u32) -> Vec<u8> {
        let mut payload = Vec::new();
        for _ in 0..5 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        payload.extend_from_slice(&duration_ms.to_be_bytes());
        for _ in 0..3 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        payload.extend_from_slice(&1u16.to_be_bytes()); // num_streams
        payload.extend_from_slice(&0u16.to_be_bytes()); // flags
        build_chunk(ID_PROP, 0, &payload)
    }

    fn build_mdpr(stream_id: u16, mime: &str, type_specific: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&stream_id.to_be_bytes());
        for _ in 0..7 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        payload.push(0); // stream_name_len
        payload.push(mime.len() as u8);
        payload.extend_from_slice(mime.as_bytes());
        payload.extend_from_slice(&(type_specific.len() as u32).to_be_bytes());
        payload.extend_from_slice(type_specific);
        build_chunk(ID_MDPR, 0, &payload)
    }

    fn build_data_chunk() -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // num_packets
        payload.extend_from_slice(&0u32.to_be_bytes()); // next_data_offset
        build_chunk(ID_DATA, 0, &payload)
    }

    #[test]
    fn probe_accepts_rmf_signature() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(build_rmf_header()));
        assert!(RealMediaReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 32]));
        assert!(!RealMediaReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_video_track_metadata() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(120_000));
        let v_props = build_video_props(b"RV40", 1280, 720, 25.0);
        blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.rm", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::RealMedia);
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].track_type, TrackType::Video);
        assert_eq!(out.tracks[0].codec.id, "V_REAL/RV40");
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1280, height: 720 }));
        assert_eq!(v.default_duration_ns, Some(40_000_000));
        let dur = out.container.properties.duration.as_ref().unwrap();
        assert_eq!(dur.ns, 120_000 * 1_000_000);
    }

    #[test]
    fn read_headers_extracts_audio_v4_track() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        let a_props = build_audio_v4(44_100, 2, 16, b"cook");
        blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.ra", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].codec.id, "A_REAL/COOK");
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.sampling_frequency, Some(44_100.0));
        assert_eq!(a.channels, Some(2));
    }

    #[test]
    fn read_headers_extracts_audio_v5_track_and_promotes_raac_to_aac() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        let a_props = build_audio_v5(48_000, 6, 16, b"raac");
        blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.ra", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.id, "A_AAC");
    }

    #[test]
    fn read_headers_handles_v3_audio_with_hardcoded_codec() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        let a_props = build_audio_v3();
        blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.ra", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.id, "A_REAL/14_4");
    }

    #[test]
    fn read_headers_records_content_title() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        let mut cont_payload = Vec::new();
        for s in ["My Movie", "Some Author", "©2026", ""] {
            cont_payload.extend_from_slice(&(s.len() as u16).to_be_bytes());
            cont_payload.extend_from_slice(s.as_bytes());
        }
        blob.extend(build_chunk(ID_CONT, 0, &cont_payload));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.rm", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.properties.title.as_deref(), Some("My Movie"));
        assert_eq!(
            out.container.properties.writing_app.as_deref(),
            Some("Some Author")
        );
    }

    #[test]
    fn read_headers_stops_at_data_chunk() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        let v_props = build_video_props(b"RV40", 320, 240, 25.0);
        blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));
        blob.extend(build_data_chunk());
        // Any extra bytes after DATA must not influence identification.
        blob.extend_from_slice(&[0xFFu8; 64]);

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.rm", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
    }

    #[test]
    fn read_headers_ignores_unknown_mime_types() {
        let mut blob = build_rmf_header();
        blob.extend(build_prop_chunk(0));
        blob.extend(build_mdpr(0, "application/octet-stream", &[]));
        blob.extend(build_data_chunk());

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.rm", 0);
        RealMediaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert!(out.tracks.is_empty());
    }
}
