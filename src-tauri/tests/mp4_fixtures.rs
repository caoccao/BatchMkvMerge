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

//! End-to-end MP4 fixtures.  Builds complete synthetic .mp4 blobs, writes
//! them to a tempfile and drives `media_metadata::parse` against the path.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseError, ParseOptions, parse};

// =============================================================================
//   MP4 box encoders (kept local — test crate doesn't pull in pub(crate) helpers).
// =============================================================================

fn encode_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
  let total = (8 + payload.len()) as u32;
  let mut out = Vec::with_capacity(total as usize);
  out.extend_from_slice(&total.to_be_bytes());
  out.extend_from_slice(kind);
  out.extend_from_slice(payload);
  out
}

fn ftyp(major: &[u8; 4], compats: &[&[u8; 4]]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(major);
  p.extend_from_slice(&0u32.to_be_bytes());
  for c in compats {
    p.extend_from_slice(*c);
  }
  encode_box(b"ftyp", &p)
}

fn mvhd(timescale: u32, duration: u32, next_track_id: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(100);
  p.push(0);
  p.extend_from_slice(&[0u8; 3]);
  p.extend_from_slice(&[0u8; 8]);
  p.extend_from_slice(&timescale.to_be_bytes());
  p.extend_from_slice(&duration.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // rate
  p.extend_from_slice(&[0u8; 2 + 10 + 36 + 24]);
  p.extend_from_slice(&next_track_id.to_be_bytes());
  encode_box(b"mvhd", &p)
}

fn tkhd(track_id: u32, width: u16, height: u16) -> Vec<u8> {
  let mut p = Vec::with_capacity(84);
  p.push(0);
  p.extend_from_slice(&[0u8, 0u8, 0x01u8]); // track_enabled
  p.extend_from_slice(&0u32.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes());
  p.extend_from_slice(&track_id.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // reserved
  p.extend_from_slice(&0u32.to_be_bytes()); // duration
  p.extend_from_slice(&[0u8; 8 + 2 + 2 + 2 + 2 + 36]);
  p.extend_from_slice(&((width as u32) << 16).to_be_bytes());
  p.extend_from_slice(&((height as u32) << 16).to_be_bytes());
  encode_box(b"tkhd", &p)
}

fn mdhd(timescale: u32, duration: u32, language: &str) -> Vec<u8> {
  let mut p = Vec::with_capacity(24);
  p.push(0);
  p.extend_from_slice(&[0u8; 3]);
  p.extend_from_slice(&0u32.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes());
  p.extend_from_slice(&timescale.to_be_bytes());
  p.extend_from_slice(&duration.to_be_bytes());
  let mut bytes = language.as_bytes().iter().copied();
  let c0 = bytes.next().unwrap_or(0).saturating_sub(0x60) as u16;
  let c1 = bytes.next().unwrap_or(0).saturating_sub(0x60) as u16;
  let c2 = bytes.next().unwrap_or(0).saturating_sub(0x60) as u16;
  let packed = ((c0 & 0x1F) << 10) | ((c1 & 0x1F) << 5) | (c2 & 0x1F);
  p.extend_from_slice(&packed.to_be_bytes());
  p.extend_from_slice(&0u16.to_be_bytes());
  encode_box(b"mdhd", &p)
}

fn hdlr(handler_type: &[u8; 4], name: &str) -> Vec<u8> {
  let mut p = Vec::new();
  p.push(0);
  p.extend_from_slice(&[0u8; 3]);
  p.extend_from_slice(&[0u8; 4]);
  p.extend_from_slice(handler_type);
  p.extend_from_slice(&[0u8; 12]);
  p.extend_from_slice(name.as_bytes());
  p.push(0);
  encode_box(b"hdlr", &p)
}

/// A minimal valid `avcC` configuration record (≥ 4 bytes) so the PARSER-177
/// first-sample verification keeps an `avc1` track via the avcC branch instead
/// of attempting a (here impossible) bitstream salvage from the stub `mdat`.
fn avcc() -> Vec<u8> {
  let mut p = Vec::new();
  p.push(1); // configurationVersion
  p.push(66); // AVCProfileIndication (Baseline)
  p.push(0); // profile_compatibility
  p.push(30); // AVCLevelIndication
  p.push(0xFF); // 6 reserved bits + lengthSizeMinusOne = 3
  p.push(0xE1); // 3 reserved bits + numOfSequenceParameterSets = 1
  let sps: &[u8] = &[0x67, 0x42, 0x00, 0x1E];
  p.extend_from_slice(&(sps.len() as u16).to_be_bytes());
  p.extend_from_slice(sps);
  p.push(1); // numOfPictureParameterSets
  let pps: &[u8] = &[0x68, 0xCE];
  p.extend_from_slice(&(pps.len() as u16).to_be_bytes());
  p.extend_from_slice(pps);
  encode_box(b"avcC", &p)
}

fn video_sample_entry(fourcc_kind: &[u8; 4], width: u16, height: u16) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 6]); // reserved
  p.extend_from_slice(&1u16.to_be_bytes());
  p.extend_from_slice(&[0u8; 16]); // QT preamble
  p.extend_from_slice(&width.to_be_bytes());
  p.extend_from_slice(&height.to_be_bytes());
  p.extend_from_slice(&[0u8; 8 + 4 + 2 + 32]);
  p.extend_from_slice(&24u16.to_be_bytes()); // depth
  p.extend_from_slice(&0u16.to_be_bytes());
  // PARSER-177: carry an avcC for avc1/avc3 entries so the verification pass
  // keeps the track.
  if matches!(fourcc_kind, b"avc1" | b"avc3") {
    p.extend(avcc());
  }
  encode_box(fourcc_kind, &p)
}

