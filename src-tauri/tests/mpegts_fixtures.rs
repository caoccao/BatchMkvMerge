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

//! End-to-end MPEG-TS fixtures.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseError, ParseOptions, parse};

const PACKET: usize = 188;
const SYNC: u8 = 0x47;

fn build_packet(pid: u16, payload_unit_start: bool, payload: &[u8]) -> Vec<u8> {
  let mut p = Vec::with_capacity(PACKET);
  p.push(SYNC);
  let b1 = ((payload_unit_start as u8) << 6) | ((pid >> 8) as u8 & 0x1F);
  p.push(b1);
  p.push((pid & 0xFF) as u8);
  p.push(0x10);
  p.extend_from_slice(payload);
  while p.len() < PACKET {
    p.push(0xFF);
  }
  p
}

fn build_pat_section(transport_stream_id: u16, entries: &[(u16, u16)]) -> Vec<u8> {
  let body_len = 5 + entries.len() * 4 + 4;
  let section_length = body_len as u16;
  let mut section = Vec::new();
  section.push(0x00);
  section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F));
  section.push((section_length & 0xFF) as u8);
  section.extend_from_slice(&transport_stream_id.to_be_bytes());
  section.push(0xC1);
  section.push(0x00);
  section.push(0x00);
  for (program, pid) in entries {
    section.extend_from_slice(&program.to_be_bytes());
    section.extend_from_slice(&(0xE000 | (pid & 0x1FFF)).to_be_bytes());
  }
  section.extend_from_slice(&0u32.to_be_bytes());
  section
}

fn build_pmt_section(program_number: u16, pcr_pid: u16, streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
  let mut body = Vec::new();
  body.extend_from_slice(&program_number.to_be_bytes());
  body.push(0xC1);
  body.push(0x00);
  body.push(0x00);
  body.extend_from_slice(&(0xE000 | (pcr_pid & 0x1FFF)).to_be_bytes());
  body.extend_from_slice(&(0xF000u16).to_be_bytes()); // program_info_length = 0
  for (st, pid, descs) in streams {
    body.push(*st);
    body.extend_from_slice(&(0xE000 | (pid & 0x1FFF)).to_be_bytes());
    body.extend_from_slice(&(0xF000 | (descs.len() as u16 & 0x0FFF)).to_be_bytes());
    body.extend_from_slice(descs);
  }
  body.extend_from_slice(&0u32.to_be_bytes()); // CRC
  let section_length = body.len() as u16;
  let mut section = Vec::new();
  section.push(0x02);
  section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F));
  section.push((section_length & 0xFF) as u8);
  section.extend(body);
  section
}

fn build_packet_with_pointer(pid: u16, section: &[u8]) -> Vec<u8> {
  let mut payload = vec![0u8]; // pointer_field
  payload.extend_from_slice(section);
  build_packet(pid, true, &payload)
}

/// Wrap an elementary-stream payload in a minimal PES packet (extended-header
/// form, no PTS/DTS) and emit one TS packet on `pid`.
fn build_pes_packet(pid: u16, es: &[u8]) -> Vec<u8> {
  let mut pes = vec![0x00, 0x00, 0x01, 0xE0];
  let pes_len = (3 + es.len()) as u16;
  pes.extend_from_slice(&pes_len.to_be_bytes());
  pes.push(0x80);
  pes.push(0x00);
  pes.push(0x00);
  pes.extend_from_slice(es);
  build_packet(pid, true, &pes)
}

/// A valid MPEG-2 sequence header (1280x720) — start code `00 00 01 B3` +
/// 12-bit width + 12-bit height + 4-bit aspect + 4-bit frame-rate-code.
fn mpeg2_sequence_header() -> Vec<u8> {
  let (width, height): (u32, u32) = (1280, 720);
  let mut bytes = vec![0x00, 0x00, 0x01, 0xB3];
  bytes.push(((width >> 4) & 0xFF) as u8);
  bytes.push((((width & 0x0F) << 4) | ((height >> 8) & 0x0F)) as u8);
  bytes.push((height & 0xFF) as u8);
  bytes.push((1u8 << 4) | 4); // aspect_ratio marker + frame_rate_code 4
  bytes.extend_from_slice(&[0u8; 4]);
  bytes
}

