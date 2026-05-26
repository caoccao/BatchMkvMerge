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

//! `hvcC` — HEVCConfigurationRecord (ISO/IEC 14496-15 §8.3.3.1.2).
//!
//! We decode the prefix fields identification needs:
//!
//! ```text
//! u8  configurationVersion (always 1)
//! u8  general_profile_space(2) | tier_flag(1) | profile_idc(5)
//! u32 general_profile_compatibility_flags
//! u48 general_constraint_indicator_flags
//! u8  general_level_idc
//! u16 reserved(4) | min_spatial_segmentation_idc(12)
//! u8  reserved(6) | parallelismType(2)
//! u8  reserved(6) | chromaFormat(2)
//! u8  reserved(5) | bitDepthLumaMinus8(3)
//! u8  reserved(5) | bitDepthChromaMinus8(3)
//! u16 avgFrameRate
//! u8  constantFrameRate(2) | numTemporalLayers(3) | temporalIdNested(1) | lengthSizeMinusOne(2)
//! ```

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{ChromaFormat, HevcTier, VideoCodecConfig};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

const MAX_PAYLOAD: u64 = 4 * 1024;
const HEADER_BYTES: usize = 23; // bytes before the SPS/PPS/VPS table

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, MAX_PAYLOAD)?;
  if payload.len() < HEADER_BYTES {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("hvcC payload {} bytes too small", payload.len()),
    });
  }
  let configuration_version = payload[0];
  if configuration_version != 1 {
    // Unknown config version — still emit raw bytes so the frontend can
    // surface them but don't try to decode.
    builder.codec_private_hex = Some(hex_encode(&payload));
    builder.video_codec_config = Some(VideoCodecConfig {
      raw_hex: Some(hex_encode(&payload)),
      is_elementary_stream: Some(false),
      ..VideoCodecConfig::default()
    });
    return Ok(());
  }

  let byte1 = payload[1];
  let tier_flag = (byte1 >> 5) & 0x01;
  let profile_idc = (byte1 & 0x1F) as u32;
  let level_idc = payload[12] as u32;

  // PARSER-258: per the HEVCDecoderConfigurationRecord layout (and
  // `../mkvtoolnix/src/common/hevc/hevcc.cpp:326-336`), chromaFormat is byte 16,
  // bitDepthLumaMinus8 byte 17, bitDepthChromaMinus8 byte 18.  Bytes 19-20 are
  // the avgFrameRate/reserved field and must not be read for bit depth.
  let chroma_idc = payload[16] & 0x03;
  let bd_luma = (payload[17] & 0x07) as u32 + 8;
  let bd_chroma = (payload[18] & 0x07) as u32 + 8;

  let cfg = VideoCodecConfig {
    profile_idc: Some(profile_idc),
    profile_name: Some(format_hevc_profile(profile_idc).to_string()),
    level_idc: Some(level_idc),
    level_name: Some(format_hevc_level(level_idc)),
    tier: Some(if tier_flag == 0 { HevcTier::Main } else { HevcTier::High }),
    chroma_format: Some(classify_chroma_idc(chroma_idc)),
    bit_depth_luma: Some(bd_luma),
    bit_depth_chroma: Some(bd_chroma),
    raw_hex: Some(hex_encode(&payload)),
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };
  if let Some(video) = builder.video.as_mut() {
    let color = video.color.get_or_insert_with(Default::default);
    color.bits_per_channel.get_or_insert(bd_luma);
  }
  builder.codec_private_hex = Some(hex_encode(&payload));
  builder.video_codec_config = Some(cfg);
  Ok(())
}

fn classify_chroma_idc(idc: u8) -> ChromaFormat {
  match idc {
    0 => ChromaFormat::Monochrome,
    1 => ChromaFormat::Yuv420,
    2 => ChromaFormat::Yuv422,
    3 => ChromaFormat::Yuv444,
    _ => ChromaFormat::Other,
  }
}

fn format_hevc_profile(idc: u32) -> &'static str {
  match idc {
    1 => "Main",
    2 => "Main 10",
    3 => "Main Still Picture",
    4 => "Range Extensions",
    _ => "Unknown",
  }
}

fn format_hevc_level(idc: u32) -> String {
  if idc == 0 {
    return "0".to_string();
  }
  // HEVC encodes level as level_idc = general_level_idc * 30. So 4.0 ⇒ 120.
  let value = (idc as f64) / 30.0;
  let major = value.trunc() as u32;
  let minor = ((value - major as f64) * 10.0).round() as u32;
  if minor == 0 {
    format!("{major}.0")
  } else {
    format!("{major}.{minor}")
  }
}