/// Build an `esds` box carrying an MPEG-4 ES descriptor with the given
/// objectTypeIndication and AudioSpecificConfig.  Mirrors the lib's
/// `esds::build_esds_payload` so real `mp4a` fixtures survive the
/// missing-decoder-config filtering (PARSER-150).
fn esds(object_type: u8, audio_specific_config: &[u8]) -> Vec<u8> {
  const TAG_ES_DESCRIPTOR: u8 = 0x03;
  const TAG_DECODER_CONFIG: u8 = 0x04;
  const TAG_DEC_SPECIFIC_INFO: u8 = 0x05;

  let mut p = vec![0u8; 4]; // FullBox header
  let dec_specific = {
    let mut v = vec![TAG_DEC_SPECIFIC_INFO, audio_specific_config.len() as u8];
    v.extend_from_slice(audio_specific_config);
    v
  };
  let dec_config = {
    let mut v = vec![TAG_DECODER_CONFIG, (13 + dec_specific.len()) as u8, object_type, 0x15];
    v.extend_from_slice(&[0u8; 3]); // bufferSizeDB
    v.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate
    v.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate
    v.extend_from_slice(&dec_specific);
    v
  };
  let es_descriptor = {
    let mut v = vec![TAG_ES_DESCRIPTOR, (3 + dec_config.len()) as u8, 0, 0, 0];
    v.extend_from_slice(&dec_config);
    v
  };
  p.extend_from_slice(&es_descriptor);
  encode_box(b"esds", &p)
}

fn audio_sample_entry(fourcc_kind: &[u8; 4], channels: u16, sample_rate: u32, children: &[u8]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 6]);
  p.extend_from_slice(&1u16.to_be_bytes());
  p.extend_from_slice(&[0u8; 8]); // version+revision+vendor
  p.extend_from_slice(&channels.to_be_bytes());
  p.extend_from_slice(&16u16.to_be_bytes()); // sample_size
  p.extend_from_slice(&[0u8; 4]); // compression_id + packet_size
  p.extend_from_slice(&(sample_rate << 16).to_be_bytes());
  p.extend_from_slice(children);
  encode_box(fourcc_kind, &p)
}

fn stsd(entries: Vec<Vec<u8>>) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 4]);
  p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
  for e in entries {
    p.extend(e);
  }
  encode_box(b"stsd", &p)
}

fn stts(count: u32, delta: u32) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 4]);
  p.extend_from_slice(&1u32.to_be_bytes());
  p.extend_from_slice(&count.to_be_bytes());
  p.extend_from_slice(&delta.to_be_bytes());
  encode_box(b"stts", &p)
}

fn video_trak(track_id: u32, codec: &[u8; 4], lang: &str, width: u16, height: u16) -> Vec<u8> {
  let stbl_payload = {
    let mut p = stsd(vec![video_sample_entry(codec, width, height)]);
    p.extend(stts(60, 1000));
    p
  };
  let minf = encode_box(b"minf", &encode_box(b"stbl", &stbl_payload));
  let mut mdia = mdhd(48_000, 1024, lang);
  mdia.extend(hdlr(b"vide", "VideoHandler"));
  mdia.extend(minf);
  let mdia = encode_box(b"mdia", &mdia);
  let mut trak = tkhd(track_id, width, height);
  trak.extend(mdia);
  encode_box(b"trak", &trak)
}

fn audio_trak(track_id: u32, codec: &[u8; 4], lang: &str, sample_rate: u32, channels: u16) -> Vec<u8> {
  // mp4a entries carry an esds with an AAC AudioSpecificConfig, as real
  // AAC-in-MP4 files do (PARSER-150 drops mp4a tracks that lack one).
  let entry = audio_sample_entry(codec, channels, sample_rate, &esds(0x40, &[0x12, 0x10]));
  let stbl_payload = stsd(vec![entry]);
  let minf = encode_box(b"minf", &encode_box(b"stbl", &stbl_payload));
  let mut mdia = mdhd(sample_rate, 0, lang);
  mdia.extend(hdlr(b"soun", "SoundHandler"));
  mdia.extend(minf);
  let mdia = encode_box(b"mdia", &mdia);
  let mut trak = tkhd(track_id, 0, 0);
  trak.extend(mdia);
  encode_box(b"trak", &trak)
}

