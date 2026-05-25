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

//! End-to-end AVI fixtures.  Builds synthetic .avi blobs, writes them to a
//! tempfile and drives `media_metadata::parse` against the path.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseError, ParseOptions, parse};

fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(8 + payload.len() + 1);
  out.extend_from_slice(kind);
  out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
  out.extend_from_slice(payload);
  if payload.len() & 1 != 0 {
    out.push(0);
  }
  out
}

fn list(kind: &[u8; 4], list_type: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
  let mut payload = list_type.to_vec();
  for c in children {
    payload.extend(c);
  }
  chunk(kind, &payload)
}

fn build_avih(microsec_per_frame: u32, total_frames: u32, streams: u32, w: u32, h: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(56);
  p.extend_from_slice(&microsec_per_frame.to_le_bytes());
  p.extend_from_slice(&5_000_000u32.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes()); // padding_granularity
  p.extend_from_slice(&0x10u32.to_le_bytes()); // flags = HAS_INDEX
  p.extend_from_slice(&total_frames.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&streams.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&w.to_le_bytes());
  p.extend_from_slice(&h.to_le_bytes());
  p.extend_from_slice(&[0u8; 16]);
  p
}

fn build_strh(fcc_type: &[u8; 4], fcc_handler: &[u8; 4], scale: u32, rate: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(56);
  p.extend_from_slice(fcc_type);
  p.extend_from_slice(fcc_handler);
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&0u16.to_le_bytes());
  p.extend_from_slice(&0u16.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&scale.to_le_bytes());
  p.extend_from_slice(&rate.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&240u32.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&[0u8; 8]);
  p
}

fn build_bmih(w: i32, h: i32, compression: &[u8; 4]) -> Vec<u8> {
  let mut p = Vec::with_capacity(40);
  p.extend_from_slice(&40u32.to_le_bytes());
  p.extend_from_slice(&w.to_le_bytes());
  p.extend_from_slice(&h.to_le_bytes());
  p.extend_from_slice(&1u16.to_le_bytes());
  p.extend_from_slice(&24u16.to_le_bytes());
  p.extend_from_slice(compression);
  p.extend_from_slice(&0u32.to_le_bytes());
  p.extend_from_slice(&[0u8; 16]);
  p
}

fn build_wfx(tag: u16, channels: u16, rate: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(18);
  p.extend_from_slice(&tag.to_le_bytes());
  p.extend_from_slice(&channels.to_le_bytes());
  p.extend_from_slice(&rate.to_le_bytes());
  p.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
  p.extend_from_slice(&(channels * 2).to_le_bytes());
  p.extend_from_slice(&16u16.to_le_bytes());
  p.extend_from_slice(&0u16.to_le_bytes());
  p
}

fn build_video_strl(w: u16, h: u16) -> Vec<u8> {
  let strh = chunk(b"strh", &build_strh(b"vids", b"H264", 1001, 24000));
  let strf = chunk(b"strf", &build_bmih(w as i32, h as i32, b"H264"));
  list(b"LIST", b"strl", &[strh, strf])
}

fn build_audio_strl(rate: u32, channels: u16) -> Vec<u8> {
  let strh = chunk(b"strh", &build_strh(b"auds", b"\0\0\0\0", 1, rate));
  let strf = chunk(b"strf", &build_wfx(0x0055, channels, rate));
  list(b"LIST", b"strl", &[strh, strf])
}

fn build_text_strl() -> Vec<u8> {
  // A text stream carries just a strh — the subtitle content lives in movi.
  let strh = chunk(b"strh", &build_strh(b"txts", b"DXSA", 1, 1000));
  list(b"LIST", b"strl", &[strh])
}

fn gab2_chunk(content: &[u8]) -> Vec<u8> {
  let mut g = b"GAB2\0".to_vec();
  g.extend_from_slice(&2u16.to_le_bytes()); // filename block (id 2)
  g.extend_from_slice(&4u32.to_le_bytes());
  g.extend_from_slice(b"a.sr");
  g.extend_from_slice(&4u16.to_le_bytes()); // subtitle block (id 4)
  g.extend_from_slice(&(content.len() as u32).to_le_bytes());
  g.extend_from_slice(content);
  g
}

