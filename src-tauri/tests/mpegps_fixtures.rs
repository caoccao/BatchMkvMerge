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

//! End-to-end MPEG-PS fixtures.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseError, ParseOptions, parse};

fn start_code(stream_id: u8) -> [u8; 4] {
  [0x00, 0x00, 0x01, stream_id]
}

fn build_ps(stream_ids: &[u8]) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(&start_code(0xBA));
  bytes.extend_from_slice(&[0u8; 10]);
  for id in stream_ids {
    bytes.extend_from_slice(&start_code(*id));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0x80, 0x00, 0x00]);
    if *id == 0xBD {
      bytes.push(0x80);
      bytes.extend_from_slice(&[0u8; 4]);
    } else {
      bytes.extend_from_slice(&[0u8; 5]);
    }
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
  let path = dir.join(format!("bmm-mpegps-{pid}-{nanos}-{seq}.mpg"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_minimal_ps_with_video_and_audio() {
  let bytes = build_ps(&[0xE0, 0xC0, 0xBD]);
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MpegPs);
  assert_eq!(m.tracks.len(), 3);
  let kinds: Vec<TrackType> = m.tracks.iter().map(|t| t.track_type).collect();
  assert!(kinds.contains(&TrackType::Video));
  assert!(kinds.contains(&TrackType::Audio));
}

#[test]
fn random_bytes_not_recognised_as_ps() {
  let path = write_tempfile(&[0x42u8; 64]);
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn duplicate_stream_ids_deduped() {
  let bytes = build_ps(&[0xE0, 0xE0, 0xC0, 0xC0, 0xE0]);
  let path = write_tempfile(&bytes);
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 2);
}
