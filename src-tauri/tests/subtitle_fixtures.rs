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

//! End-to-end fixtures for the subtitle readers (SRT, SSA/ASS, WebVTT, USF,
//! MicroDVD, VobSub, PGS, HDMV TextST, VobButton).

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
  let path = dir.join(format!("bmm-subs-{pid}-{nanos}-{seq}.{ext}"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_srt_clip() {
  let blob = b"1\r\n00:00:00,000 --> 00:00:02,500\r\nHello world\r\n\r\n";
  let path = write_tempfile(blob, "srt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Srt);
  assert_eq!(m.tracks[0].track_type, TrackType::Subtitles);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/UTF8");
}

#[test]
fn parses_ass_clip() {
  let blob = b"[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n";
  let path = write_tempfile(blob, "ass");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::SsaAss);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/ASS");
}

#[test]
fn parses_ssa_clip() {
  let blob = b"[Script Info]\nScriptType: v4.00\n\n[V4 Styles]\n";
  let path = write_tempfile(blob, "ssa");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/SSA");
}

#[test]
fn parses_webvtt_clip() {
  let blob = b"WEBVTT\n\n00:00.000 --> 00:02.000\nHello\n";
  let path = write_tempfile(blob, "vtt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Webvtt);
}

#[test]
fn parses_usf_clip() {
  let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\">\n</USFSubtitles>\n";
  let path = write_tempfile(blob, "usf");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Usf);
}

#[test]
fn parses_microdvd_clip() {
  let blob = b"{1}{125}Hello world\n{126}{250}Second line\n";
  let path = write_tempfile(blob, "sub");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MicroDvd);
}

#[test]
fn parses_vobsub_idx_with_per_language_entries() {
  let blob = b"# VobSub index file, v7
id: en, index: 0
timestamp: 00:00:01:000, filepos: 000000000
id: ja, index: 1
timestamp: 00:00:02:000, filepos: 000000100
";
  let path = write_tempfile(blob, "idx");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::VobSub);
  assert_eq!(m.tracks.len(), 2);
}

fn build_pgs_segment(seg_type: u8, payload_len: u16) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"PG");
  bytes.extend_from_slice(&[0u8; 4]); // PTS
  bytes.extend_from_slice(&[0u8; 4]); // DTS
  bytes.push(seg_type);
  bytes.extend_from_slice(&payload_len.to_be_bytes());
  bytes.extend(std::iter::repeat(0u8).take(payload_len as usize));
  bytes
}

#[test]
fn parses_pgs_sup_clip() {
  let mut blob = build_pgs_segment(0x16, 11); // PCS
  blob.extend(build_pgs_segment(0x17, 9)); // WDS
  let path = write_tempfile(&blob, "sup");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::HdmvPgs);
  let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
  assert!(!sub.text_subtitles);
}

#[test]
fn parses_hdmv_textst_clip() {
  let mut blob = b"TextST".to_vec();
  blob.push(0x81); // Dialog Style
  blob.extend_from_slice(&(8u16).to_be_bytes());
  blob.extend_from_slice(&[0u8; 8]);
  let path = write_tempfile(&blob, "textst");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::HdmvTextSt);
}

#[test]
fn parses_vobbtn_clip() {
  let mut blob = b"butonDVD".to_vec();
  blob.extend_from_slice(&[0u8; 8]);
  blob.extend_from_slice(&[0x00, 0x00, 0x01, 0xBF]);
  blob.extend_from_slice(&[0x03, 0xD4, 0x00]);
  blob.extend_from_slice(&[0u8; 16]);
  let path = write_tempfile(&blob, "btn");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::VobButton);
  assert_eq!(m.tracks[0].track_type, TrackType::Buttons);
}