fn build_avi(streams: Vec<Vec<u8>>) -> Vec<u8> {
  build_avi_with_movi(streams, &[])
}

fn build_avi_with_movi(streams: Vec<Vec<u8>>, movi_chunks: &[Vec<u8>]) -> Vec<u8> {
  let avih = chunk(b"avih", &build_avih(41_708, 240, streams.len() as u32, 1920, 1080));
  let mut hdrl_children = vec![avih];
  hdrl_children.extend(streams);
  let hdrl = list(b"LIST", b"hdrl", &hdrl_children);
  let movi = list(b"LIST", b"movi", movi_chunks);
  let mut riff_payload = b"AVI ".to_vec();
  riff_payload.extend(hdrl);
  riff_payload.extend(movi);
  let total = riff_payload.len() as u32;
  let mut bytes = b"RIFF".to_vec();
  bytes.extend_from_slice(&total.to_le_bytes());
  bytes.extend(riff_payload);
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
  let path = dir.join(format!("bmm-avi-{pid}-{nanos}-{seq}.avi"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_video_and_audio_avi() {
  let bytes = build_avi(vec![build_video_strl(1920, 1080), build_audio_strl(48000, 2)]);
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Avi);
  assert!(m.container.recognized);
  assert_eq!(m.tracks.len(), 2);
  assert_eq!(m.tracks[0].track_type, TrackType::Video);
  assert_eq!(m.tracks[1].track_type, TrackType::Audio);
  let v = m.tracks[0].properties.video.as_ref().unwrap();
  assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
  let a = m.tracks[1].properties.audio.as_ref().unwrap();
  assert_eq!(a.channels, Some(2));
  assert_eq!(a.sampling_frequency, Some(48000.0));
}

#[test]
fn recognised_gab2_subtitle_creates_subtitle_track_after_audio() {
  // video(0), audio(1), text(2) → subtitle id == 2; text chunk tag "02tx".
  let srt = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHello world\r\n";
  let text_chunk = chunk(b"02tx", &gab2_chunk(srt));
  let bytes = build_avi_with_movi(
    vec![build_video_strl(1920, 1080), build_audio_strl(48000, 2), build_text_strl()],
    &[text_chunk],
  );
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 3);
  assert_eq!(m.tracks[0].id, 0);
  assert_eq!(m.tracks[0].track_type, TrackType::Video);
  assert_eq!(m.tracks[1].id, 1);
  assert_eq!(m.tracks[1].track_type, TrackType::Audio);
  assert_eq!(m.tracks[2].id, 2);
  assert_eq!(m.tracks[2].track_type, TrackType::Subtitles);
  assert_eq!(m.tracks[2].codec.id, "S_TEXT/UTF8");
}

#[test]
fn unrecognised_text_chunk_yields_no_subtitle_track() {
  let junk = b"plain prose, definitely not a subtitle file at all";
  let text_chunk = chunk(b"01tx", &gab2_chunk(junk));
  let bytes = build_avi_with_movi(
    vec![build_video_strl(1920, 1080), build_text_strl()],
    &[text_chunk],
  );
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  assert!(m.tracks.iter().all(|t| t.track_type != TrackType::Subtitles));
}

#[test]
fn random_bytes_not_recognised_as_avi() {
  let path = write_tempfile(&[0x42u8; 32]);
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn avi_without_hdrl_returns_malformed() {
  let mut riff_payload = b"AVI ".to_vec();
  riff_payload.extend(list(b"LIST", b"movi", &[]));
  let total = riff_payload.len() as u32;
  let mut bytes = b"RIFF".to_vec();
  bytes.extend_from_slice(&total.to_le_bytes());
  bytes.extend(riff_payload);
  let path = write_tempfile(&bytes);
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Malformed { .. }));
}
