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

//! `av1C` AV1 codec configuration box — AV1 Codec ISO Media File Format
//! Binding §2.3.  Layout (4-byte fixed prefix):
//!
//! ```text
//! marker(1) + version(7)               // must be 0x81 (marker=1, version=1)
//! seq_profile(3) + seq_level_idx_0(5)
//! seq_tier_0(1) + high_bitdepth(1) + twelve_bit(1) + monochrome(1)
//!   + chroma_subsampling_x(1) + chroma_subsampling_y(1) + chroma_sample_position(2)
//! reserved(3) + initial_presentation_delay_present(1)
//!   + (initial_presentation_delay_minus_one(4) | reserved(4))
//! configOBUs[]                          // optional payload OBUs
//! ```
//!
//! PARSER-077.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{ChromaFormat, VideoCodecConfig};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  parse_with_cap(src, header, builder, u64::MAX)
}

pub fn parse_with_cap(
  src: &mut FileSource,
  header: &BoxHeader,
  builder: &mut TrackBuilder,
  payload_cap: u64,
) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, payload_cap)?;
  builder.codec_private_hex = Some(hex_encode(&payload));
  if payload.len() < 4 {
    return Ok(());
  }
  let byte0 = payload[0];
  if byte0 & 0x80 == 0 {
    // marker bit absent — the box is not a valid AV1ConfigurationBox.
    return Ok(());
  }
  let byte1 = payload[1];
  let byte2 = payload[2];
  let seq_profile = (byte1 >> 5) & 0x07;
  let high_bitdepth = (byte2 >> 6) & 0x01 != 0;
  let twelve_bit = (byte2 >> 5) & 0x01 != 0;
  let monochrome = (byte2 >> 4) & 0x01 != 0;
  let chroma_subsampling_x = (byte2 >> 3) & 0x01;
  let chroma_subsampling_y = (byte2 >> 2) & 0x01;
  let bit_depth = if twelve_bit {
    12
  } else if high_bitdepth {
    10
  } else {
    8
  };
  let chroma_format = if monochrome {
    ChromaFormat::Monochrome
  } else {
    match (chroma_subsampling_x, chroma_subsampling_y) {
      (1, 1) => ChromaFormat::Yuv420,
      (1, 0) => ChromaFormat::Yuv422,
      (0, 0) => ChromaFormat::Yuv444,
      _ => ChromaFormat::Other,
    }
  };
  let cfg = VideoCodecConfig {
    profile_idc: Some(seq_profile as u32),
    profile_name: Some(profile_name(seq_profile).to_string()),
    bit_depth_luma: Some(bit_depth as u32),
    bit_depth_chroma: Some(bit_depth as u32),
    chroma_format: Some(chroma_format),
    raw_hex: Some(hex_encode(&payload)),
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };
  builder.video_codec_config = Some(cfg);
  Ok(())
}

fn profile_name(profile: u8) -> &'static str {
  match profile {
    0 => "Main",
    1 => "High",
    2 => "Professional",
    _ => "Unknown",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn read(bytes: Vec<u8>) -> (BoxHeader, FileSource) {
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    (h, s)
  }

  #[test]
  fn decodes_profile_zero_8bit_420() {
    // marker=1, version=1 → 0x81; seq_profile=0, level=0 → 0x00;
    // high_bitdepth=0, twelve_bit=0, monochrome=0, sub=1/1, pos=0 → 0x0C
    let payload = vec![0x81, 0x00, 0x0C, 0x00];
    let bytes = encode_box(b"av1C", &payload);
    let (h, mut s) = read(bytes);
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(0));
    assert_eq!(cfg.bit_depth_luma, Some(8));
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
  }

  #[test]
  fn decodes_profile_two_12bit_444() {
    // seq_profile=2 → 0x40 in byte1.
    // byte2 layout (MSB → LSB): seq_tier_0(1) high_bitdepth(1) twelve_bit(1)
    //   monochrome(1) sub_x(1) sub_y(1) chroma_sample_position(2).
    // For high_bitdepth=1, twelve_bit=1, sub=(0,0): 0b01100000 = 0x60.
    let payload = vec![0x81, 0x40, 0x60, 0x00];
    let bytes = encode_box(b"av1C", &payload);
    let (h, mut s) = read(bytes);
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(2));
    assert_eq!(cfg.bit_depth_luma, Some(12));
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv444));
  }

  #[test]
  fn monochrome_overrides_subsampling() {
    // monochrome=1 → byte2 bit 4 set → 0x10
    let payload = vec![0x81, 0x00, 0x10, 0x00];
    let bytes = encode_box(b"av1C", &payload);
    let (h, mut s) = read(bytes);
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Monochrome));
  }

  #[test]
  fn missing_marker_bit_stores_raw_hex_only() {
    let payload = vec![0x01, 0x00, 0x00, 0x00];
    let bytes = encode_box(b"av1C", &payload);
    let (h, mut s) = read(bytes);
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    assert!(b.video_codec_config.is_none());
    assert_eq!(b.codec_private_hex.as_deref(), Some("01000000"));
  }
}
