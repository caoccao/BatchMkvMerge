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

//! `dvcC` / `dvvC` — Dolby Vision configuration box.  Per Dolby's "Dolby
//! Vision Streams Within the ISO Base Media File Format" §3.2.
//!
//! Layout:
//!
//! ```text
//! u8  dv_version_major
//! u8  dv_version_minor
//! u8  reserved(5) | dv_profile(2) | rpu_present(1)  -- bit packing varies by spec rev
//! ...
//! ```
//!
//! We extract profile + level + BL/EL/RPU presence flags and store them as
//! human-readable strings in `VideoCodecConfig.profile_name` / `.level_name`.
//! The raw 24-byte payload is preserved as hex.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::VideoCodecConfig;

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

const MIN_PAYLOAD: usize = 4;
const MAX_PAYLOAD: u64 = 256;

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, MAX_PAYLOAD)?;
  if payload.len() < MIN_PAYLOAD {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("dvcC payload {} bytes too small", payload.len()),
    });
  }
  let major = payload[0];
  let minor = payload[1];
  let byte2 = payload[2];
  let byte3 = payload[3];
  // Dolby uses a 7-bit profile + 6-bit level packed across bytes 2-3.
  let dv_profile = (byte2 >> 1) & 0x7F;
  let dv_level = ((byte2 & 0x01) << 5) | ((byte3 >> 3) & 0x1F);
  let rpu_present = (byte3 >> 2) & 0x01;
  let el_present = (byte3 >> 1) & 0x01;
  let bl_present = byte3 & 0x01;

  let mut details = format!(
    "Dolby Vision v{}.{} profile {} level {}",
    major, minor, dv_profile, dv_level,
  );
  if bl_present == 1 {
    details.push_str(" BL");
  }
  if el_present == 1 {
    details.push_str("+EL");
  }
  if rpu_present == 1 {
    details.push_str("+RPU");
  }

  let cfg = builder.video_codec_config.get_or_insert_with(VideoCodecConfig::default);
  cfg.profile_name = Some(details);
  cfg.profile_idc = Some(dv_profile as u32);
  cfg.level_idc = Some(dv_level as u32);
  cfg.level_name = Some(dv_level.to_string());
  cfg.raw_hex = Some(hex_encode(&payload));
  Ok(())
}

#[cfg(test)]
pub(crate) fn build_dvcc_payload(profile: u8, level: u8, bl: bool, el: bool, rpu: bool) -> Vec<u8> {
  let mut p = vec![0u8; 24];
  p[0] = 1; // version major
  p[1] = 0; // version minor
  p[2] = ((profile & 0x7F) << 1) | ((level >> 5) & 0x01);
  p[3] = ((level & 0x1F) << 3) | ((rpu as u8) << 2) | ((el as u8) << 1) | (bl as u8);
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn run(payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(b"dvcC", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  #[test]
  fn profile_8_level_6_with_bl_el_rpu() {
    let payload = build_dvcc_payload(8, 6, true, true, true);
    let b = run(payload);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(8));
    assert_eq!(cfg.level_idc, Some(6));
    let name = cfg.profile_name.unwrap();
    assert!(name.contains("profile 8"));
    assert!(name.contains("BL+EL+RPU"));
  }

  #[test]
  fn profile_5_bl_only() {
    let payload = build_dvcc_payload(5, 3, true, false, false);
    let b = run(payload);
    let cfg = b.video_codec_config.unwrap();
    assert!(cfg.profile_name.unwrap().ends_with("BL"));
  }

  #[test]
  fn raw_hex_round_trips() {
    let payload = build_dvcc_payload(8, 6, true, false, true);
    let b = run(payload.clone());
    let raw = b.video_codec_config.unwrap().raw_hex.unwrap();
    let decoded: Vec<u8> = (0..raw.len())
      .step_by(2)
      .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
      .collect();
    assert_eq!(decoded, payload);
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"dvcC", &[0u8; 2]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let err = parse(&mut s, &h, &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
