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

//! PARSER-177: first-sample track verification.
//!
//! Port of mkvtoolnix's `qtmp4_demuxer_c::verify_*` family
//! (`r_qtmp4.cpp:3660-3853`).  After the `moov` walk, mkvmerge verifies every
//! track and DROPS any it cannot use; for AVC tracks missing an `avcC` it
//! salvages a configuration record from the first frames
//! (`derive_track_params_from_avc_bitstream`).  This pass runs during
//! `read_headers` (it needs the `FileSource` for bounded first-sample reads,
//! which `identify::finalise` has no access to).
//!
//! All reads are bounded (≤ 16 KiB per track) and deadline-checked — we only
//! locate and read the FIRST sample, never demux.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track::TrackType;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoCodecConfig};

use super::codec_specific::hex_encode;
use super::identify;
use super::moov::MoovBuilder;
use super::moov::hdlr::Handler;
use super::moov::trak::TrackBuilder;

/// Hard cap on any single first-sample read.  Mirrors the largest buffer the
/// C++ verification path requests (`read_first_bytes(16384)`).
const MAX_FIRST_BYTES: u64 = 16384;

/// Run the verification pass over every track in `moov`, setting
/// `builder.probe_failed = true` for tracks mkvtoolnix would reject.
pub fn verify_tracks(src: &mut FileSource, deadline: &Deadline, moov: &mut MoovBuilder) -> Result<(), ParseError> {
  for builder in &mut moov.tracks {
    deadline.check("mp4::verify")?;
    if builder.probe_failed || builder.media_invalid {
      continue;
    }
    let track_type = classify(builder);
    match track_type {
      Some(TrackType::Audio) => verify_audio(src, deadline, builder)?,
      Some(TrackType::Video) => verify_video(src, deadline, builder)?,
      Some(TrackType::Subtitles) => verify_subtitles(builder),
      // Buttons / unknown / metadata handlers are dropped later by
      // `build_track`; no first-sample gate applies.
      _ => {}
    }
  }
  Ok(())
}

/// Classify a builder's track type from its handler, mirroring the
/// `handler.classify()` step `build_track` performs.  Returns `None` for
/// handler-less / metadata tracks (which `build_track` already drops).
fn classify(builder: &TrackBuilder) -> Option<TrackType> {
  let handler_type = builder.handler_type?;
  let handler = Handler {
    handler_type,
    name: String::new(),
  };
  if handler.is_metadata_handler() {
    return None;
  }
  Some(handler.classify())
}

// ----- audio --------------------------------------------------------------

/// r_qtmp4.cpp:3687-3731.  Drop when channels / sampling frequency are
/// missing, then specialise DTS by parsing the first frame header.
fn verify_audio(src: &mut FileSource, deadline: &Deadline, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let codec = identify::effective_codec_id(builder);

  // DTS specialisation (r_qtmp4.cpp:3719-3731, 3698-3699): read the first
  // bytes and decode a DTS header; failure → drop.
  if is_dts(&codec) {
    let buf = read_first_bytes(src, deadline, builder, MAX_FIRST_BYTES)?;
    let params = buf
      .as_deref()
      .and_then(crate::media_metadata::audio::dts::first_header_params);
    match params {
      Some((channels, sample_rate, bits)) => {
        let audio = builder
          .audio
          .get_or_insert_with(crate::media_metadata::model::track_properties_audio::AudioTrackProperties::default);
        if audio.channels.unwrap_or(0) == 0 && channels != 0 {
          audio.channels = Some(channels);
        }
        if audio.sampling_frequency.unwrap_or(0.0) == 0.0 && sample_rate != 0 {
          audio.sampling_frequency = Some(sample_rate as f64);
        }
        if audio.bit_depth.is_none() && bits != 0 {
          audio.bit_depth = Some(bits);
        }
      }
      None => {
        builder.probe_failed = true;
        return Ok(());
      }
    }
  }

  // AUDIO general (r_qtmp4.cpp:3687-3690): zero channels or zero/absent
  // sampling frequency → broken header → drop.
  let channels = builder.audio.as_ref().and_then(|a| a.channels).unwrap_or(0);
  let rate = builder.audio.as_ref().and_then(|a| a.sampling_frequency).unwrap_or(0.0);
  if channels == 0 || rate == 0.0 {
    builder.probe_failed = true;
  }
  Ok(())
}

// ----- video --------------------------------------------------------------

