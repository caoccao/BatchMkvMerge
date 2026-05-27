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

//! End-to-end Ogg fixtures.  Builds synthetic .ogg blobs, writes them to a
//! tempfile and drives `media_metadata::parse` against the path.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseError, ParseOptions, parse};

const HEADER_FLAG_BOS: u8 = 0x02;

fn build_page(flags: u8, granule: u64, serial: u32, sequence: u32, packets: &[&[u8]]) -> Vec<u8> {
  let mut segments: Vec<u8> = Vec::new();
  let mut payload: Vec<u8> = Vec::new();
  for packet in packets {
    let mut remaining = packet.len();
    let mut offset = 0;
    while remaining >= 255 {
      segments.push(255);
      payload.extend_from_slice(&packet[offset..offset + 255]);
      offset += 255;
      remaining -= 255;
    }
    segments.push(remaining as u8);
    payload.extend_from_slice(&packet[offset..]);
  }
  let mut bytes = b"OggS".to_vec();
  bytes.push(0);
  bytes.push(flags);
  bytes.extend_from_slice(&granule.to_le_bytes());
  bytes.extend_from_slice(&serial.to_le_bytes());
  bytes.extend_from_slice(&sequence.to_le_bytes());
  bytes.extend_from_slice(&0u32.to_le_bytes()); // crc
  bytes.push(segments.len() as u8);
  bytes.extend_from_slice(&segments);
  bytes.extend_from_slice(&payload);
  bytes
}

fn vorbis_identification(channels: u8, sample_rate: u32) -> Vec<u8> {
  let mut p = vec![0x01u8];
  p.extend_from_slice(b"vorbis");
  p.extend_from_slice(&0u32.to_le_bytes()); // version
  p.push(channels);
  p.extend_from_slice(&sample_rate.to_le_bytes());
  p.extend_from_slice(&[0u8; 12]); // bitrate triplet
  p.push(0xB8);
  p.push(0x01);
  p
}

fn vorbis_comment_packet(vendor: &str, entries: &[(&str, &str)]) -> Vec<u8> {
  let mut p = vec![0x03u8];
  p.extend_from_slice(b"vorbis");
  p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
  p.extend_from_slice(vendor.as_bytes());
  p.extend_from_slice(&(entries.len() as u32).to_le_bytes());
  for (k, v) in entries {
    let entry = format!("{}={}", k, v);
    p.extend_from_slice(&(entry.len() as u32).to_le_bytes());
    p.extend_from_slice(entry.as_bytes());
  }
  p.push(0x01); // framing bit
  p
}

fn opus_head(channels: u8, input_sample_rate: u32) -> Vec<u8> {
  let mut p = b"OpusHead".to_vec();
  p.push(1);
  p.push(channels);
  p.extend_from_slice(&0u16.to_le_bytes());
  p.extend_from_slice(&input_sample_rate.to_le_bytes());
  p.extend_from_slice(&0u16.to_le_bytes());
  p.push(0);
  p
}

fn opus_tags(vendor: &str) -> Vec<u8> {
  let mut p = b"OpusTags".to_vec();
  p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
  p.extend_from_slice(vendor.as_bytes());
  p.extend_from_slice(&0u32.to_le_bytes());
  p
}

fn write_tempfile(bytes: &[u8], ext: &str) -> std::path::PathBuf {
  let dir = std::env::temp_dir();
  let pid = std::process::id();
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
  let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let path = dir.join(format!("bmm-ogg-{pid}-{nanos}-{seq}.{ext}"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_vorbis_stream_with_comments() {
  let bos = vorbis_identification(2, 44100);
  let comments = vorbis_comment_packet("libvorbis", &[("TITLE", "Track"), ("ARTIST", "Artist")]);
  // PARSER-181: Vorbis needs three header packets (ident + comments + setup)
  // before its `headers_read` is satisfied; without the setup packet the
  // stream is now erased by finalise (r_ogm.cpp:633).
  let mut setup = vec![0x05u8];
  setup.extend_from_slice(b"vorbis");
  setup.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
  let mut bytes = build_page(HEADER_FLAG_BOS, 0, 0xC0FE, 0, &[&bos]);
  bytes.extend(build_page(0, 0, 0xC0FE, 1, &[&comments, &setup]));
  let path = write_tempfile(&bytes, "ogg");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Ogg);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].codec.id, "A_VORBIS");
  let a = m.tracks[0].properties.audio.as_ref().unwrap();
  assert_eq!(a.channels, Some(2));
  assert_eq!(a.sampling_frequency, Some(44100.0));
  // TITLE + ARTIST + VENDOR
  assert_eq!(m.tracks[0].properties.tags.len(), 3);
}

#[test]
fn parses_opus_stream() {
  let bos = opus_head(2, 48000);
  let tags = opus_tags("libopus 1.4");
  let mut bytes = build_page(HEADER_FLAG_BOS, 0, 1, 0, &[&bos]);
  bytes.extend(build_page(0, 0, 1, 1, &[&tags]));
  let path = write_tempfile(&bytes, "opus");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].codec.id, "A_OPUS");
  assert_eq!(m.tracks[0].track_type, TrackType::Audio);
  let a = m.tracks[0].properties.audio.as_ref().unwrap();
  assert_eq!(a.channels, Some(2));
  // PARSER-152: the OpusHead input_sample_rate is reported as the sampling
  // frequency; output_sampling_frequency is left unset (mkvtoolnix parity).
  assert_eq!(a.sampling_frequency, Some(48000.0));
  assert_eq!(a.output_sampling_frequency, None);
}

#[test]
fn random_bytes_not_recognised_as_ogg() {
  let path = write_tempfile(&[0x42u8; 16], "bin");
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn empty_ogg_file_returns_empty_track_list() {
  // Empty file = no pages = no tracks
  let path = write_tempfile(&[], "ogg");
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn ogg_page_only_without_bos_is_rejected() {
  // Build a minimal OggS-prefixed file (probe will accept) but with no
  // BOS pages — mkvtoolnix rejects it instead of recognising an empty Ogg.
  let mut bytes = b"OggS".to_vec();
  bytes.extend_from_slice(&[0u8; 23]); // minimum-length stub
  bytes[26] = 0; // 0 segments → still a valid header
  let path = write_tempfile(&bytes, "ogg");
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Malformed { format: "ogg", .. }));
}