fn build_mp4(major: &[u8; 4], compats: &[&[u8; 4]], traks: Vec<Vec<u8>>) -> Vec<u8> {
  let mut bytes = ftyp(major, compats);
  let mvhd = mvhd(1000, 60_000, (traks.len() + 1) as u32);
  let mut moov_payload = mvhd;
  for t in traks {
    moov_payload.extend(t);
  }
  let moov = encode_box(b"moov", &moov_payload);
  bytes.extend(moov);
  bytes.extend(encode_box(b"mdat", &[0u8; 4]));
  bytes
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
  let path = dir.join(format!("bmm-mp4-{pid}-{nanos}-{seq}.{ext}"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

// =============================================================================
//   Tests
// =============================================================================

#[test]
fn parses_minimal_mp4_with_video_and_audio() {
  let video = video_trak(1, b"avc1", "eng", 1920, 1080);
  let audio = audio_trak(2, b"mp4a", "jpn", 48_000, 2);
  let bytes = build_mp4(b"mp42", &[b"isom", b"mp41"], vec![video, audio]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);

  assert_eq!(m.container.format, ContainerFormat::Mp4);
  assert!(m.container.recognized);
  assert!(m.container.supported);
  assert_eq!(m.container.properties.major_brand.as_deref(), Some("mp42"));
  assert_eq!(m.container.properties.movie_timescale, Some(1000));
  assert_eq!(m.container.properties.duration.unwrap().ns, 60_000_000_000);

  assert_eq!(m.tracks.len(), 2);
  assert_eq!(m.tracks[0].track_type, TrackType::Video);
  assert_eq!(m.tracks[1].track_type, TrackType::Audio);

  let v = m.tracks[0].properties.video.as_ref().unwrap();
  assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
  assert_eq!(v.pixel_dimensions.unwrap().height, 1080);
  let a = m.tracks[1].properties.audio.as_ref().unwrap();
  assert_eq!(a.channels, Some(2));
  assert_eq!(a.sampling_frequency, Some(48_000.0));
}

#[test]
fn quicktime_brand_recognised_separately() {
  let video = video_trak(1, b"avc1", "eng", 1920, 1080);
  let bytes = build_mp4(b"qt  ", &[], vec![video]);
  let path = write_tempfile(&bytes, "mov");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::QuickTime);
}

#[test]
fn file_missing_moov_returns_malformed() {
  // Just ftyp + mdat — no moov
  let mut bytes = ftyp(b"isom", &[]);
  bytes.extend(encode_box(b"mdat", &[0u8; 4]));
  let path = write_tempfile(&bytes, "mp4");
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Malformed { .. }));
}

#[test]
fn random_bytes_not_recognised() {
  let path = write_tempfile(&[0x42u8; 64], "bin");
  let err = parse(&path, ParseOptions::default()).unwrap_err();
  let _ = std::fs::remove_file(&path);
  assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn duration_unit_derivation_via_stts() {
  let video = video_trak(1, b"avc1", "eng", 1280, 720);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![video]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  let v = m.tracks[0].properties.video.as_ref().unwrap();
  // mdhd timescale = 48_000, stts delta = 1000 ⇒ 20.833 ms ≈ 20_833_333 ns
  assert_eq!(v.default_duration_ns, Some(20_833_333));
}

#[test]
fn language_pipeline_round_trips_jpn() {
  let audio = audio_trak(1, b"mp4a", "jpn", 48_000, 2);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![audio]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  let lang = m.tracks[0].properties.common.language.as_ref().unwrap();
  assert_eq!(lang.iso639_2, "jpn");
}

#[test]
fn empty_track_set_still_parses() {
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert!(m.tracks.is_empty());
  assert_eq!(m.container.properties.is_fragmented, Some(false));
}

#[test]
fn fragmented_mp4_sets_flag_and_index_count() {
  let video = video_trak(1, b"avc1", "eng", 320, 240);
  let mut bytes = build_mp4(b"mp42", &[b"isom"], vec![video]);
  // Append a moof with one traf
  let mut tfhd_payload = vec![0u8; 4];
  tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
  let tfhd = encode_box(b"tfhd", &tfhd_payload);
  let mut trun_payload = vec![0u8; 4];
  trun_payload.extend_from_slice(&30u32.to_be_bytes());
  let trun = encode_box(b"trun", &trun_payload);
  let mut traf = tfhd;
  traf.extend(trun);
  let traf = encode_box(b"traf", &traf);
  let moof = encode_box(b"moof", &traf);
  bytes.extend(moof);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.properties.is_fragmented, Some(true));
  assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(30));
}
