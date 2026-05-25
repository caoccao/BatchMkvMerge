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

//! `MpegPsReader`. Pure-Rust port of `mkvtoolnix/src/input/r_mpeg_ps.cpp`.
//!
//! - Probe scans the first 32 KiB for a leading pack header or a
//!   system-header + packet start-code pair (PARSER-049).
//! - PES packets are depacketised per stream; Program Stream Map entries are
//!   parsed (PARSER-051) and private-stream-1 substream ids are recorded so
//!   the codec can be resolved later (PARSER-050); the accumulated elementary
//!   payload feeds codec-header decoding (PARSER-052).

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::identify::{self, StreamObservation};
use super::packet::{self, StartCode, PACK_HEADER, SYSTEM_HEADER};
use super::{pes, stream_map};

const PROBE_BYTES: usize = 64 * 1024;
const PROBE_SCAN: usize = 32 * 1024;
const STREAM_PAYLOAD_CAP: usize = 256 * 1024;
const MAX_STREAMS: usize = 64;
const PACKET_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, PACK_HEADER];

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegPsReader;

/// A start-code byte is a PS packet-layer code (`0xB9`..`0xFF`) rather than an
/// elementary-stream code (`0x00`..`0xB8`, e.g. MPEG slice / sequence headers
/// embedded in a video payload).
fn is_packet_layer(sid: u8) -> bool {
    sid >= 0xB9
}

/// Scan forward for the next packet-layer start code, skipping elementary
/// start codes that appear inside a video payload.
fn next_packet_layer(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    loop {
        match packet::find_start_code(bytes, i) {
            Some((pos, sid)) if is_packet_layer(sid) => return pos,
            Some((pos, _)) => i = pos + 4,
            None => return bytes.len(),
        }
    }
}

#[derive(Default)]
struct StreamAcc {
    payload: Vec<u8>,
}

