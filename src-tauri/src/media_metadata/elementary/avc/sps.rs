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

//! H.264 SPS (Sequence Parameter Set) decoder.
//!
//! Layout per ITU-T H.264 §7.3.2.1.1.  We decode the fields identification
//! needs:
//!
//! - `profile_idc`, `level_idc`.
//! - `chroma_format_idc`, `bit_depth_luma`, `bit_depth_chroma` (for the
//!   High-family profiles only).
//! - `pic_width_in_mbs`, `pic_height_in_map_units` → coded resolution.
//! - `frame_mbs_only_flag` (multiplied through for field-coded streams).
//! - `frame_cropping_offsets` → cropped display resolution.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;

#[derive(Debug, Clone)]
pub struct AvcSps {
  pub profile_idc: u8,
  /// The 8-bit constraint-set / profile-compatibility byte
  /// (`sps.profile_compat = w.copy_bits(8, r)`,
  /// `../mkvtoolnix/src/common/avc/util.cpp:235`).  Written verbatim into byte
  /// 2 of the AVCDecoderConfigurationRecord (PARSER-257).
  pub profile_compat: u8,
  pub level_idc: u8,
  pub chroma_format_idc: u8,
  pub separate_colour_plane: bool,
  pub bit_depth_luma: u8,
  pub bit_depth_chroma: u8,
  pub coded_width: u32,
  pub coded_height: u32,
  /// Cropped pixel dimensions (mkvtoolnix's `get_width()` / `get_height()`).
  pub display_width: u32,
  pub display_height: u32,
  /// Sample aspect ratio from the VUI (`0/0` when none was signalled).
  pub par_num: u32,
  pub par_den: u32,
  pub default_duration_ns: Option<u64>,
}

impl AvcSps {
  /// Apply the VUI sample-aspect-ratio to the cropped pixel dimensions, a port
  /// of `es_parser_c::get_display_dimensions`
  /// (`../mkvtoolnix/src/common/xyzvc/es_parser.cpp:571-584`): with PAR ≥ 1 the
  /// width is stretched, otherwise the height is.  Returns the cropped pixel
  /// dimensions unchanged when no usable PAR was found (PARSER-240).
  pub fn display_dimensions(&self) -> (u32, u32) {
    if self.par_num == 0 || self.par_den == 0 {
      return (self.display_width, self.display_height);
    }
    let (num, den) = (self.par_num as u64, self.par_den as u64);
    if num >= den {
      let width = (self.display_width as u64 * num + den / 2) / den;
      (width as u32, self.display_height)
    } else {
      let height = (self.display_height as u64 * den + num / 2) / num;
      (self.display_width, height as u32)
    }
  }
}

/// Predefined sample-aspect-ratio table (`s_predefined_pars`,
/// `../mkvtoolnix/src/common/avc/util.cpp:42-58`).  Index 0 (unspecified) and
/// any out-of-range `aspect_ratio_idc` yield no PAR.
const SAR_PREDEFINED: [(u32, u32); 17] = [
  (0, 0),
  (1, 1),
  (12, 11),
  (10, 11),
  (16, 11),
  (40, 33),
  (24, 11),
  (20, 11),
  (32, 11),
  (80, 33),
  (18, 11),
  (15, 11),
  (64, 33),
  (160, 99),
  (4, 3),
  (3, 2),
  (2, 1),
];
/// `EXTENDED_SAR` aspect_ratio_idc — explicit 16-bit PAR numerator/denominator.
const EXTENDED_SAR: u64 = 255;

/// Decoded VUI fields the parser needs: sample aspect ratio and frame timing.
#[derive(Debug, Default, Clone, Copy)]
struct VuiInfo {
  par_num: u32,
  par_den: u32,
  default_duration_ns: Option<u64>,
}

