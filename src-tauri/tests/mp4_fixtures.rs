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

/// `stsz` with per-sample sizes (sample_size = 0).
fn stsz(sizes: &[u32]) -> Vec<u8> {
  let mut p = vec![0u8; 4];
  p.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0 → per-sample
  p.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
  for s in sizes {
    p.extend_from_slice(&s.to_be_bytes());
  }
  encode_box(b"stsz", &p)
}

/// `stsc` with explicit (first_chunk, samples_per_chunk, sample_desc) runs.
fn stsc(entries: &[(u32, u32, u32)]) -> Vec<u8> {
  let mut p = vec![0u8; 4];
  p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
  for (fc, spc, sd) in entries {
    p.extend_from_slice(&fc.to_be_bytes());
    p.extend_from_slice(&spc.to_be_bytes());
    p.extend_from_slice(&sd.to_be_bytes());
  }
  encode_box(b"stsc", &p)
}

/// `stco` with explicit 32-bit chunk offsets.
fn stco(offsets: &[u32]) -> Vec<u8> {
  let mut p = vec![0u8; 4];
  p.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
  for o in offsets {
    p.extend_from_slice(&o.to_be_bytes());
  }
  encode_box(b"stco", &p)
}

/// An `alac` magic-cookie box: 4-byte FullBox header + a `payload`-byte
/// ALACSpecificConfig.  A valid config is 24 bytes; a short payload models a
/// broken/truncated cookie that mkvtoolnix rejects (r_qtmp4.cpp:3705-3716).
fn alac_box(config: &[u8]) -> Vec<u8> {
  let mut p = vec![0u8; 4]; // version + flags
  p.extend_from_slice(config);
  encode_box(b"alac", &p)
}

/// A `dOps` (OpusSpecificBox) carrying the given channels / pre-skip / input
/// sample rate / output gain.  All multi-byte fields are stored big-endian in
/// MP4 — the parser rewrites them little-endian when building OpusHead.
fn dops_box(channels: u8, pre_skip: u16, input_rate: u32, output_gain: u16) -> Vec<u8> {
  let mut p = vec![0u8, channels]; // version 0 + output channel count
  p.extend_from_slice(&pre_skip.to_be_bytes());
  p.extend_from_slice(&input_rate.to_be_bytes());
  p.extend_from_slice(&output_gain.to_be_bytes());
  p.push(0); // channel mapping family
  encode_box(b"dOps", &p)
}

/// A `dfLa` (FLAC) box: 4-byte FullBox header + a STREAMINFO metadata block
/// chain (block header + 34-byte STREAMINFO).
fn dfla_box(sample_rate: u32, channels: u8, bits: u8) -> Vec<u8> {
  let mut info = vec![0u8; 34];
  let packed = ((sample_rate as u64) << 44) | (((channels as u64) - 1) << 41) | (((bits as u64) - 1) << 36);
  info[10..18].copy_from_slice(&packed.to_be_bytes());
  let mut p = vec![0u8; 4]; // FullBox version + flags
  p.extend_from_slice(&[0x80, 0x00, 0x00, 34]); // last STREAMINFO block header
  p.extend_from_slice(&info);
  encode_box(b"dfLa", &p)
}

