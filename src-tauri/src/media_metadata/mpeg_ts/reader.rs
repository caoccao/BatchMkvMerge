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

//! Top-level `MpegTsReader`. Pure-Rust port of `r_mpeg_ts.cpp`.
//!
//! - Packet size is detected with alignment + several sync confirmations
//!   (PARSER-053).
//! - PSI sections (PAT / PMT / SDT) are reassembled across packets using the
//!   declared `section_length` (PARSER-054); SDT (PID 0x0011) supplies program
//!   service provider/name (PARSER-055).
//! - Transport-error packets are dropped and continuity-counter discontinuities
//!   reset the in-progress section (PARSER-057).
//! - PIDs carrying PES that no PMT lists are content-sniffed as a fallback when
//!   no listed streams are found (PARSER-056).

use std::collections::HashMap;

use crate::media_metadata::audio::{ac3, mp3};
use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::{avc, mpeg_video};
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::descriptors::DescriptorSummary;
use super::descriptors::service;
use super::identify;
use super::packet::{self, PACKET_SIZE_BD_M2TS, detect_packet_size_aligned};
use super::pat;
use super::pmt;
use super::stream_table::{self, StreamRow};

const MAX_PACKETS: usize = 64 * 1024;
const PROBE_BYTES: usize = 16 * 1024;
const SDT_PID: u16 = 0x0011;
const NULL_PID: u16 = 0x1FFF;
const UNLISTED_PAYLOAD_CAP: usize = 16 * 1024;
const MAX_UNLISTED_PIDS: usize = 16;

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegTsReader;

/// Per-PID PSI section reassembler (PARSER-054).
#[derive(Default)]
struct SectionAssembler {
  buf: Vec<u8>,
  last_cc: Option<u8>,
  in_progress: bool,
}

impl SectionAssembler {
  /// Feed one packet's payload; return a complete section when one finishes.
  fn feed(&mut self, header: &packet::PacketHeader, payload: &[u8]) -> Option<Vec<u8>> {
    // Continuity check (PARSER-057): a gap means we lost packets, so any
    // partially assembled section must be discarded.
    if let Some(last) = self.last_cc {
      let expected = (last + 1) & 0x0F;
      if header.continuity_counter != expected && header.continuity_counter != last {
        self.buf.clear();
        self.in_progress = false;
      }
    }
    self.last_cc = Some(header.continuity_counter);

    if header.payload_unit_start {
      if payload.is_empty() {
        return None;
      }
      let pointer = payload[0] as usize;
      let start = 1 + pointer;
      if start > payload.len() {
        return None;
      }
      self.buf.clear();
      self.buf.extend_from_slice(&payload[start..]);
      self.in_progress = true;
    } else if self.in_progress {
      self.buf.extend_from_slice(payload);
    } else {
      return None;
    }

    if self.buf.len() >= 3 {
      let section_length = (((self.buf[1] as usize) & 0x0F) << 8) | self.buf[2] as usize;
      let total = 3 + section_length;
      if self.buf.len() >= total {
        let section = self.buf[..total].to_vec();
        self.buf.clear();
        self.in_progress = false;
        return Some(section);
      }
    }
    None
  }
}

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
    Ok(detect_packet_size_aligned(&head[..read]).is_some())
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let probe_len = src.read_at_most(&mut probe)?;
    let (packet_size, start) = detect_packet_size_aligned(&probe[..probe_len]).ok_or(ParseError::Unrecognised)?;
    let header_offset = if packet_size == PACKET_SIZE_BD_M2TS { 4 } else { 0 };

    src.seek_to(start as u64)?;
    let mut pmt_pids: HashMap<u16, u16> = HashMap::new(); // pmt_pid → program_number
    let mut rows: Vec<StreamRow> = Vec::new();
    let mut seen_pmt_pids: std::collections::HashSet<u16> = Default::default();
    let mut sdt_map: HashMap<u16, (String, String)> = HashMap::new();
    let mut assemblers: HashMap<u16, SectionAssembler> = HashMap::new();
    let mut pmt_stream_pids: std::collections::HashSet<u16> = Default::default();
    let mut unlisted: HashMap<u16, Vec<u8>> = HashMap::new();

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
      // Drop transport-error packets (PARSER-057).
      if header.transport_error || !header.has_payload() {
        continue;
      }
      let payload = packet::payload_slice(pkt, &header);
      if payload.is_empty() {
        continue;
      }

      if packet::is_pat_pid(header.pid) {
        if let Some(section) = assemblers.entry(0).or_default().feed(&header, payload) {
          handle_pat(&section, &mut pmt_pids);
        }
      } else if header.pid == SDT_PID {
        if let Some(section) = assemblers.entry(SDT_PID).or_default().feed(&header, payload) {
          handle_sdt(&section, &mut sdt_map);
        }
      } else if let Some(&prog) = pmt_pids.get(&header.pid) {
        let asm = assemblers.entry(header.pid).or_default();
        if let Some(section) = asm.feed(&header, payload) {
          if seen_pmt_pids.insert(header.pid) {
            handle_pmt(&section, prog, &mut rows, &mut pmt_stream_pids);
          }
        }
      } else if header.pid != NULL_PID && header.payload_unit_start {
        // Candidate unlisted PES PID (PARSER-056) — record its payload.
        if payload.len() >= 4 && payload[0] == 0 && payload[1] == 0 && payload[2] == 1 {
          if unlisted.len() < MAX_UNLISTED_PIDS || unlisted.contains_key(&header.pid) {
            let buf = unlisted.entry(header.pid).or_default();
            if buf.len() < UNLISTED_PAYLOAD_CAP {
              let take = (UNLISTED_PAYLOAD_CAP - buf.len()).min(payload.len());
              buf.extend_from_slice(&payload[..take]);
            }
          }
        }
      }

      // Stop once every known PMT is processed and (if present) the SDT.
      if !pmt_pids.is_empty() && seen_pmt_pids.len() == pmt_pids.len() && (!sdt_map.is_empty() || packet_count > 4096) {
        break;
      }
    }

    // Fallback content detection for unlisted PES PIDs (PARSER-056): only
    // when no PMT-listed streams were found, and only on a confident sniff.
    if rows.is_empty() {
      let mut pids: Vec<u16> = unlisted.keys().copied().collect();
      pids.sort_unstable();
      for pid in pids {
        if pmt_stream_pids.contains(&pid) {
          continue;
        }
        if let Some((kind, id, name)) = sniff_codec(&unlisted[&pid]) {
          rows.push(StreamRow {
            pid,
            stream_type: 0,
            program_number: 0,
            language: None,
            teletext_page: None,
            service_name: None,
            codec_id: id.to_string(),
            codec_name: name.to_string(),
            track_kind: kind,
            codec_private: None,
            hearing_impaired: None,
          });
        }
      }
    }

    identify::finalise_with_sdt(rows, &sdt_map, out);
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

