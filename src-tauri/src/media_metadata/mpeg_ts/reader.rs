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

//! Top-level `MpegTsReader`.

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::descriptors::DescriptorSummary;
use super::identify;
use super::packet::{
    self, detect_packet_size, PacketHeader, PACKET_SIZE_BD_M2TS, PACKET_SIZE_STANDARD,
};
use super::pat;
use super::pmt;
use super::stream_table::{self, StreamRow};

/// Cap on packets we'll walk before bailing out — protects against very
/// long streams while still leaving plenty of room for PAT + PMT.
const MAX_PACKETS: usize = 8 * 1024;
const PROBE_BYTES: usize = 8 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegTsReader;

impl Reader for MpegTsReader {
    fn name(&self) -> &'static str {
        "mpeg_ts"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 188 {
            return Ok(false);
        }
        Ok(detect_packet_size(&head[..read]).is_some())
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        // 1. Sniff packet size from the first 8 KB.
        let mut probe = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let probe_len = src.read_at_most(&mut probe)?;
        let packet_size = detect_packet_size(&probe[..probe_len]).ok_or(
            ParseError::Unrecognised,
        )?;
        let header_offset = if packet_size == PACKET_SIZE_BD_M2TS { 4 } else { 0 };

        // 2. Walk packets reassembling PSI sections.
        src.seek_to(0)?;
        let mut pmt_pids: HashMap<u16, u16> = HashMap::new(); // pmt_pid → program_number
        let mut rows: Vec<StreamRow> = Vec::new();
        let mut seen_pmt_pids: std::collections::HashSet<u16> = Default::default();

        let mut packet_buf = vec![0u8; packet_size];
        let mut packet_count = 0usize;
        loop {
            deadline.check("mpeg_ts::reader")?;
            if packet_count >= MAX_PACKETS {
                break;
            }
            let read = src.read_at_most(&mut packet_buf)?;
            if read < packet_size {
                break;
            }
            packet_count += 1;
            let pkt = &packet_buf[header_offset..];
            if pkt.len() < 4 || pkt[0] != packet::TS_SYNC_BYTE {
                continue;
            }
            let header = match packet::decode_header(pkt) {
                Ok(h) => h,
                Err(_) => continue,
            };
            if !header.has_payload() || !header.payload_unit_start {
                continue;
            }
            let payload = packet::payload_slice(pkt, &header);
            if payload.is_empty() {
                continue;
            }
            // PSI payloads start with a 1-byte pointer_field; section follows.
            let pointer = payload[0] as usize;
            if 1 + pointer >= payload.len() {
                continue;
            }
            let section = &payload[1 + pointer..];

            if packet::is_pat_pid(header.pid) {
                handle_pat(section, &mut pmt_pids);
            } else if pmt_pids.contains_key(&header.pid) {
                let prog = *pmt_pids.get(&header.pid).unwrap();
                if seen_pmt_pids.insert(header.pid) {
                    handle_pmt(section, prog, &mut rows);
                }
            }
            // Stop early if every known PMT has been processed.
            if !pmt_pids.is_empty() && seen_pmt_pids.len() == pmt_pids.len() {
                break;
            }
        }
        let _ = header_offset; // silence unused warning when packet_size == 188

        identify::finalise(rows, out);
        Ok(())
    }
}

fn handle_pat(section: &[u8], pmt_pids: &mut HashMap<u16, u16>) {
    if let Ok(pat) = pat::parse(section) {
        for entry in pat.entries {
            pmt_pids.insert(entry.pmt_pid, entry.program_number);
        }
    }
}

fn handle_pmt(section: &[u8], program_number: u16, rows: &mut Vec<StreamRow>) {
    let Ok(pmt) = pmt::parse(section) else { return };
    let program_descriptors: DescriptorSummary =
        super::descriptors::walk(&pmt.program_descriptors);
    for entry in pmt.streams {
        let row = stream_table::build_row(entry.elementary_pid, program_number, &entry, &program_descriptors);
        rows.push(row);
    }
    let _ = (pmt.program_number, pmt.pcr_pid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mpeg_ts::packet::build_packet_with_pointer;
    use crate::media_metadata::mpeg_ts::pat::build_section as build_pat_section;
    use crate::media_metadata::mpeg_ts::pmt::build_section as build_pmt_section;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn assemble_ts(pmt_pid: u16) -> Vec<u8> {
        // Build a minimal TS file: PAT packet + PMT packet + padding packet.
        let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
        let pat_pkt = build_packet_with_pointer(0, &pat_section);

        let pmt_section = build_pmt_section(
            1,
            pmt_pid,
            &[],
            &[
                (0x1B, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00]), // H.264 + lang descriptor
                (0x0F, 0x111, vec![]),                                    // AAC
            ],
        );
        let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);

        // Add 4 padding packets so packet-size detection has plenty of sync bytes.
        let mut bytes = pat_pkt;
        bytes.extend(pmt_pkt);
        for _ in 0..6 {
            bytes.extend(crate::media_metadata::mpeg_ts::packet::build_packet(
                0x1FFF, false, &[],
            ));
        }
        bytes
    }

    #[test]
    fn probe_accepts_standard_188_byte_ts_stream() {
        let bytes = assemble_ts(0x100);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(MpegTsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_garbage() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 256]));
        assert!(!MpegTsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_avc_and_aac_tracks() {
        let bytes = assemble_ts(0x100);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ts", 0);
        MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(
            out.container.format,
            crate::media_metadata::model::container::ContainerFormat::MpegTs
        );
        assert_eq!(out.tracks.len(), 2);
        assert_eq!(out.tracks[0].codec.id, "V_MPEG4/ISO/AVC");
        assert_eq!(out.tracks[1].codec.id, "A_AAC");
        // Language descriptor on the H.264 stream
        let lang = out.tracks[0].properties.common.language.as_ref().unwrap();
        assert_eq!(lang.iso639_2, "eng");
        // Programs container populated
        assert_eq!(out.container.properties.programs.len(), 1);
    }

    #[test]
    fn short_input_returns_unrecognised() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 64]));
        let mut out = MediaMetadata::new("clip.ts", 0);
        let err = MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }
}