/// A 24-byte ALACSpecificConfig carrying the given channels / bit depth / rate.
fn alac_config(channels: u8, bit_depth: u8, sample_rate: u32) -> Vec<u8> {
  let mut c = Vec::new();
  c.extend_from_slice(&4096u32.to_be_bytes()); // frameLength
  c.push(0); // compatibleVersion
  c.push(bit_depth);
  c.extend_from_slice(&[0, 0, 0]); // pb / mb / kb
  c.push(channels);
  c.extend_from_slice(&0u16.to_be_bytes()); // maxRun
  c.extend_from_slice(&0u32.to_be_bytes()); // maxFrameBytes
  c.extend_from_slice(&0u32.to_be_bytes()); // avgBitRate
  c.extend_from_slice(&sample_rate.to_be_bytes());
  c
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

/// Build an audio trak with an explicit sample entry + a full sample table
/// (stsz/stsc/stco) so the verification pass can run a bounded first-bytes read.
fn audio_trak_with_table(
  track_id: u32,
  codec: &[u8; 4],
  lang: &str,
  sample_rate: u32,
  channels: u16,
  entry_children: &[u8],
  sizes: &[u32],
  chunk_offset: u32,
) -> Vec<u8> {
  let entry = audio_sample_entry(codec, channels, sample_rate, entry_children);
  let mut stbl = stsd(vec![entry]);
  stbl.extend(stts(sizes.len().max(1) as u32, 1024));
  stbl.extend(stsz(sizes));
  stbl.extend(stsc(&[(1, sizes.len().max(1) as u32, 1)]));
  stbl.extend(stco(&[chunk_offset]));
  let minf = encode_box(b"minf", &encode_box(b"stbl", &stbl));
  // mdhd timescale must be non-zero (a zero timescale marks the track invalid),
  // independent of the audio sample-rate placeholder we are testing recovery of.
  let mut mdia = mdhd(48_000, 0, lang);
  mdia.extend(hdlr(b"soun", "SoundHandler"));
  mdia.extend(minf);
  let mdia = encode_box(b"mdia", &mdia);
  let mut trak = tkhd(track_id, 0, 0);
  trak.extend(mdia);
  encode_box(b"trak", &trak)
}

/// Assemble `ftyp + mdat(sample_data) + moov(trak)`.  `mdat` precedes `moov`
/// so the sample offset is deterministic (`len(ftyp) + 8`), which the caller
/// bakes into the trak's `stco`.
fn build_mp4_mdat_first(major: &[u8; 4], trak_builder: impl FnOnce(u32) -> Vec<u8>, sample_data: &[u8]) -> Vec<u8> {
  let ftyp = ftyp(major, &[b"isom"]);
  let mdat = encode_box(b"mdat", sample_data);
  let sample_offset = (ftyp.len() + 8) as u32; // 8 = mdat box header
  let trak = trak_builder(sample_offset);
  let mvhd = mvhd(1000, 60_000, 2);
  let mut moov_payload = mvhd;
  moov_payload.extend(trak);
  let moov = encode_box(b"moov", &moov_payload);
  let mut bytes = ftyp;
  bytes.extend(mdat);
  bytes.extend(moov);
  bytes
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

// PARSER-185: a valid ALAC track (cookie ≥ 28 bytes private) is kept; a
// truncated cookie is dropped even though the sample-entry channels/rate are OK.
#[test]
fn alac_with_valid_config_kept() {
  let cookie = alac_box(&alac_config(2, 16, 44_100));
  let trak = audio_trak_with_table(1, b"alac", "eng", 44_100, 2, &cookie, &[64], 0);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "m4a");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].track_type, TrackType::Audio);
}

#[test]
fn alac_with_truncated_config_dropped() {
  // Only 16 bytes of config (< 24) → cookie too small → track dropped.
  let cookie = alac_box(&[0u8; 16]);
  let trak = audio_trak_with_table(1, b"alac", "eng", 44_100, 2, &cookie, &[64], 0);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "m4a");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert!(m.tracks.is_empty());
}

// PARSER-201: the ALAC codec private is only the ALACSpecificConfig — the
// 4-byte FullBox header is stripped (24 bytes, not 28).
#[test]
fn alac_codec_private_is_config_only_without_fullbox_header() {
  let cookie = alac_box(&alac_config(2, 16, 44_100));
  let trak = audio_trak_with_table(1, b"alac", "eng", 44_100, 2, &cookie, &[64], 0);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "m4a");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  let priv_blob = m.tracks[0].codec.codec_private.as_ref().unwrap();
  assert_eq!(priv_blob.length, 24);
  // sampleRate is the last 4 bytes of the 24-byte config.
  let bytes = hex_to_bytes(&priv_blob.hex);
  assert_eq!(u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]), 44_100);
}

// PARSER-199: Opus codec private is rewritten as an OpusHead ID header — magic
// prepended, pre-skip / input-rate / output-gain converted to little-endian.
#[test]
fn opus_codec_private_is_opushead_with_le_fields() {
  let dops = dops_box(2, 312, 48_000, 0);
  let trak = audio_trak_with_table(1, b"Opus", "eng", 48_000, 2, &dops, &[64], 0);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  let priv_blob = m.tracks[0].codec.codec_private.as_ref().unwrap();
  let head = hex_to_bytes(&priv_blob.hex);
  assert_eq!(&head[0..8], b"OpusHead");
  assert_eq!(head[9], 2); // channel count
  assert_eq!(u16::from_le_bytes([head[10], head[11]]), 312); // pre-skip (LE)
  assert_eq!(u32::from_le_bytes([head[12], head[13], head[14], head[15]]), 48_000); // input rate (LE)
}