fn handle_pmt(
  section: &[u8],
  program_number: u16,
  rows: &mut Vec<StreamRow>,
  stream_pids: &mut std::collections::HashSet<u16>,
) {
  let Ok(pmt) = pmt::parse(section) else { return };
  let program_descriptors: DescriptorSummary = super::descriptors::walk(&pmt.program_descriptors);
  for entry in pmt.streams {
    stream_pids.insert(entry.elementary_pid);
    let new_rows = stream_table::build_rows(entry.elementary_pid, program_number, &entry, &program_descriptors);
    rows.extend(new_rows);
  }
  let _ = (pmt.program_number, pmt.pcr_pid);
}

/// Parse a DVB Service Description Table section (PARSER-055), recording each
/// service id → (provider, name).
fn handle_sdt(section: &[u8], sdt: &mut HashMap<u16, (String, String)>) {
  // table_id 0x42 = actual TS SDT.
  if section.first() != Some(&0x42) || section.len() < 12 {
    return;
  }
  let section_length = (((section[1] as usize) & 0x0F) << 8) | section[2] as usize;
  let end = (3 + section_length).min(section.len()).saturating_sub(4); // strip CRC
  let mut pos = 11usize;
  while pos + 5 <= end {
    let service_id = u16::from_be_bytes([section[pos], section[pos + 1]]);
    let loop_len = (((section[pos + 3] as usize) & 0x0F) << 8) | section[pos + 4] as usize;
    let desc_start = pos + 5;
    let desc_end = (desc_start + loop_len).min(end);
    let mut d = desc_start;
    while d + 2 <= desc_end {
      let tag = section[d];
      let len = section[d + 1] as usize;
      let body_start = d + 2;
      let body_end = (body_start + len).min(desc_end);
      if tag == 0x48 {
        if let Some((provider, name)) = service::decode_full(&section[body_start..body_end]) {
          sdt.entry(service_id).or_insert((provider, name));
        }
      }
      d = body_end;
    }
    pos = desc_end;
  }
}