/// High-family profiles (per H.264 §7.3.2.1.1) that carry the extra
/// `chroma_format_idc + bit_depth` block.
const HIGH_FAMILY_PROFILES: &[u8] = &[100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn is_high_family(profile_idc: u8) -> bool {
  HIGH_FAMILY_PROFILES.contains(&profile_idc)
}

pub fn parse(rbsp: &[u8]) -> Result<AvcSps, ParseError> {
  if rbsp.len() < 3 {
    return Err(ParseError::Malformed {
      format: "avc",
      offset: 0,
      reason: format!("SPS RBSP {} bytes too small", rbsp.len()),
    });
  }
  let profile_idc = rbsp[0];
  let profile_compat = rbsp[1];
  let level_idc = rbsp[2];
  let mut reader = BitReader::from_rbsp(&rbsp[3..]);
  let _seq_parameter_set_id = reader.read_ue()?;
  let mut chroma_format_idc = 1u32;
  let mut separate_colour_plane = false;
  let mut bit_depth_luma = 8u8;
  let mut bit_depth_chroma = 8u8;
  if is_high_family(profile_idc) {
    chroma_format_idc = reader.read_ue()?;
    if chroma_format_idc == 3 {
      separate_colour_plane = reader.read_bit()?;
    }
    bit_depth_luma = (reader.read_ue()? + 8) as u8;
    bit_depth_chroma = (reader.read_ue()? + 8) as u8;
    // qpprime_y_zero_transform_bypass_flag
    let _ = reader.read_bit()?;
    let seq_scaling_matrix_present = reader.read_bit()?;
    if seq_scaling_matrix_present {
      let scaling_list_count = if chroma_format_idc == 3 { 12 } else { 8 };
      for i in 0..scaling_list_count {
        let scaling_list_present = reader.read_bit()?;
        if scaling_list_present {
          let size = if i < 6 { 16 } else { 64 };
          skip_scaling_list(&mut reader, size)?;
        }
      }
    }
  }
  let _log2_max_frame_num_minus4 = reader.read_ue()?;
  let pic_order_cnt_type = reader.read_ue()?;
  match pic_order_cnt_type {
    0 => {
      let _log2_max_pic_order_cnt_lsb_minus4 = reader.read_ue()?;
    }
    1 => {
      let _delta_pic_order_always_zero_flag = reader.read_bit()?;
      let _offset_for_non_ref_pic = reader.read_se()?;
      let _offset_for_top_to_bottom_field = reader.read_se()?;
      let n = reader.read_ue()?;
      for _ in 0..n {
        let _ = reader.read_se()?;
      }
    }
    _ => {}
  }
  let _num_ref_frames = reader.read_ue()?;
  let _gaps_in_frame_num_value_allowed_flag = reader.read_bit()?;
  let pic_width_in_mbs_minus1 = reader.read_ue()?;
  let pic_height_in_map_units_minus1 = reader.read_ue()?;
  let frame_mbs_only_flag = reader.read_bit()?;
  if !frame_mbs_only_flag {
    let _mb_adaptive_frame_field_flag = reader.read_bit()?;
  }
  let _direct_8x8_inference_flag = reader.read_bit()?;
  let frame_cropping_flag = reader.read_bit()?;
  let (crop_left, crop_right, crop_top, crop_bottom) = if frame_cropping_flag {
    (
      reader.read_ue()?,
      reader.read_ue()?,
      reader.read_ue()?,
      reader.read_ue()?,
    )
  } else {
    (0, 0, 0, 0)
  };

  let coded_width = (pic_width_in_mbs_minus1 + 1) * 16;
  let coded_height = (pic_height_in_map_units_minus1 + 1) * 16 * if frame_mbs_only_flag { 1 } else { 2 };

  let (sub_width, sub_height) = chroma_subsampling_factors(chroma_format_idc);
  let crop_x_unit = sub_width;
  let crop_y_unit = sub_height * (if frame_mbs_only_flag { 1 } else { 2 });
  let display_width = coded_width.saturating_sub((crop_left + crop_right) * crop_x_unit);
  let display_height = coded_height.saturating_sub((crop_top + crop_bottom) * crop_y_unit);
  // VUI: capture the sample aspect ratio (PARSER-240) and frame timing
  // (PARSER-238).  Short reads here make the SPS invalid, matching
  // mkvtoolnix's single exception boundary around the full SPS parse.
  let vui = if reader.read_bit()? {
    parse_vui(&mut reader)?
  } else {
    VuiInfo::default()
  };

  Ok(AvcSps {
    profile_idc,
    profile_compat,
    level_idc,
    chroma_format_idc: chroma_format_idc as u8,
    separate_colour_plane,
    bit_depth_luma,
    bit_depth_chroma,
    coded_width,
    coded_height,
    display_width,
    display_height,
    par_num: vui.par_num,
    par_den: vui.par_den,
    default_duration_ns: vui.default_duration_ns,
  })
}

/// Decode the VUI parameters mkvtoolnix consumes (`../mkvtoolnix/src/common/avc/util.cpp:316-383`).
fn parse_vui(reader: &mut BitReader<'_>) -> Result<VuiInfo, ParseError> {
  let mut info = VuiInfo::default();
  // aspect_ratio_info_present_flag
  if reader.read_bit()? {
    let aspect_ratio_idc = reader.read_bits(8)?;
    if aspect_ratio_idc == EXTENDED_SAR {
      info.par_num = reader.read_bits(16)? as u32;
      info.par_den = reader.read_bits(16)? as u32;
    } else if (aspect_ratio_idc as usize) < SAR_PREDEFINED.len() {
      let (num, den) = SAR_PREDEFINED[aspect_ratio_idc as usize];
      info.par_num = num;
      info.par_den = den;
    }
  }
  // overscan_info_present_flag
  if reader.read_bit()? {
    reader.skip_bits(1)?; // overscan_appropriate_flag
  }
  // video_signal_type_present_flag
  if reader.read_bit()? {
    reader.skip_bits(4)?; // video_format(3) + video_full_range_flag(1)
    if reader.read_bit()? {
      // colour_description_present_flag
      reader.skip_bits(24)?; // colour_primaries / transfer / matrix
    }
  }
  // chroma_loc_info_present_flag
  if reader.read_bit()? {
    let _ = reader.read_ue()?; // chroma_sample_loc_type_top_field
    let _ = reader.read_ue()?; // chroma_sample_loc_type_bottom_field
  }
  // timing_info_present_flag
  if reader.read_bit()? {
    let num_units_in_tick = reader.read_bits(32)?;
    let time_scale = reader.read_bits(32)?;
    let _fixed_frame_rate_flag = reader.read_bit()?;
    if num_units_in_tick != 0 && time_scale != 0 {
      // PARSER-238: `timing_info_t::default_duration()` is
      // `num_units_in_tick * 1e9 / time_scale` — no factor of two
      // (`../mkvtoolnix/src/common/avc/util.cpp:98-101`).
      info.default_duration_ns =
        Some(((num_units_in_tick as u128 * 1_000_000_000u128) / time_scale as u128) as u64);
    }
  }
  Ok(info)
}

fn skip_scaling_list(reader: &mut BitReader<'_>, size: u32) -> Result<(), ParseError> {
  let mut last_scale: i32 = 8;
  let mut next_scale: i32 = 8;
  for _ in 0..size {
    if next_scale != 0 {
      let delta = reader.read_se()?;
      next_scale = (last_scale + delta + 256) % 256;
    }
    if next_scale != 0 {
      last_scale = next_scale;
    }
  }
  Ok(())
}

fn chroma_subsampling_factors(chroma_format_idc: u32) -> (u32, u32) {
  // Table 6-1 — sub_width_c × sub_height_c
  match chroma_format_idc {
    1 => (2, 2), // 4:2:0
    2 => (2, 1), // 4:2:2
    3 => (1, 1), // 4:4:4
    _ => (1, 1), // monochrome
  }
}

/// Format the level-idc as a decimal string (`30 → "3.0"`, `41 → "4.1"`).
pub fn format_level(level_idc: u8) -> String {
  if level_idc == 0 {
    return "0".to_string();
  }
  format!("{}.{}", level_idc / 10, level_idc % 10)
}

/// Human-readable AVC profile name.
pub fn format_profile(profile_idc: u8) -> &'static str {
  match profile_idc {
    66 => "Baseline",
    77 => "Main",
    88 => "Extended",
    100 => "High",
    110 => "High 10",
    122 => "High 4:2:2",
    244 => "High 4:4:4 Predictive",
    44 => "CAVLC 4:4:4",
    83 => "Scalable Baseline",
    86 => "Scalable High",
    118 => "Multiview High",
    128 => "Stereo High",
    _ => "Unknown",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Build a baseline-profile SPS that says 1920×1080 progressive, level 4.0.
  fn build_baseline_1080p() -> Vec<u8> {
    // Use a minimal encoder helper instead of hand-crafting bits — easier
    // to keep correct than a literal byte string.
    let mut writer = BitWriter::new();
    writer.write_bits(0, 3); // not actually emitted — placeholder
    // Real test uses a literal: profile=baseline (66), constraints, level=40
    let mut bytes = vec![66u8, 0u8, 40u8];
    // seq_parameter_set_id = 0 ⇒ ue(0) = 1 bit (1)
    // log2_max_frame_num_minus4 = 0 ⇒ ue(0) = 1
    // pic_order_cnt_type = 0 ⇒ ue(0) = 1
    // log2_max_pic_order_cnt_lsb_minus4 = 0 ⇒ ue(0) = 1
    // num_ref_frames = 0 ⇒ ue(0) = 1
    // gaps_in_frame_num_value_allowed_flag = 0
    // pic_width_in_mbs_minus1 = 119 (1920/16-1) ⇒ ue(119)
    // pic_height_in_map_units_minus1 = 67 (1080/16-1) ⇒ ue(67)
    // frame_mbs_only_flag = 1
    // direct_8x8_inference_flag = 0
    // frame_cropping_flag = 1
    // crop_left = 0, crop_right = 0, crop_top = 0, crop_bottom = 4 (truncated 1088 → 1080)
    bytes.extend_from_slice(&encode_sps_tail_baseline(119, 67, /*crop_bottom*/ 4));
    let _ = writer; // silence
    bytes
  }

  /// Convenience: produce the bit-packed tail of a baseline SPS so tests
  /// remain readable.  This mirrors the bit stream consumed by `parse`.
  fn encode_sps_tail_baseline(
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
    crop_bottom: u32,
  ) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(pic_width_in_mbs_minus1);
    w.write_ue(pic_height_in_map_units_minus1);
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(crop_bottom);
    w.write_bit(false); // vui_parameters_present_flag
    w.into_bytes()
  }

  /// Encode a baseline SPS tail carrying a VUI with the given aspect ratio and
  /// optional timing, so PAR / display-dimension handling can be tested.
  fn encode_sps_tail_with_vui(
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
    crop_bottom: u32,
    aspect_ratio_idc: u8,
    par: Option<(u16, u16)>,
  ) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(pic_width_in_mbs_minus1);
    w.write_ue(pic_height_in_map_units_minus1);
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(crop_bottom);
    w.write_bit(true); // vui_parameters_present_flag
    w.write_bit(true); // aspect_ratio_info_present_flag
    w.write_bits(aspect_ratio_idc as u64, 8);
    if let Some((num, den)) = par {
      w.write_bits(num as u64, 16);
      w.write_bits(den as u64, 16);
    }
    w.write_bit(false); // overscan_info_present_flag
    w.write_bit(false); // video_signal_type_present_flag
    w.write_bit(false); // chroma_loc_info_present_flag
    w.write_bit(false); // timing_info_present_flag
    w.into_bytes()
  }

  fn encode_sps_tail_with_truncated_vui() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(119);
    w.write_ue(67);
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(4); // crop_bottom
    w.write_bit(true); // vui_parameters_present_flag
    w.write_bit(true); // aspect_ratio_info_present_flag
    w.write_bits(EXTENDED_SAR, 8); // par_num/par_den are intentionally absent.
    w.into_bytes()
  }

  #[derive(Default)]
  struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
  }

  impl BitWriter {
    fn new() -> Self {
      Self::default()
    }
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
        let bit = (value >> (n - 1 - i)) & 1 != 0;
        self.write_bit(bit);
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
      // Pad with a trailing 1-bit + zeros (rbsp_trailing_bits)
      self.write_bit(true);
      while self.bit_index != 0 {
        self.write_bit(false);
      }
      self.buf
    }
  }

  #[test]
  fn parses_baseline_1080p_sps() {
    let rbsp = build_baseline_1080p();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.profile_idc, 66);
    assert_eq!(sps.level_idc, 40);
    assert_eq!(sps.coded_width, 1920);
    assert_eq!(sps.coded_height, 1088);
    // crop_bottom = 4 × crop_y_unit (2 for 4:2:0) = 8 pixels off the bottom
    assert_eq!(sps.display_height, 1080);
    assert_eq!(sps.display_width, 1920);
    assert_eq!(sps.chroma_format_idc, 1);
    assert_eq!(sps.bit_depth_luma, 8);
  }

  #[test]
  fn captures_profile_compat_byte() {
    // PARSER-257: rbsp[1] is the constraint-set / profile-compatibility byte.
    let mut rbsp = vec![66u8, 0xC0, 40u8];
    rbsp.extend(encode_sps_tail_baseline(119, 67, 4));
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.profile_compat, 0xC0);
  }

  #[test]
  fn rejects_truncated_rbsp() {
    let rbsp = vec![100u8, 0u8];
    assert!(matches!(parse(&rbsp), Err(ParseError::Malformed { .. })));
  }

  #[test]
  fn rejects_truncated_vui_tail() {
    let mut rbsp = vec![66u8, 0u8, 40u8];
    rbsp.extend(encode_sps_tail_with_truncated_vui());
    assert!(matches!(parse(&rbsp), Err(ParseError::UnexpectedEof { .. })));
  }

  // ---- PARSER-240: VUI sample aspect ratio → display dimensions --------

  #[test]
  fn predefined_par_stretches_display_width() {
    // aspect_ratio_idc 14 → 4:3 PAR (s_predefined_pars[14] == {4,3}).
    let mut rbsp = vec![66u8, 0u8, 40u8];
    rbsp.extend(encode_sps_tail_with_vui(119, 67, 4, 14, None));
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.par_num, 4);
    assert_eq!(sps.par_den, 3);
    // 1920 × 4/3 = 2560, height unchanged.
    assert_eq!(sps.display_dimensions(), (2560, 1080));
  }

  #[test]
  fn extended_par_below_one_stretches_height() {
    // EXTENDED_SAR (255) with PAR 1:2 → par < 1 → height stretched.
    let mut rbsp = vec![66u8, 0u8, 40u8];
    rbsp.extend(encode_sps_tail_with_vui(119, 67, 4, 255, Some((1, 2))));
    let sps = parse(&rbsp).unwrap();
    assert_eq!((sps.par_num, sps.par_den), (1, 2));
    // height 1080 × 2/1 = 2160, width unchanged.
    assert_eq!(sps.display_dimensions(), (1920, 2160));
  }

  #[test]
  fn no_par_leaves_display_equal_to_cropped() {
    let rbsp = build_baseline_1080p();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.par_num, 0);
    assert_eq!(sps.display_dimensions(), (1920, 1080));
  }

  #[test]
  fn unity_par_does_not_change_display() {
    // aspect_ratio_idc 1 → 1:1.
    let mut rbsp = vec![66u8, 0u8, 40u8];
    rbsp.extend(encode_sps_tail_with_vui(119, 67, 4, 1, None));
    let sps = parse(&rbsp).unwrap();
    assert_eq!((sps.par_num, sps.par_den), (1, 1));
    assert_eq!(sps.display_dimensions(), (1920, 1080));
  }

  #[test]
  fn format_level_pretty_prints() {
    assert_eq!(format_level(30), "3.0");
    assert_eq!(format_level(41), "4.1");
    assert_eq!(format_level(0), "0");
  }

  #[test]
  fn format_profile_table() {
    assert_eq!(format_profile(66), "Baseline");
    assert_eq!(format_profile(100), "High");
    assert_eq!(format_profile(122), "High 4:2:2");
    assert_eq!(format_profile(244), "High 4:4:4 Predictive");
    assert_eq!(format_profile(255), "Unknown");
  }

  #[test]
  fn chroma_factors_match_h264_table_6_1() {
    assert_eq!(chroma_subsampling_factors(1), (2, 2));
    assert_eq!(chroma_subsampling_factors(2), (2, 1));
    assert_eq!(chroma_subsampling_factors(3), (1, 1));
    assert_eq!(chroma_subsampling_factors(0), (1, 1));
  }

  #[test]
  fn is_high_family_predicate_covers_documented_values() {
    for p in [100u8, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135] {
      assert!(is_high_family(p));
    }
    for p in [66u8, 77, 88, 200] {
      assert!(!is_high_family(p));
    }
  }
}