/// r_qtmp4.cpp:3749-3832.  Drop on zero dimensions; require / derive the
/// decoder config for AVC / HEVC / MP4V.
fn verify_video(src: &mut FileSource, deadline: &Deadline, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  // VIDEO general (r_qtmp4.cpp:3754-3757): missing dimensions → drop.
  let dims = builder.video.as_ref().and_then(|v| v.pixel_dimensions);
  let (width, height) = dims.map(|d| (d.width, d.height)).unwrap_or((0, 0));
  if width == 0 || height == 0 {
    builder.probe_failed = true;
    return Ok(());
  }

  let codec = identify::effective_codec_id(builder);

  // MP4V (r_qtmp4.cpp:3818-3832): require the esds decoder config.
  if is_mp4v(&codec) {
    if builder.esds_object_type.is_none() || builder.esds_decoder_specific_len.unwrap_or(0) == 0 {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // AVC (r_qtmp4.cpp:3771-3806): keep if an avcC was parsed (video codec
  // config from avcC, priv ≥ 4); else try to derive from the bitstream.
  if is_avc(&codec) {
    if has_decoder_config(builder, 4) {
      return Ok(());
    }
    if derive_avc_from_bitstream(src, deadline, builder)? {
      return Ok(());
    }
    builder.probe_failed = true;
    return Ok(());
  }

  // HEVC (r_qtmp4.cpp:3808-3816): require an hvcC (config ≥ 23 bytes); no
  // bitstream derivation.
  if is_hevc(&codec) {
    if !has_decoder_config(builder, 23) {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // Other video codecs (AV1, VP9, …): no extra gate beyond dimensions.
  Ok(())
}

/// True when the track carries a parsed decoder configuration record whose raw
/// bytes are at least `min_len` bytes long (mkvtoolnix's `priv[0]->get_size()`
/// check).  The avcC / hvcC parsers populate `video_codec_config.raw_hex`.
fn has_decoder_config(builder: &TrackBuilder, min_len: usize) -> bool {
  builder
    .video_codec_config
    .as_ref()
    .and_then(|c| c.raw_hex.as_ref())
    .map(|hex| (hex.len() / 2) >= min_len)
    .unwrap_or(false)
}

/// r_qtmp4.cpp:3771-3794 `derive_track_params_from_avc_bitstream`: read the
/// first 10 000 bytes, split NALs, and build an avcC from the SPS/PPS.  Sets
/// the derived `video_codec_config` + `codec_private` + pixel dims and returns
/// `true` on success.
fn derive_avc_from_bitstream(
  src: &mut FileSource,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<bool, ParseError> {
  use crate::media_metadata::elementary::avc::nal::{self, NAL_UNIT_TYPE_PPS, NAL_UNIT_TYPE_SPS};
  use crate::media_metadata::elementary::avc::sps;

  let buf = match read_first_bytes(src, deadline, builder, 10_000)? {
    Some(b) => b,
    None => return Ok(false),
  };
  let units = nal::split_nal_units(&buf);
  let mut sps_unit = None;
  let mut pps_unit = None;
  let mut decoded_sps = None;
  for unit in &units {
    if unit.nal_unit_type == NAL_UNIT_TYPE_SPS && sps_unit.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if let Ok(parsed) = sps::parse(&rbsp) {
        decoded_sps = Some(parsed);
        sps_unit = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_PPS && pps_unit.is_none() {
      pps_unit = Some(*unit);
    }
  }
  let (sps_unit, sps) = match (sps_unit, decoded_sps) {
    (Some(u), Some(s)) => (u, s),
    _ => return Ok(false),
  };

  // Build a minimal avcC.  PPS is included when present (mirrors the es_parser
  // configuration record); without it we still emit a usable SPS-only record
  // (≥ 4 bytes), which mkvtoolnix accepts via the `4 <= priv[0]->get_size()`
  // check.
  let avcc = build_avcc(&sps, sps_unit, pps_unit);

  let cfg = VideoCodecConfig {
    profile_idc: Some(sps.profile_idc as u32),
    profile_name: Some(sps::format_profile(sps.profile_idc).to_string()),
    level_idc: Some(sps.level_idc as u32),
    level_name: Some(sps::format_level(sps.level_idc)),
    bit_depth_luma: Some(sps.bit_depth_luma as u32),
    bit_depth_chroma: Some(sps.bit_depth_chroma as u32),
    coded_dimensions: Some(Dimensions2D {
      width: sps.coded_width,
      height: sps.coded_height,
    }),
    raw_hex: Some(hex_encode(&avcc)),
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };
  builder.codec_private_hex = Some(hex_encode(&avcc));
  builder.video_codec_config = Some(cfg);
  // Refine pixel/display dims from the SPS (mkvtoolnix uses the derived
  // resolution when the sample entry's were unusable).
  if let Some(video) = builder.video.as_mut() {
    let display = Dimensions2D {
      width: sps.display_width,
      height: sps.display_height,
    };
    video.pixel_dimensions = Some(display);
    if video.display_dimensions.is_none() {
      video.display_dimensions = Some(display);
    }
    if let Some(cfg) = builder.video_codec_config.clone() {
      video.codec_config = Some(cfg);
    }
  }
  Ok(true)
}

/// Assemble an `avcC` configuration record from a decoded SPS and the raw
/// SPS/PPS NAL bytes.  `lengthSizeMinusOne` defaults to 3 (4-byte NAL length).
fn build_avcc(sps: &crate::media_metadata::elementary::avc::sps::AvcSps, sps_unit: crate::media_metadata::elementary::avc::nal::NalUnit<'_>, pps_unit: Option<crate::media_metadata::elementary::avc::nal::NalUnit<'_>>) -> Vec<u8> {
  let sps_bytes = nal_bytes(sps_unit);
  let mut out = Vec::new();
  out.push(1); // configurationVersion
  out.push(sps.profile_idc);
  out.push(0); // profile_compatibility
  out.push(sps.level_idc);
  out.push(0xff); // 6 reserved bits + lengthSizeMinusOne = 3
  out.push(0xe1); // 3 reserved bits + numOfSequenceParameterSets = 1
  out.extend_from_slice(&(sps_bytes.len() as u16).to_be_bytes());
  out.extend_from_slice(&sps_bytes);
  match pps_unit {
    Some(pps) => {
      let pps_bytes = nal_bytes(pps);
      out.push(1); // numOfPictureParameterSets
      out.extend_from_slice(&(pps_bytes.len() as u16).to_be_bytes());
      out.extend_from_slice(&pps_bytes);
    }
    None => {
      out.push(0); // numOfPictureParameterSets
    }
  }
  out
}

fn nal_bytes(unit: crate::media_metadata::elementary::avc::nal::NalUnit<'_>) -> Vec<u8> {
  let mut bytes = Vec::with_capacity(unit.payload.len() + 1);
  bytes.push((unit.nal_ref_idc << 5) | unit.nal_unit_type);
  bytes.extend_from_slice(unit.payload);
  bytes
}

// ----- subtitles ----------------------------------------------------------

/// r_qtmp4.cpp:3835-3853.  VobSub requires an esds decoder config ≥ 64 bytes;
/// tx3g/text always verify.  Image / unknown subtitle codecs whose private
/// data we preserved (PARSER-178) are kept — mkvtoolnix only has explicit
/// gates for S_VOBSUB and S_TX3G, and our `build_track` already emits the
/// remaining types as image subtitles.
fn verify_subtitles(builder: &mut TrackBuilder) {
  let codec = identify::effective_codec_id(builder);
  if codec == "S_VOBSUB" && builder.esds_decoder_specific_len.unwrap_or(0) < 64 {
    builder.probe_failed = true;
  }
}

// ----- bounded first-sample read ------------------------------------------

/// Port of `read_first_bytes` restricted to sample 0: seek to the first
/// sample's file offset and read `min(max, first_sample_size, available)`
/// bytes, with `max` clamped to [`MAX_FIRST_BYTES`].  Returns `None` when the
/// first sample cannot be located (no chunk offset) or the read comes up short.
fn read_first_bytes(
  src: &mut FileSource,
  deadline: &Deadline,
  builder: &TrackBuilder,
  max: u64,
) -> Result<Option<Vec<u8>>, ParseError> {
  deadline.check("mp4::read_first_bytes")?;
  let offset = match builder.first_sample_file_offset {
    Some(o) => o,
    None => return Ok(None),
  };
  let mut want = max.min(MAX_FIRST_BYTES);
  if let Some(size) = builder.first_sample_size {
    if size > 0 {
      want = want.min(size);
    }
  }
  if let Some(len) = src.length() {
    if offset >= len {
      return Ok(None);
    }
    want = want.min(len - offset);
  }
  if want == 0 {
    return Ok(None);
  }
  src.seek_to(offset)?;
  let mut buf = vec![0u8; want as usize];
  let read = src.read_at_most(&mut buf)?;
  buf.truncate(read);
  if buf.is_empty() {
    return Ok(None);
  }
  Ok(Some(buf))
}

// ----- codec-family predicates --------------------------------------------

fn is_avc(codec: &str) -> bool {
  matches!(codec, "avc1" | "avc3" | "V_MPEG4/ISO/AVC")
}

fn is_hevc(codec: &str) -> bool {
  matches!(codec, "hvc1" | "hev1" | "V_MPEGH/ISO/HEVC")
}

fn is_mp4v(codec: &str) -> bool {
  matches!(codec, "mp4v" | "V_MPEG4/ISO/ASP")
}

fn is_dts(codec: &str) -> bool {
  matches!(codec, "A_DTS" | "dtsc" | "dtsh" | "dtse" | "dts ")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
  use crate::media_metadata::model::track_properties_video::VideoTrackProperties;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn source(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn video_builder(codec: &str, width: u32, height: u32) -> TrackBuilder {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"vide");
    b.codec_id_str = Some(codec.to_string());
    b.video = Some(VideoTrackProperties {
      pixel_dimensions: Some(Dimensions2D { width, height }),
      ..Default::default()
    });
    b
  }

  fn audio_builder(codec: &str, channels: u32, rate: f64) -> TrackBuilder {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"soun");
    b.codec_id_str = Some(codec.to_string());
    b.audio = Some(AudioTrackProperties {
      channels: if channels != 0 { Some(channels) } else { None },
      sampling_frequency: if rate != 0.0 { Some(rate) } else { None },
      ..Default::default()
    });
    b
  }

  fn avcc_raw_hex() -> String {
    // Minimal avcC ≥ 4 bytes.
    hex_encode(&crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
      66,
      30,
      3,
      &[&[0u8; 4]],
      &[&[0u8; 2]],
      None,
    ))
  }

  // r_qtmp4.cpp:3771-3799: avc1 with an avcC (priv ≥ 4) is kept.
  #[test]
  fn avc_with_avcc_kept() {
    let mut b = video_builder("avc1", 1920, 1080);
    b.video_codec_config = Some(VideoCodecConfig {
      raw_hex: Some(avcc_raw_hex()),
      ..Default::default()
    });
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3801 derive_track_params_from_avc_bitstream salvages an avc1
  // that has no avcC but a decodable SPS in the first frames.
  #[test]
  fn avc_without_avcc_salvaged_from_bitstream() {
    let mut b = video_builder("avc1", 1920, 1080);
    // mdat = an Annex B SPS + PPS the AVC SPS parser can decode.
    let es = build_avc_annex_b();
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(es.len() as u64);
    let mut src = source(es);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(66));
    assert!(b.codec_private_hex.is_some());
  }

  // r_qtmp4.cpp:3804: avc1 with no avcC and junk first bytes is skipped.
  #[test]
  fn avc_without_avcc_and_junk_dropped() {
    let mut b = video_builder("avc1", 1920, 1080);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0xAAu8; 64]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3810: hev1 without an hvcC is skipped.
  #[test]
  fn hevc_without_hvcc_dropped() {
    let mut b = video_builder("hev1", 3840, 2160);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn hevc_with_hvcc_kept() {
    let mut b = video_builder("hev1", 3840, 2160);
    b.video_codec_config = Some(VideoCodecConfig {
      raw_hex: Some("00".repeat(23)),
      ..Default::default()
    });
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3754-3757: zero dimensions → drop.
  #[test]
  fn video_zero_dimensions_dropped() {
    let mut b = video_builder("av01", 0, 0);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3818-3832: mp4v without an esds decoder config → drop.
  #[test]
  fn mp4v_without_esds_decoder_config_dropped() {
    let mut b = video_builder("mp4v", 720, 480);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn mp4v_with_esds_decoder_config_kept() {
    let mut b = video_builder("mp4v", 720, 480);
    b.esds_object_type = Some(0x20);
    b.esds_decoder_specific_len = Some(10);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3719-3731: DTS with a real header in the first bytes is kept
  // and gets channels/rate; without a header it is dropped.
  #[test]
  fn dts_with_header_kept_and_specialised() {
    let mut b = audio_builder("A_DTS", 0, 0.0);
    let frame = build_dts_frame();
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(frame.len() as u64);
    let mut src = source(frame);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let a = b.audio.unwrap();
    assert!(a.channels.unwrap() > 0);
    assert!(a.sampling_frequency.unwrap() > 0.0);
  }

  #[test]
  fn dts_without_header_dropped() {
    let mut b = audio_builder("A_DTS", 0, 0.0);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0u8; 64]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3687-3690: a zero-channel audio track is dropped.
  #[test]
  fn zero_channel_audio_dropped() {
    let mut b = audio_builder("mp4a", 0, 48_000.0);
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn good_audio_kept() {
    let mut b = audio_builder("mp4a", 2, 48_000.0);
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3845-3853: S_VOBSUB needs an esds decoder config ≥ 64 bytes.
  #[test]
  fn vobsub_short_decoder_config_dropped() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"subp");
    b.codec_id_str = Some("S_VOBSUB".to_string());
    b.esds_decoder_specific_len = Some(16);
    verify_subtitles(&mut b);
    assert!(b.probe_failed);
  }

  #[test]
  fn vobsub_long_decoder_config_kept() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"subp");
    b.codec_id_str = Some("S_VOBSUB".to_string());
    b.esds_decoder_specific_len = Some(64);
    verify_subtitles(&mut b);
    assert!(!b.probe_failed);
  }

  #[test]
  fn tx3g_subtitle_kept() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"text");
    b.codec_id_str = Some("tx3g".to_string());
    verify_subtitles(&mut b);
    assert!(!b.probe_failed);
  }

  // read_first_bytes returns None when no chunk offset was recorded.
  #[test]
  fn read_first_bytes_none_without_offset() {
    let b = video_builder("avc1", 100, 100);
    let mut src = source(vec![1, 2, 3, 4]);
    assert!(read_first_bytes(&mut src, &dl(), &b, 16).unwrap().is_none());
  }

  #[test]
  fn read_first_bytes_clamps_to_sample_size_and_eof() {
    let mut b = video_builder("avc1", 100, 100);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(3);
    let mut src = source(vec![9, 8, 7, 6, 5]);
    let got = read_first_bytes(&mut src, &dl(), &b, 16).unwrap().unwrap();
    assert_eq!(got, vec![9, 8, 7]);
  }

  #[test]
  fn verify_tracks_skips_metadata_handler() {
    let mut moov = MoovBuilder::default();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"meta");
    b.codec_id_str = Some("mdir".to_string());
    moov.tracks.push(b);
    let mut src = source(vec![]);
    verify_tracks(&mut src, &dl(), &mut moov).unwrap();
    // No gate ran (metadata handler) so it stays un-flagged here.
    assert!(!moov.tracks[0].probe_failed);
  }

  #[test]
  fn family_predicates() {
    assert!(is_avc("avc1"));
    assert!(is_avc("V_MPEG4/ISO/AVC"));
    assert!(is_hevc("hev1"));
    assert!(is_mp4v("mp4v"));
    assert!(is_dts("A_DTS"));
    assert!(is_dts("dtsc"));
    assert!(!is_avc("hev1"));
  }

  // --- fixtures ---

  /// A tiny Annex B byte stream carrying a baseline 1920x1080 SPS + PPS.
  fn build_avc_annex_b() -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67]; // SPS NAL header
    bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
    bytes.extend(build_baseline_sps_tail());
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE]); // PPS NAL
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]); // AUD terminator
    bytes
  }

  fn build_baseline_sps_tail() -> Vec<u8> {
    let mut w = BitWriter::default();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(119); // pic_width_in_mbs_minus1 (1920/16-1)
    w.write_ue(67); // pic_height_in_map_units_minus1
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(4); // crop_bottom
    w.into_bytes()
  }

  #[derive(Default)]
  struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
  }
  impl BitWriter {
    fn write_bit(&mut self, b: bool) {
      if self.bit_index == 0 {
        self.buf.push(0);
      }
      if b {
        let last = self.buf.len() - 1;
        self.buf[last] |= 1 << (7 - self.bit_index);
      }
      self.bit_index = (self.bit_index + 1) % 8;
    }
    fn write_bits(&mut self, value: u64, n: u32) {
      for i in 0..n {
        self.write_bit((value >> (n - 1 - i)) & 1 != 0);
      }
    }
    fn write_ue(&mut self, value: u32) {
      let codeword = value as u64 + 1;
      let nb = 64 - codeword.leading_zeros();
      for _ in 0..(nb - 1) {
        self.write_bit(false);
      }
      self.write_bits(codeword, nb);
    }
    fn into_bytes(mut self) -> Vec<u8> {
      self.write_bit(true);
      while self.bit_index != 0 {
        self.write_bit(false);
      }
      self.buf
    }
  }

  /// Build a single decodable DTS core frame via the audio::dts test helper.
  /// amode 6 = 3 channels, sfreq idx 13 = 48000 Hz.
  fn build_dts_frame() -> Vec<u8> {
    crate::media_metadata::audio::dts::build_dts_core_frame(6, 13)
  }
}