/// Confidently sniff an elementary payload's codec for the unlisted-PID
/// fallback. Returns `None` unless a real codec header is found.
fn sniff_codec(payload: &[u8]) -> Option<(TrackKind, &'static str, &'static str)> {
  for nal in avc::nal::split_nal_units(payload) {
    if nal.nal_unit_type == 7 {
      let rbsp = avc::nal::strip_emulation_prevention(nal.payload);
      if avc::sps::parse(&rbsp).is_ok() {
        return Some((TrackKind::Video, "V_MPEG4/ISO/AVC", "AVC/H.264"));
      }
    }
  }
  if mpeg_video::decode_sequence_header(payload).is_some() {
    return Some((TrackKind::Video, "V_MPEG2", "MPEG-2 Video"));
  }
  if ac3::find_frame_sync(payload).is_some() {
    return Some((TrackKind::Audio, "A_AC3", "AC-3"));
  }
  if mp3::find_consecutive_mp3_headers(payload, 4).is_some() {
    return Some((TrackKind::Audio, "A_MPEG/L3", "MPEG-1/2 Audio"));
  }
  None
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

  fn padding() -> Vec<u8> {
    crate::media_metadata::mpeg_ts::packet::build_packet(0x1FFF, false, &[])
  }

  fn assemble_ts(pmt_pid: u16) -> Vec<u8> {
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(
      1,
      pmt_pid,
      &[],
      &[
        (0x1B, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00]),
        (0x0F, 0x111, vec![]),
      ],
    );
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    for _ in 0..6 {
      bytes.extend(padding());
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
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 4096]));
    assert!(!MpegTsReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-053: alignment / leading garbage --------------------------

  #[test]
  fn probe_accepts_stream_with_leading_garbage() {
    let mut bytes = vec![0xAAu8; 100]; // not a multiple of 188
    bytes.extend(assemble_ts(0x100));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegTsReader.probe(&mut s).unwrap());
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
    let lang = out.tracks[0].properties.common.language.as_ref().unwrap();
    assert_eq!(lang.iso639_2, "eng");
    assert_eq!(out.container.properties.programs.len(), 1);
  }

  // ---- PARSER-054: PMT split across packets -----------------------------

  #[test]
  fn read_headers_reassembles_pmt_across_packets() {
    let pmt_pid = 0x100u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);

    let mut streams = Vec::new();
    for i in 0..40u16 {
      if i % 2 == 0 {
        streams.push((0x1Bu8, 0x200 + i, vec![]));
      } else {
        streams.push((0x0Fu8, 0x200 + i, vec![]));
      }
    }
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &streams);
    assert!(pmt_section.len() > 184, "section must exceed one packet payload");

    // Split into TS packets: head carries the pointer + start.
    let mut full = vec![0u8]; // pointer_field = 0
    full.extend_from_slice(&pmt_section);
    let payload_len = 184usize;
    let mut bytes = pat_pkt;
    let mut cc = 0u8;
    let mut idx = 0usize;
    while idx < full.len() {
      let chunk_end = (idx + payload_len).min(full.len());
      let chunk = &full[idx..chunk_end];
      let mut p = vec![packet::TS_SYNC_BYTE];
      let pusi = if idx == 0 { 0x40 } else { 0x00 };
      p.push(pusi | ((pmt_pid >> 8) as u8 & 0x1F));
      p.push((pmt_pid & 0xFF) as u8);
      p.push(0x10 | (cc & 0x0F));
      p.extend_from_slice(chunk);
      while p.len() < 188 {
        p.push(0xFF);
      }
      bytes.extend(p);
      cc = (cc + 1) & 0x0F;
      idx = chunk_end;
    }
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 40);
  }

  // ---- PARSER-055: SDT service name -------------------------------------

  #[test]
  fn read_headers_applies_sdt_service_name() {
    let provider = b"ACME";
    let name = b"Channel One";
    let mut desc_body = vec![0x01u8]; // service_type
    desc_body.push(provider.len() as u8);
    desc_body.extend_from_slice(provider);
    desc_body.push(name.len() as u8);
    desc_body.extend_from_slice(name);
    let mut descriptor = vec![0x48u8, desc_body.len() as u8];
    descriptor.extend_from_slice(&desc_body);

    let mut svc = Vec::new();
    svc.extend_from_slice(&1u16.to_be_bytes()); // service_id = 1
    svc.push(0x00); // EIT flags
    let loop_len = descriptor.len();
    svc.push(((loop_len >> 8) as u8) & 0x0F);
    svc.push((loop_len & 0xFF) as u8);
    svc.extend_from_slice(&descriptor);

    let mut body = Vec::new();
    body.extend_from_slice(&1u16.to_be_bytes()); // transport_stream_id
    body.push(0xC1); // version/current_next
    body.push(0); // section_number
    body.push(0); // last_section_number
    body.extend_from_slice(&1u16.to_be_bytes()); // original_network_id
    body.push(0xFF); // reserved
    body.extend_from_slice(&svc);
    body.extend_from_slice(&0u32.to_be_bytes()); // CRC placeholder

    let mut section = vec![0x42u8];
    let section_length = body.len();
    section.push(0x80 | ((section_length >> 8) as u8 & 0x0F));
    section.push((section_length & 0xFF) as u8);
    section.extend_from_slice(&body);

    let sdt_pkt = build_packet_with_pointer(SDT_PID, &section);

    let mut bytes = assemble_ts(0x100);
    bytes.extend(sdt_pkt);
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let prog = &out.container.properties.programs[0];
    assert_eq!(prog.service_name.as_deref(), Some("Channel One"));
    assert_eq!(prog.service_provider.as_deref(), Some("ACME"));
  }

  #[test]
  fn short_input_returns_unrecognised() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 64]));
    let mut out = MediaMetadata::new("clip.ts", 0);
    let err = MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
