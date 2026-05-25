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

//! End-to-end fixtures for the Phase 10 residual readers (FLV, RealMedia,
//! IVF).

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseOptions, parse};

fn write_tempfile(bytes: &[u8], ext: &str) -> std::path::PathBuf {
  let dir = std::env::temp_dir();
  let pid = std::process::id();
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
  let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let path = dir.join(format!("bmm-residual-{pid}-{nanos}-{seq}.{ext}"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

// -- IVF -------------------------------------------------------------------

fn build_ivf_header(fourcc: &[u8; 4], width: u16, height: u16) -> Vec<u8> {
  let mut buf = Vec::with_capacity(32);
  buf.extend_from_slice(b"DKIF");
  buf.extend_from_slice(&0u16.to_le_bytes()); // version
  buf.extend_from_slice(&32u16.to_le_bytes()); // header_size
  buf.extend_from_slice(fourcc);
  buf.extend_from_slice(&width.to_le_bytes());
  buf.extend_from_slice(&height.to_le_bytes());
  buf.extend_from_slice(&30_000u32.to_le_bytes()); // frame_rate_num
  buf.extend_from_slice(&1000u32.to_le_bytes()); // frame_rate_den
  buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
  buf.extend_from_slice(&0u32.to_le_bytes()); // unused
  buf
}

#[test]
fn parses_ivf_av1_clip() {
  let blob = build_ivf_header(b"AV01", 1920, 1080);
  let path = write_tempfile(&blob, "ivf");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Ivf);
  assert_eq!(m.tracks[0].codec.id, "V_AV1");
}

#[test]
fn parses_ivf_vp9_clip() {
  let blob = build_ivf_header(b"VP90", 1280, 720);
  let path = write_tempfile(&blob, "ivf");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks[0].codec.id, "V_VP9");
}

// -- FLV -------------------------------------------------------------------

fn build_flv_header(type_flags: u8) -> Vec<u8> {
  let mut buf = Vec::with_capacity(9);
  buf.extend_from_slice(b"FLV");
  buf.push(1);
  buf.push(type_flags);
  buf.extend_from_slice(&9u32.to_be_bytes());
  buf
}

fn build_flv_tag(tag_type: u8, payload: &[u8]) -> Vec<u8> {
  let mut buf = Vec::new();
  buf.extend_from_slice(&0u32.to_be_bytes()); // previous_tag_size
  buf.push(tag_type);
  let len = payload.len() as u32;
  buf.push(((len >> 16) & 0xFF) as u8);
  buf.push(((len >> 8) & 0xFF) as u8);
  buf.push((len & 0xFF) as u8);
  buf.extend_from_slice(&[0u8; 3]); // timestamp
  buf.push(0u8); // timestamp_ext
  buf.extend_from_slice(&[0u8; 3]); // stream id
  buf.extend_from_slice(payload);
  buf
}

fn minimal_avcc() -> Vec<u8> {
  let mut payload = vec![1, 66, 0, 40, 0xff, 0xe1];
  payload.extend_from_slice(&2u16.to_be_bytes());
  payload.extend_from_slice(&[0x67, 0x80]);
  payload.push(1);
  payload.extend_from_slice(&2u16.to_be_bytes());
  payload.extend_from_slice(&[0x68, 0x80]);
  payload
}

#[test]
fn parses_flv_avc_aac_clip() {
  let mut blob = build_flv_header(0x05); // audio + video
  // Video tag with AVC packet type 0 and an avcC header.
  let mut video_payload = vec![(1 << 4) | 7, 0, 0, 0, 0];
  video_payload.extend(minimal_avcc());
  blob.extend(build_flv_tag(0x09, &video_payload));
  // Audio tag with AAC packet type 0 and LC 44.1k stereo ASC.
  let audio_byte = (10 << 4) | (3 << 2) | (1 << 1) | 1;
  blob.extend(build_flv_tag(0x08, &[audio_byte, 0, 0x12, 0x10]));
  let path = write_tempfile(&blob, "flv");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Flv);
  assert_eq!(m.tracks.len(), 2);
  let v = m.tracks.iter().find(|t| t.track_type == TrackType::Video).unwrap();
  assert_eq!(v.codec.id, "V_MPEG4/ISO/AVC");
  let a = m.tracks.iter().find(|t| t.track_type == TrackType::Audio).unwrap();
  assert_eq!(a.codec.id, "A_AAC");
}

// -- RealMedia -------------------------------------------------------------

fn build_rm_chunk(id: [u8; 4], payload: &[u8]) -> Vec<u8> {
  let mut buf = Vec::with_capacity(10 + payload.len());
  buf.extend_from_slice(&id);
  let size = (10 + payload.len()) as u32;
  buf.extend_from_slice(&size.to_be_bytes());
  buf.extend_from_slice(&0u16.to_be_bytes());
  buf.extend_from_slice(payload);
  buf
}

fn build_video_props(fourcc: &[u8; 4], width: u16, height: u16, fps: f64) -> Vec<u8> {
  let mut buf = vec![0u8; 32];
  buf[0..4].copy_from_slice(&32u32.to_be_bytes());
  buf[4..8].copy_from_slice(b"VIDO");
  buf[8..12].copy_from_slice(fourcc);
  buf[12..14].copy_from_slice(&width.to_be_bytes());
  buf[14..16].copy_from_slice(&height.to_be_bytes());
  buf[16..18].copy_from_slice(&24u16.to_be_bytes());
  let fps_q16 = ((fps.trunc() as u32) << 16) | (((fps.fract() * 65536.0).round() as u32) & 0xFFFF);
  buf[24..28].copy_from_slice(&fps_q16.to_be_bytes());
  buf
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
  build_rm_chunk(*b"MDPR", &payload)
}

fn build_data() -> Vec<u8> {
  let mut payload = Vec::new();
  payload.extend_from_slice(&0u32.to_be_bytes());
  payload.extend_from_slice(&0u32.to_be_bytes());
  build_rm_chunk(*b"DATA", &payload)
}

fn build_prop() -> Vec<u8> {
  let mut payload = Vec::new();
  for _ in 0..5 {
    payload.extend_from_slice(&0u32.to_be_bytes());
  }
  payload.extend_from_slice(&60_000u32.to_be_bytes()); // duration_ms
  for _ in 0..3 {
    payload.extend_from_slice(&0u32.to_be_bytes());
  }
  payload.extend_from_slice(&1u16.to_be_bytes());
  payload.extend_from_slice(&0u16.to_be_bytes());
  build_rm_chunk(*b"PROP", &payload)
}

#[test]
fn parses_realmedia_video_clip() {
  let mut payload = Vec::new();
  payload.extend_from_slice(&0u32.to_be_bytes()); // format_version
  payload.extend_from_slice(&5u32.to_be_bytes()); // num_headers
  let mut blob = build_rm_chunk(*b".RMF", &payload);
  blob.extend(build_prop());
  let v_props = build_video_props(b"RV40", 1280, 720, 25.0);
  blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));
  blob.extend(build_data());

  let path = write_tempfile(&blob, "rm");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::RealMedia);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].codec.id, "V_REAL/RV40");
}