// PARSER-200: FLAC codec private is the metadata block chain — the 4-byte
// dfLa FullBox header is stripped.
#[test]
fn flac_codec_private_excludes_dfla_fullbox_header() {
  let dfla = dfla_box(44_100, 2, 16);
  let trak = audio_trak_with_table(1, b"fLaC", "eng", 44_100, 2, &dfla, &[64], 0);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  let priv_blob = m.tracks[0].codec.codec_private.as_ref().unwrap();
  // block header (4) + STREAMINFO (34) = 38 bytes, no FullBox header.
  assert_eq!(priv_blob.length, 38);
  let blob = hex_to_bytes(&priv_blob.hex);
  assert_eq!(&blob[0..4], &[0x80, 0x00, 0x00, 34]); // STREAMINFO block header first
}

// PARSER-198: a track with two sample-description entries — the LAST entry's
// dimensions win, mirroring mkvtoolnix re-running the per-entry parse and
// overwriting `dmx.stsd` for each entry.  Both entries are avc1 (so each
// carries an avcC and survives the verification pass); the second entry's
// larger dimensions must be the ones surfaced.
#[test]
fn multiple_stsd_entries_last_one_wins() {
  let entry_a = video_sample_entry(b"avc1", 640, 480);
  let entry_b = video_sample_entry(b"avc1", 1920, 1080);
  let stbl_payload = {
    let mut p = stsd(vec![entry_a, entry_b]);
    p.extend(stts(60, 1000));
    p
  };
  let minf = encode_box(b"minf", &encode_box(b"stbl", &stbl_payload));
  let mut mdia = mdhd(1000, 60_000, "eng");
  mdia.extend(hdlr(b"vide", "VideoHandler"));
  mdia.extend(minf);
  let mdia = encode_box(b"mdia", &mdia);
  let mut trak = tkhd(1, 1920, 1080);
  trak.extend(mdia);
  let trak = encode_box(b"trak", &trak);
  let bytes = build_mp4(b"mp42", &[b"isom"], vec![trak]);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  assert_eq!(m.tracks[0].codec.id, "avc1");
  // Last entry's dimensions win (1920x1080, not the first entry's 640x480).
  let v = m.tracks[0].properties.video.as_ref().unwrap();
  assert_eq!(v.pixel_dimensions.as_ref().unwrap().width, 1920);
  assert_eq!(v.pixel_dimensions.as_ref().unwrap().height, 1080);
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
  (0..hex.len())
    .step_by(2)
    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
    .collect()
}

/// A minimal MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo frame (header + zero
/// padding to the frame size).  Mirrors the lib's `build_mp3_frame` builder,
/// reproduced here because that helper is `#[cfg(test)]`-private to the lib.
fn mp3_frame_mpeg1_l3_128_44100_stereo() -> Vec<u8> {
  // AAAAAAAA AAABBCCD EEEEFFGH …  version=MPEG1(0b11), layer=III(0b01),
  // protection=1, bitrate_index for 128 kbps (9), sr_index for 44100 (0),
  // channel_mode=stereo(0b00).
  let mut header: u32 = 0xffe0_0000;
  header |= 0b11 << 19; // version
  header |= 0b01 << 17; // layer III
  header |= 1 << 16; // protection bit
  header |= 9 << 12; // bitrate index (128 kbps, MPEG-1 L3)
  header |= 0 << 10; // sample-rate index (44100)
  // channel_mode = 0 (stereo) → bits already zero.
  let head = header.to_be_bytes();
  // frame size = 144000 * 128 / 44100 = 417 bytes (no padding).
  let frame_size = 144_000 * 128 / 44_100 + 0;
  let mut bytes = Vec::with_capacity(frame_size);
  bytes.extend_from_slice(&head);
  bytes.resize(frame_size, 0);
  bytes
}

// PARSER-184 + PARSER-183: an MP3-in-MP4 track with zero channels/rate in the
// sample entry recovers its parameters from the first frame across the sample
// table (esds objectTypeIndication 0x6B → MP3).
#[test]
fn mp3_in_mp4_params_recovered_from_first_bytes() {
  let frame = mp3_frame_mpeg1_l3_128_44100_stereo();
  let sizes = vec![frame.len() as u32];
  let trak = move |chunk_offset: u32| {
    audio_trak_with_table(1, b"mp4a", "eng", 0, 0, &esds(0x6B, &[]), &sizes, chunk_offset)
  };
  let bytes = build_mp4_mdat_first(b"mp42", trak, &frame);
  let path = write_tempfile(&bytes, "mp4");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks.len(), 1);
  let a = m.tracks[0].properties.audio.as_ref().unwrap();
  assert_eq!(a.channels, Some(2));
  assert_eq!(a.sampling_frequency, Some(44_100.0));
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