/// One 8-byte ADTS AAC frame (profile 1, sr_index 3 = 48 kHz, 2 channels).
fn adts_single_frame() -> Vec<u8> {
  let (profile, sr_index, channel_config, frame_length): (u8, u8, u8, u16) = (1, 3, 2, 8);
  let mut bytes = vec![0u8; frame_length as usize];
  bytes[0] = 0xFF;
  bytes[1] = 0xF1;
  bytes[2] = (profile << 6) | (sr_index << 2) | ((channel_config >> 2) & 0x01);
  bytes[3] = ((channel_config & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03);
  bytes[4] = ((frame_length >> 3) & 0xFF) as u8;
  bytes[5] = (((frame_length & 0x07) << 5) | 0x1F) as u8;
  bytes[6] = 0xFC;
  bytes
}

/// Six consecutive ADTS frames. PARSER-206: MPEG-TS AAC enrichment now requires
/// five consecutive frames (matching `find_consecutive_frames(..., 5)`), so a
/// real AAC stream must carry several frames.
fn adts_frame() -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..6 {
    bytes.extend(adts_single_frame());
  }
  bytes
}

/// A minimal AC-3 frame (fscod 0 = 48 kHz, frmsizecod 0 = 128 bytes, bsid 8,
/// acmod 2 stereo, no LFE) decodable by the native AC-3 header parser.
fn ac3_frame() -> Vec<u8> {
  let (fscod, frmsizecod, bsid, acmod): (u8, u8, u8, u8) = (0, 0, 8, 2);
  let mut bytes = vec![0u8; 128];
  bytes[0] = 0x0B;
  bytes[1] = 0x77;
  bytes[4] = (fscod << 6) | (frmsizecod & 0x3F);
  bytes[5] = (bsid & 0x1F) << 3;
  bytes[6] = (acmod & 0x07) << 5; // lfeon = 0
  bytes
}

/// Build a single-program TS (PAT + PMT) and append one PES packet (built from
/// `es`) per `(pid, es)` entry after the PMT, so PARSER-169's probed_ok filter
/// keeps the rows.
fn build_ts_with_pes(pmt_pid: u16, streams: &[(u8, u16, Vec<u8>)], pes: &[(u16, Vec<u8>)]) -> Vec<u8> {
  let pat = build_pat_section(1, &[(1, pmt_pid)]);
  let pmt = build_pmt_section(1, pmt_pid, streams);
  let mut bytes = build_packet_with_pointer(0, &pat);
  bytes.extend(build_packet_with_pointer(pmt_pid, &pmt));
  for (pid, es) in pes {
    bytes.extend(build_pes_packet(*pid, es));
  }
  for _ in 0..8 {
    bytes.extend(build_packet(0x1FFF, false, &[]));
  }
  bytes
}

fn write_tempfile(bytes: &[u8]) -> std::path::PathBuf {
  let dir = std::env::temp_dir();
  let pid = std::process::id();
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
  let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let path = dir.join(format!("bmm-mpegts-{pid}-{nanos}-{seq}.ts"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_minimal_ts_with_video_and_aac() {
  // PARSER-169: PMT-listed rows survive only when their bounded PES header
  // probes successfully, so each stream carries a real elementary header.
  // (AVC SPS bytes are hard to synthesise by hand in an integration test, so
  // the video stream is MPEG-2 here; the AVC SPS path is covered by the unit
  // tests in elementary/avc + mpeg_ts::reader.)
  let bytes = build_ts_with_pes(
    0x100,
    &[
      (0x02, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00]), // MPEG-2 + lang
      (0x0F, 0x111, vec![]),                                   // AAC
    ],
    &[(0x110, mpeg2_sequence_header()), (0x111, adts_frame())],
  );
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MpegTs);
  assert_eq!(m.tracks.len(), 2);
  assert_eq!(m.tracks[0].codec.id, "V_MPEG2");
  assert_eq!(m.tracks[1].codec.id, "A_AAC");
  // PARSER-171: `number` equals the elementary PID.
  assert_eq!(m.tracks[0].properties.common.number, Some(0x110));
  assert_eq!(m.tracks[1].properties.common.number, Some(0x111));
  assert_eq!(m.tracks[0].properties.common.language.as_ref().unwrap().iso639_2, "eng");
}

#[test]
fn ts_with_ac3_descriptor_promotes_private_stream_to_ac3() {
  // PARSER-169: the promoted A_AC3 row also needs a decodable AC-3 PES frame.
  let descs = vec![0x6A, 0x00]; // TAG_AC3, length 0
  let bytes = build_ts_with_pes(0x100, &[(0x06, 0x120, descs)], &[(0x120, ac3_frame())]);
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].codec.id, "A_AC3");
  assert_eq!(m.tracks[0].track_type, TrackType::Audio);
}

#[test]
fn random_bytes_not_recognised_as_ts() {
  let path = write_tempfile(&[0x42u8; 256]);
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}