#[cfg(test)]
pub(crate) fn build_hvcc_payload(
  profile_idc: u8,
  tier_high: bool,
  level_idc: u8,
  chroma_idc: u8,
  bd_luma_m8: u8,
  bd_chroma_m8: u8,
) -> Vec<u8> {
  let mut p = vec![0u8; HEADER_BYTES];
  p[0] = 1; // configuration version
  p[1] = (if tier_high { 1u8 << 5 } else { 0 }) | (profile_idc & 0x1F);
  p[12] = level_idc;
  p[16] = 0xFC | (chroma_idc & 0x03); // reserved(6) | chromaFormat(2)
  p[17] = 0xF8 | (bd_luma_m8 & 0x07); // reserved(5) | bitDepthLumaMinus8(3)
  p[18] = 0xF8 | (bd_chroma_m8 & 0x07); // reserved(5) | bitDepthChromaMinus8(3)
  p[22] = 0; // num arrays
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn run(payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(b"hvcC", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  #[test]
  fn main10_at_level_5_1() {
    // Main 10 = profile 2, tier high, level 5.1 = level_idc 153
    let payload = build_hvcc_payload(2, true, 153, 1, 2, 2);
    let b = run(payload);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(2));
    assert_eq!(cfg.profile_name.as_deref(), Some("Main 10"));
    assert_eq!(cfg.tier, Some(HevcTier::High));
    assert_eq!(cfg.level_idc, Some(153));
    assert_eq!(cfg.level_name.as_deref(), Some("5.1"));
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
    assert_eq!(cfg.bit_depth_luma, Some(10));
    assert_eq!(cfg.bit_depth_chroma, Some(10));
  }

  #[test]
  fn main_tier_main_profile_level_4_0() {
    let payload = build_hvcc_payload(1, false, 120, 1, 0, 0);
    let b = run(payload);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_name.as_deref(), Some("Main"));
    assert_eq!(cfg.tier, Some(HevcTier::Main));
    assert_eq!(cfg.level_name.as_deref(), Some("4.0"));
  }

  // PARSER-258: chroma/bit-depth come from bytes 16/17/18; the avgFrameRate
  // bytes 19/20 must be ignored.  Distinct values at each position make a
  // regression to the old (18/19/20) offsets observable.
  #[test]
  fn reads_chroma_and_bit_depth_from_offsets_16_17_18() {
    let mut p = vec![0u8; HEADER_BYTES];
    p[0] = 1;
    p[1] = 2; // profile_idc 2
    p[12] = 120;
    p[16] = 0xFC | 0x02; // chromaFormat = 4:2:2
    p[17] = 0xF8 | 0x02; // bitDepthLumaMinus8 = 2 → 10-bit
    p[18] = 0xF8 | 0x04; // bitDepthChromaMinus8 = 4 → 12-bit
    p[19] = 0xFF; // avgFrameRate high byte — must be ignored
    p[20] = 0xFF; // avgFrameRate low byte — must be ignored
    let b = run(p);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv422));
    assert_eq!(cfg.bit_depth_luma, Some(10));
    assert_eq!(cfg.bit_depth_chroma, Some(12));
  }

  #[test]
  fn unknown_configuration_version_falls_back_to_raw_only() {
    let mut payload = vec![0u8; HEADER_BYTES];
    payload[0] = 7; // unknown version
    let b = run(payload);
    let cfg = b.video_codec_config.unwrap();
    assert!(cfg.profile_idc.is_none());
    assert!(cfg.raw_hex.is_some());
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"hvcC", &[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let err = parse(&mut s, &h, &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn chroma_idc_full_table() {
    assert_eq!(classify_chroma_idc(0), ChromaFormat::Monochrome);
    assert_eq!(classify_chroma_idc(1), ChromaFormat::Yuv420);
    assert_eq!(classify_chroma_idc(2), ChromaFormat::Yuv422);
    assert_eq!(classify_chroma_idc(3), ChromaFormat::Yuv444);
  }

  #[test]
  fn format_hevc_level_decimal() {
    assert_eq!(format_hevc_level(90), "3.0");
    assert_eq!(format_hevc_level(120), "4.0");
    assert_eq!(format_hevc_level(153), "5.1");
    assert_eq!(format_hevc_level(0), "0");
  }
}
