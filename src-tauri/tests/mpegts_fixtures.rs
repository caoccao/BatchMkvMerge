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

fn build_ts(pmt_pid: u16, streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
  let pat = build_pat_section(1, &[(1, pmt_pid)]);
  let pmt = build_pmt_section(1, pmt_pid, streams);
  let mut bytes = build_packet_with_pointer(0, &pat);
  bytes.extend(build_packet_with_pointer(pmt_pid, &pmt));
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
fn parses_minimal_ts_with_avc_and_aac() {
  let bytes = build_ts(
    0x100,
    &[
      (0x1B, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00]), // H.264 + lang
      (0x0F, 0x111, vec![]),                                   // AAC
    ],
  );
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MpegTs);
  assert_eq!(m.tracks.len(), 2);
  assert_eq!(m.tracks[0].codec.id, "V_MPEG4/ISO/AVC");
  assert_eq!(m.tracks[1].codec.id, "A_AAC");
  assert_eq!(m.tracks[0].properties.common.language.as_ref().unwrap().iso639_2, "eng");
}

#[test]
fn ts_with_ac3_descriptor_promotes_private_stream_to_ac3() {
  let descs = vec![0x6A, 0x00]; // TAG_AC3, length 0
  let bytes = build_ts(0x100, &[(0x06, 0x120, descs)]);
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