impl Reader for MpegPsReader {
    fn name(&self) -> &'static str {
        "mpeg_ps"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = vec![0u8; PROBE_SCAN];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 4 {
            return Ok(false);
        }
        // Fast path: file begins with a pack header.
        if head[..4] == PACKET_START_CODE {
            return Ok(true);
        }
        // Otherwise require both a system-header and a packet start code within
        // the scan window (mkvtoolnix's fallback).
        let bytes = &head[..read];
        let mut system_header = false;
        let mut packet_start = false;
        let mut i = 0usize;
        while i + 4 <= bytes.len() && (!system_header || !packet_start) {
            if let Some((pos, sid)) = packet::find_start_code(bytes, i) {
                match sid {
                    SYSTEM_HEADER => system_header = true,
                    PACK_HEADER => packet_start = true,
                    _ => {}
                }
                i = pos + 4;
            } else {
                break;
            }
        }
        Ok(system_header && packet_start)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut probe)?;
        if read < 4 {
            return Err(ParseError::Unrecognised);
        }
        let bytes = &probe[..read];

        // Insertion-ordered stream table keyed by (stream_id, sub_id).
        let mut order: Vec<(u8, Option<u8>)> = Vec::new();
        let mut streams: HashMap<(u8, Option<u8>), StreamAcc> = HashMap::new();
        let mut psm_types: HashMap<u8, u8> = HashMap::new();

        let mut offset = 0usize;
        let mut iterations = 0usize;
        while let Some((pos, sid)) = packet::find_start_code(bytes, offset) {
            deadline.check("mpeg_ps::reader")?;
            iterations += 1;
            if iterations > 200_000 {
                break;
            }
            if !is_packet_layer(sid) {
                offset = pos + 4;
                continue;
            }
            match StartCode::from_byte(sid) {
                StartCode::ProgramStreamMap => {
                    if let Ok(psm) = stream_map::parse(&bytes[pos + 4..]) {
                        for e in psm.entries {
                            psm_types
                                .entry(e.elementary_stream_id)
                                .or_insert(e.stream_type);
                        }
                    }
                    offset = pos + 4;
                }
                StartCode::Audio(_) | StartCode::Video(_) | StartCode::PrivateStream1 => {
                    let pkt = &bytes[pos..];
                    let pkt_len = pes::parse(pkt).map(|h| h.packet_length as usize).unwrap_or(0);
                    let payoff = pes::pes_payload_offset(pkt);
                    let payload_abs = (pos + payoff).min(bytes.len());
                    let pkt_end = if pkt_len > 0 {
                        (pos + 6 + pkt_len).min(bytes.len())
                    } else {
                        next_packet_layer(bytes, payload_abs.max(pos + 4))
                    };

                    let sub_id = if sid == 0xBD && payload_abs < bytes.len() {
                        Some(bytes[payload_abs])
                    } else {
                        None
                    };
                    let data_start = if sid == 0xBD {
                        (payload_abs + 1).min(bytes.len())
                    } else {
                        payload_abs
                    };
                    let data_end = pkt_end.min(bytes.len()).max(data_start);

                    let key = (sid, sub_id);
                    let acc = streams.entry(key).or_insert_with(|| {
                        if order.len() < MAX_STREAMS {
                            order.push(key);
                        }
                        StreamAcc::default()
                    });
                    if acc.payload.len() < STREAM_PAYLOAD_CAP {
                        let take = (STREAM_PAYLOAD_CAP - acc.payload.len())
                            .min(data_end.saturating_sub(data_start));
                        acc.payload.extend_from_slice(&bytes[data_start..data_start + take]);
                    }
                    offset = pkt_end.max(pos + 4);
                }
                _ => {
                    // PackHeader / SystemHeader / Padding / PrivateStream2 / ...
                    offset = pos + 4;
                }
            }
        }

        let observations: Vec<StreamObservation> = order
            .into_iter()
            .filter_map(|key| {
                streams.remove(&key).map(|acc| StreamObservation {
                    stream_id: key.0,
                    sub_id: key.1,
                    psm_stream_type: psm_types.get(&key.0).copied(),
                    payload: acc.payload,
                })
            })
            .collect();

        identify::finalise(observations, out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::model::container::ContainerFormat;
    use crate::media_metadata::model::track::TrackType;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn start_code(stream_id: u8) -> [u8; 4] {
        [0x00, 0x00, 0x01, stream_id]
    }

    fn build_ps(stream_ids: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 10]); // pack body
        for id in stream_ids {
            bytes.extend_from_slice(&start_code(*id));
            bytes.extend_from_slice(&8u16.to_be_bytes()); // packet length
            bytes.extend_from_slice(&[0u8; 8]);
        }
        bytes
    }

    #[test]
    fn probe_accepts_files_starting_with_pack_header() {
        let bytes = build_ps(&[0xE0]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(MpegPsReader.probe(&mut s).unwrap());
    }

    // ---- PARSER-049: scan-based probe ------------------------------------

    #[test]
    fn probe_accepts_leading_garbage_then_system_and_pack() {
        let mut bytes = vec![0xAA, 0xBB, 0xCC]; // leading junk
        bytes.extend_from_slice(&start_code(SYSTEM_HEADER));
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 8]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(MpegPsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_files_without_pack_header() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
        assert!(!MpegPsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_collects_unique_stream_ids() {
        // 0xBD packets with all-zero payload have sub-id 0x00 which is *not*
        // a documented private-stream-1 substream — mkvtoolnix drops them, so
        // we expect just the two unique audio/video stream ids (PARSER-095).
        let bytes = build_ps(&[0xE0, 0xC0, 0xE0, 0xC0, 0xBD]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::MpegPs);
        assert_eq!(out.tracks.len(), 2);
        let kinds: Vec<TrackType> = out.tracks.iter().map(|t| t.track_type).collect();
        assert!(kinds.contains(&TrackType::Video));
        assert!(kinds.contains(&TrackType::Audio));
    }

    #[test]
    fn read_headers_returns_unrecognised_on_empty_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(Vec::<u8>::new()));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        let err = MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn padding_stream_is_ignored() {
        let bytes = build_ps(&[0xBE, 0xE0, 0xBE]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].track_type, TrackType::Video);
    }

    // ---- PARSER-050: private-stream-1 substreams -------------------------

    #[test]
    fn private_stream_1_dts_substream_classified() {
        // 0xBD packet whose first payload byte (sub_id) is 0x88 → DTS.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 10]);
        bytes.extend_from_slice(&start_code(0xBD));
        bytes.extend_from_slice(&16u16.to_be_bytes()); // packet length
        // PES header: 2 flag bytes + header_data_length=0, then payload.
        bytes.extend_from_slice(&[0x80, 0x80, 0x00]);
        bytes.push(0x88); // sub_id → DTS
        bytes.extend_from_slice(&[0u8; 12]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.vob", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].codec.id, "A_DTS");
    }

    // ---- PARSER-094: VC-1 stream id 0xFD --------------------------------

    #[test]
    fn vc1_stream_id_fd_is_collected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 10]);
        bytes.extend_from_slice(&start_code(0xFD));
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].codec.id, "V_VC1");
        assert_eq!(out.tracks[0].track_type, TrackType::Video);
    }

    // ---- PARSER-051: Program Stream Map ----------------------------------

    #[test]
    fn program_stream_map_overrides_classification() {
        // PSM mapping stream id 0xE0 → stream_type 0x1B (AVC).
        let mut psm_payload = Vec::new();
        psm_payload.extend_from_slice(&0u16.to_be_bytes()); // map length (unused)
        psm_payload.push(0x80); // current_next + version
        psm_payload.push(0x01); // marker
        psm_payload.extend_from_slice(&0u16.to_be_bytes()); // program_stream_info_length
        psm_payload.extend_from_slice(&4u16.to_be_bytes()); // elementary_stream_map_length
        psm_payload.push(0x1B); // stream_type AVC
        psm_payload.push(0xE0); // elementary_stream_id
        psm_payload.extend_from_slice(&0u16.to_be_bytes()); // es_info_length
        psm_payload.extend_from_slice(&0u32.to_be_bytes()); // CRC

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 10]);
        bytes.extend_from_slice(&start_code(super::super::packet::PROGRAM_STREAM_MAP));
        bytes.extend_from_slice(&psm_payload);
        // A video PES on 0xE0.
        bytes.extend_from_slice(&start_code(0xE0));
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&[0u8; 8]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].codec.id, "V_MPEG4/ISO/AVC");
    }
}
