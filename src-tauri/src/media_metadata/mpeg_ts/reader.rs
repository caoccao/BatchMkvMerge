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

use crate::media_metadata::audio::{aac, ac3, mp3};
use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::{avc, hevc, mpeg_video, vc1};
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::descriptors::DescriptorSummary;
use super::descriptors::service;
use super::identify::{self, EsEnrichment};
use super::packet::{self, PACKET_SIZE_BD_M2TS, detect_packet_size_aligned};
use super::pat;
use super::pmt;
use super::stream_table::{self, StreamRow};

const MAX_PACKETS: usize = 64 * 1024;
const PROBE_BYTES: usize = 16 * 1024;
const SDT_PID: u16 = 0x0011;
const NULL_PID: u16 = 0x1FFF;
/// PARSER-158: bounded per-PID PES accumulation for codec probing.  We hold up
/// to 64 KiB of each candidate stream's payload — enough for an AVC/HEVC SPS or
/// the first audio frame header — across at most this many PIDs.
const PES_PAYLOAD_CAP: usize = 64 * 1024;
const MAX_PES_PIDS: usize = 32;
/// Packet budget scanned (after all PMTs are seen) before stopping, so the
/// per-PID PES buffers have a chance to fill for the header probe.
const PES_PROBE_PACKET_BUDGET: usize = 4096;

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
    // PARSER-056 / PARSER-158: bounded per-PID PES payload accumulation, used
    // both to sniff unlisted PIDs and to probe codec parameters for listed
    // tracks.
    let mut pes_payloads: HashMap<u16, Vec<u8>> = HashMap::new();

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
      } else if header.pid != NULL_PID {
        // Candidate PES PID (PARSER-056 / PARSER-158).  Start a buffer at the
        // first PES-start packet, then keep appending continuation packets up
        // to the cap so the codec probe has enough of the elementary stream.
        let is_pes_start =
          header.payload_unit_start && payload.len() >= 4 && payload[0] == 0 && payload[1] == 0 && payload[2] == 1;
        let known = pes_payloads.contains_key(&header.pid);
        if known || (is_pes_start && pes_payloads.len() < MAX_PES_PIDS) {
          let buf = pes_payloads.entry(header.pid).or_default();
          if buf.len() < PES_PAYLOAD_CAP {
            let take = (PES_PAYLOAD_CAP - buf.len()).min(payload.len());
            buf.extend_from_slice(&payload[..take]);
          }
        }
      }

      // Stop once every known PMT is processed and the PES probe budget is
      // spent (PARSER-158: scan far enough to fill the per-PID buffers rather
      // than bailing the moment the SDT arrives).
      if !pmt_pids.is_empty() && seen_pmt_pids.len() == pmt_pids.len() && packet_count > PES_PROBE_PACKET_BUDGET {
        break;
      }
    }

    // Fallback content detection for unlisted PES PIDs (PARSER-056): only
    // when no PMT-listed streams were found, and only on a confident sniff.
    if rows.is_empty() {
      let mut pids: Vec<u16> = pes_payloads.keys().copied().collect();
      pids.sort_unstable();
      for pid in pids {
        if pmt_stream_pids.contains(&pid) {
          continue;
        }
        if let Some((kind, id, name)) = sniff_codec(strip_pes_header(&pes_payloads[&pid])) {
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

    // PARSER-158: probe each track's PES payload for codec parameters the PMT
    // alone cannot supply (audio channels / sampling rate, video dimensions).
    let enrichment = compute_enrichment(&rows, &pes_payloads);

    identify::finalise_with_sdt(rows, &sdt_map, &enrichment, out);
    Ok(())
  }
}

/// PARSER-158: strip the PES packet header so codec probes see the elementary
/// stream bytes.  Streams carrying the full optional header (`PTS_DTS_flags`
/// etc.) start their payload at `9 + PES_header_data_length`; the few stream
/// ids that never carry it (program stream map / padding / private-2 / ECM /
/// EMM / directory / DSMCC / H.222.1-E) start right after the 6-byte prefix.
fn strip_pes_header(bytes: &[u8]) -> &[u8] {
  if bytes.len() < 6 || bytes[0..3] != [0x00, 0x00, 0x01] {
    return bytes;
  }
  match bytes[3] {
    0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF => bytes.get(6..).unwrap_or(&[]),
    _ => {
      if bytes.len() < 9 {
        return &[];
      }
      let header_data_len = bytes[8] as usize;
      bytes.get(9 + header_data_len..).unwrap_or(&[])
    }
  }
}

/// PARSER-158: build the per-PID enrichment map by running the codec-specific
/// header probe over the accumulated elementary stream.  One entry per PID
/// (the primary stream); the TrueHD/AC-3 coupled case (stream_type 0x83) is
/// skipped because its embedded sub-stream needs the demux-time splitter.
fn compute_enrichment(rows: &[StreamRow], pes: &HashMap<u16, Vec<u8>>) -> HashMap<u16, EsEnrichment> {
  let mut map: HashMap<u16, EsEnrichment> = HashMap::new();
  for row in rows {
    if map.contains_key(&row.pid) || row.stream_type == 0x83 {
      continue;
    }
    let Some(raw) = pes.get(&row.pid) else {
      continue;
    };
    if let Some(enrichment) = enrich_for_codec(&row.codec_id, strip_pes_header(raw)) {
      map.insert(row.pid, enrichment);
    }
  }
  map
}

/// Decode the codec-specific header at the start of an elementary stream into
/// the parameters mkvmerge recovers from its bounded probe.
fn enrich_for_codec(codec_id: &str, es: &[u8]) -> Option<EsEnrichment> {
  match codec_id {
    "V_MPEG4/ISO/AVC" => {
      for nal in avc::nal::split_nal_units(es) {
        if nal.nal_unit_type == 7 {
          let rbsp = avc::nal::strip_emulation_prevention(nal.payload);
          if let Ok(sps) = avc::sps::parse(&rbsp) {
            return Some(EsEnrichment {
              pixel_dimensions: Some((sps.display_width, sps.display_height)),
              ..EsEnrichment::default()
            });
          }
        }
      }
      None
    }
    "V_MPEGH/ISO/HEVC" | "V_HEVC" => {
      for nal in hevc::nal::split_nal_units(es) {
        // HEVC SPS_NUT == 33.
        if nal.nal_unit_type == 33 {
          let rbsp = hevc::nal::strip_emulation_prevention(nal.payload);
          if let Ok(sps) = hevc::sps::parse(&rbsp) {
            return Some(EsEnrichment {
              pixel_dimensions: Some((sps.display_width, sps.display_height)),
              ..EsEnrichment::default()
            });
          }
        }
      }
      None
    }
    "V_MPEG1" | "V_MPEG2" => mpeg_video::decode_sequence_header(es).map(|h| EsEnrichment {
      pixel_dimensions: Some((h.horizontal_size, h.vertical_size)),
      ..EsEnrichment::default()
    }),
    "V_VC1" => vc1::decode_sequence_header(es).map(|h| EsEnrichment {
      pixel_dimensions: Some((h.max_coded_width, h.max_coded_height)),
      ..EsEnrichment::default()
    }),
    "A_AC3" | "A_EAC3" => {
      // The PMT already tells us this is AC-3, so probe the first decodable
      // frame at a sync word rather than requiring several consecutive frames
      // (a bounded PES sample may carry only one).
      let frame = first_ac3_frame(es)?;
      Some(EsEnrichment {
        channels: Some(frame.channels),
        sampling_frequency: Some(frame.sample_rate as f64),
        ..EsEnrichment::default()
      })
    }
    "A_MPEG/L3" => {
      let (_, header) = mp3::find_consecutive_mp3_headers(es, 1)?;
      Some(EsEnrichment {
        channels: Some(header.channels),
        sampling_frequency: Some(header.sampling_frequency as f64),
        ..EsEnrichment::default()
      })
    }
    "A_AAC" => {
      let header = first_adts_header(es)?;
      Some(EsEnrichment {
        channels: Some(header.channels),
        sampling_frequency: Some(header.sample_rate as f64),
        ..EsEnrichment::default()
      })
    }
    _ => None,
  }
}

/// Decode the first ADTS frame found at a `0xFFF` sync in `es` — the PMT stream
/// type already identified AAC, so one frame is enough (no multi-frame
/// confirmation required).
fn first_adts_header(es: &[u8]) -> Option<aac::AacHeader> {
  let mut i = 0;
  while i + 7 <= es.len() {
    if es[i] == 0xFF && (es[i + 1] & 0xF0) == 0xF0 {
      if let Some(header) = aac::decode_adts(&es[i..]) {
        return Some(header);
      }
    }
    i += 1;
  }
  None
}

/// Decode the first AC-3 frame found at a `0x0B77` sync word in `es`.  Unlike
/// [`ac3::find_frame_sync`], this does not require several confirming frames —
/// the PMT stream type already identified the codec.
fn first_ac3_frame(es: &[u8]) -> Option<ac3::Ac3Frame> {
  let mut i = 0;
  while i + 2 <= es.len() {
    if es[i] == 0x0B && es[i + 1] == 0x77 {
      if let Some(frame) = ac3::decode_frame(&es[i..]) {
        return Some(frame);
      }
    }
    i += 1;
  }
  None
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
  fn read_headers_enriches_ac3_audio_from_pes_payload() {
    // PARSER-158: a Blu-ray AC-3 stream (stream_type 0x81) carrying a real
    // AC-3 frame in its PES payload must surface channels + sampling frequency
    // probed from that frame, not an empty audio struct.
    let pmt_pid = 0x100u16;
    let ac3_pid = 0x111u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x81, ac3_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);

    // fscod=0 (48 kHz), bsid=8 (AC-3), acmod=7 (3/2) + LFE → 6 channels.
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame_full(0, 0, 8, 7, true);
    let mut pes = vec![0x00, 0x00, 0x01, 0xBD]; // private_stream_1
    let pes_len = (3 + frame.len()) as u16; // flags(2) + header_data_length(1) + ES
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80); // '10' marker + flags
    pes.push(0x00); // no PTS/DTS
    pes.push(0x00); // PES_header_data_length = 0
    pes.extend_from_slice(&frame);
    let pes_pkt = packet::build_packet(ac3_pid, true, &pes);

    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_pkt);
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_AC3");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn strip_pes_header_handles_both_stream_forms() {
    // Extended-header stream (video 0xE0): ES starts at 9 + header_data_len.
    let mut pes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x80, 0x00, 0x02, 0xAA, 0xBB];
    pes.extend_from_slice(&[0x11, 0x22, 0x33]); // ES
    assert_eq!(strip_pes_header(&pes), &[0x11, 0x22, 0x33]);

    // Padding stream (0xBE) carries no optional header — ES right after byte 6.
    let pad = vec![0x00, 0x00, 0x01, 0xBE, 0x00, 0x00, 0x44, 0x55];
    assert_eq!(strip_pes_header(&pad), &[0x44, 0x55]);

    // Non-PES bytes pass through unchanged.
    assert_eq!(strip_pes_header(&[0x01, 0x02]), &[0x01, 0x02]);
  }

  #[test]
  fn enrich_for_codec_decodes_each_supported_codec() {
    use crate::media_metadata::audio::{aac, ac3, mp3};
    use crate::media_metadata::elementary::{mpeg_video, vc1};

    // MPEG-2 video → pixel dimensions.
    let es = mpeg_video::build_sequence_header(1280, 720, 4);
    assert_eq!(enrich_for_codec("V_MPEG2", &es).unwrap().pixel_dimensions, Some((1280, 720)));

    // VC-1 → pixel dimensions.
    let es = vc1::build_sequence_header(1920, 1080);
    assert_eq!(enrich_for_codec("V_VC1", &es).unwrap().pixel_dimensions, Some((1920, 1080)));

    // MP3 → channels + sampling frequency.
    let es = mp3::build_mp3_frame_v1(128, 44100, false);
    let m = enrich_for_codec("A_MPEG/L3", &es).unwrap();
    assert_eq!(m.channels, Some(2));
    assert_eq!(m.sampling_frequency, Some(44100.0));

    // AAC (ADTS, sr_index 3 = 48 kHz, channel_config 2).
    let es = aac::build_adts_frame(1, 3, 2);
    let m = enrich_for_codec("A_AAC", &es).unwrap();
    assert_eq!(m.channels, Some(2));
    assert_eq!(m.sampling_frequency, Some(48000.0));

    // AC-3 (acmod 7 + LFE = 6 channels, 48 kHz).
    let es = ac3::build_ac3_frame_full(0, 0, 8, 7, true);
    let m = enrich_for_codec("A_AC3", &es).unwrap();
    assert_eq!(m.channels, Some(6));
    assert_eq!(m.sampling_frequency, Some(48000.0));

    // Unknown / unprobeable codec yields nothing.
    assert!(enrich_for_codec("S_HDMV/PGS", &es).is_none());
    assert!(enrich_for_codec("A_AC3", &[0u8; 4]).is_none());
  }

  #[test]
  fn short_input_returns_unrecognised() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 64]));
    let mut out = MediaMetadata::new("clip.ts", 0);
    let err = MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
