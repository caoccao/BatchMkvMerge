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

//! HEVC SPS decoder (ITU-T H.265 §7.3.2.2.1).
//!
//! We read enough of the SPS to recover:
//! - profile_idc, tier (Main / High), level_idc
//! - chroma_format_idc
//! - pic_width / pic_height (luma)
//! - bit_depth_luma / bit_depth_chroma
//! - conformance_window cropping

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HevcTier {
  Main,
  High,
}

#[derive(Debug, Clone)]
pub struct HevcSps {
  pub profile_idc: u8,
  pub tier: HevcTier,
  pub level_idc: u8,
  pub chroma_format_idc: u8,
  pub separate_colour_plane: bool,
  pub coded_width: u32,
  pub coded_height: u32,
  pub display_width: u32,
  pub display_height: u32,
  pub bit_depth_luma: u8,
  pub bit_depth_chroma: u8,
  pub default_duration_ns: Option<u64>,
}

pub fn parse(rbsp: &[u8]) -> Result<HevcSps, ParseError> {
  if rbsp.len() < 12 {
    return Err(ParseError::Malformed {
      format: "hevc",
      offset: 0,
      reason: format!("SPS RBSP {} bytes too small", rbsp.len()),
    });
  }
  let mut reader = BitReader::from_rbsp(rbsp);
  let _sps_video_parameter_set_id = reader.read_bits(4)?;
  let sps_max_sub_layers_minus1 = reader.read_bits(3)? as u32;
  let _sps_temporal_id_nesting_flag = reader.read_bit()?;
  let (profile_idc, tier, level_idc) = parse_profile_tier_level(&mut reader, sps_max_sub_layers_minus1)?;
  let _sps_seq_parameter_set_id = reader.read_ue()?;
  let chroma_format_idc = reader.read_ue()? as u8;
  let mut separate_colour_plane = false;
  if chroma_format_idc == 3 {
    separate_colour_plane = reader.read_bit()?;
  }
  let pic_width_in_luma_samples = reader.read_ue()?;
  let pic_height_in_luma_samples = reader.read_ue()?;
  let conformance_window_flag = reader.read_bit()?;
  let (crop_left, crop_right, crop_top, crop_bottom) = if conformance_window_flag {
    (
      reader.read_ue()?,
      reader.read_ue()?,
      reader.read_ue()?,
      reader.read_ue()?,
    )
  } else {
    (0, 0, 0, 0)
  };
  let bit_depth_luma = (reader.read_ue()? + 8) as u8;
  let bit_depth_chroma = (reader.read_ue()? + 8) as u8;
  let default_duration_ns = parse_sps_tail_timing(&mut reader, sps_max_sub_layers_minus1).unwrap_or(None);

  let (sub_w, sub_h) = chroma_subsampling_factors(chroma_format_idc);
  let display_width = pic_width_in_luma_samples.saturating_sub((crop_left + crop_right) * sub_w);
  let display_height = pic_height_in_luma_samples.saturating_sub((crop_top + crop_bottom) * sub_h);

  Ok(HevcSps {
    profile_idc,
    tier,
    level_idc,
    chroma_format_idc,
    separate_colour_plane,
    coded_width: pic_width_in_luma_samples,
    coded_height: pic_height_in_luma_samples,
    display_width,
    display_height,
    bit_depth_luma,
    bit_depth_chroma,
    default_duration_ns,
  })
}

fn parse_sps_tail_timing(reader: &mut BitReader<'_>, max_sub_layers_minus1: u32) -> Result<Option<u64>, ParseError> {
  if reader.remaining_bits() < 16 {
    return Ok(None);
  }
  let _log2_max_pic_order_cnt_lsb_minus4 = reader.read_ue()?;
  let sub_layer_ordering_info_present = reader.read_bit()?;
  let start = if sub_layer_ordering_info_present {
    0
  } else {
    max_sub_layers_minus1
  };
  for _ in start..=max_sub_layers_minus1 {
    let _ = reader.read_ue()?; // sps_max_dec_pic_buffering_minus1
    let _ = reader.read_ue()?; // sps_max_num_reorder_pics
    let _ = reader.read_ue()?; // sps_max_latency_increase_plus1
  }
  let _ = reader.read_ue()?; // log2_min_luma_coding_block_size_minus3
  let _ = reader.read_ue()?; // log2_diff_max_min_luma_coding_block_size
  let _ = reader.read_ue()?; // log2_min_luma_transform_block_size_minus2
  let _ = reader.read_ue()?; // log2_diff_max_min_luma_transform_block_size
  let _ = reader.read_ue()?; // max_transform_hierarchy_depth_inter
  let _ = reader.read_ue()?; // max_transform_hierarchy_depth_intra
  let scaling_list_enabled = reader.read_bit()?;
  if scaling_list_enabled {
    let scaling_list_data_present = reader.read_bit()?;
    if scaling_list_data_present {
      return Ok(None);
    }
  }
  reader.skip_bits(2)?; // amp_enabled_flag + sample_adaptive_offset_enabled_flag
  let pcm_enabled = reader.read_bit()?;
  if pcm_enabled {
    reader.skip_bits(8)?; // pcm_sample_bit_depth_luma/chroma_minus1
    let _ = reader.read_ue()?; // log2_min_pcm_luma_coding_block_size_minus3
    let _ = reader.read_ue()?; // log2_diff_max_min_pcm_luma_coding_block_size
    reader.skip_bits(1)?; // pcm_loop_filter_disabled_flag
  }
  let num_short_term_ref_pic_sets = reader.read_ue()?;
  if num_short_term_ref_pic_sets != 0 {
    return Ok(None);
  }
  let long_term_ref_pics_present = reader.read_bit()?;
  if long_term_ref_pics_present {
    return Ok(None);
  }
  reader.skip_bits(2)?; // sps_temporal_mvp_enabled_flag + strong_intra_smoothing_enabled_flag
  let vui_parameters_present = reader.read_bit()?;
  if !vui_parameters_present {
    return Ok(None);
  }
  parse_vui_timing(reader)
}

fn parse_vui_timing(reader: &mut BitReader<'_>) -> Result<Option<u64>, ParseError> {
  let aspect_ratio_info_present = reader.read_bit()?;
  if aspect_ratio_info_present {
    let aspect_ratio_idc = reader.read_bits(8)?;
    if aspect_ratio_idc == 255 {
      reader.skip_bits(32)?;
    }
  }
  let overscan_info_present = reader.read_bit()?;
  if overscan_info_present {
    reader.skip_bits(1)?;
  }
  let video_signal_type_present = reader.read_bit()?;
  if video_signal_type_present {
    reader.skip_bits(4)?;
    let colour_description_present = reader.read_bit()?;
    if colour_description_present {
      reader.skip_bits(24)?;
    }
  }
  let chroma_loc_info_present = reader.read_bit()?;
  if chroma_loc_info_present {
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
  }
  reader.skip_bits(3)?; // neutral_chroma, field_seq, frame_field_info
  let default_display_window = reader.read_bit()?;
  if default_display_window {
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
  }
  let timing_info_present = reader.read_bit()?;
  if !timing_info_present {
    return Ok(None);
  }
  let num_units_in_tick = reader.read_bits(32)?;
  let time_scale = reader.read_bits(32)?;
  if reader.read_bit()? {
    let _ = reader.read_ue()?;
  }
  if num_units_in_tick == 0 || time_scale == 0 {
    return Ok(None);
  }
  Ok(Some(
    (num_units_in_tick as u128 * 1_000_000_000u128 / time_scale as u128) as u64,
  ))
}

fn parse_profile_tier_level(
  reader: &mut BitReader<'_>,
  max_sub_layers_minus1: u32,
) -> Result<(u8, HevcTier, u8), ParseError> {
  // profile_space (2) | tier_flag (1) | profile_idc (5)
  let _profile_space = reader.read_bits(2)?;
  let tier_flag = reader.read_bit()?;
  let profile_idc = reader.read_bits(5)? as u8;
  // profile_compatibility_flag[0..32] = 32 bits
  reader.skip_bits(32)?;
  // progressive_source_flag, interlaced_source_flag, non_packed_constraint_flag,
  // frame_only_constraint_flag = 4 bits + reserved 43 bits + general_inbld_flag = 1 bit
  // (Together 48 bits for the constraint indicator block.)
  reader.skip_bits(48)?;
  let level_idc = reader.read_bits(8)? as u8;

  // sub-layer profile/level structures
  let mut sub_layer_profile = vec![false; max_sub_layers_minus1 as usize];
  let mut sub_layer_level = vec![false; max_sub_layers_minus1 as usize];
  for i in 0..max_sub_layers_minus1 {
    sub_layer_profile[i as usize] = reader.read_bit()?;
    sub_layer_level[i as usize] = reader.read_bit()?;
  }
  if max_sub_layers_minus1 > 0 {
    for _ in max_sub_layers_minus1..8 {
      reader.skip_bits(2)?; // reserved_zero_2bits
    }
  }
  for i in 0..max_sub_layers_minus1 {
    if sub_layer_profile[i as usize] {
      reader.skip_bits(2 + 1 + 5 + 32 + 48)?; // same shape as top
    }
    if sub_layer_level[i as usize] {
      reader.skip_bits(8)?;
    }
  }
  let tier = if tier_flag { HevcTier::High } else { HevcTier::Main };
  Ok((profile_idc, tier, level_idc))
}

fn chroma_subsampling_factors(chroma_format_idc: u8) -> (u32, u32) {
  match chroma_format_idc {
    1 => (2, 2), // 4:2:0
    2 => (2, 1), // 4:2:2
    3 => (1, 1), // 4:4:4
    _ => (1, 1), // monochrome
  }
}

pub fn format_profile(idc: u8) -> &'static str {
  match idc {
    1 => "Main",
    2 => "Main 10",
    3 => "Main Still Picture",
    4 => "Range Extensions",
    _ => "Unknown",
  }
}

/// HEVC encodes level as `level_idc / 30` (decimal).  For example `120` is
/// "4.0" and `153` is "5.1".
pub fn format_level(idc: u8) -> String {
  if idc == 0 {
    return "0".to_string();
  }
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
mod tests {
  use super::*;

  /// Encode a synthetic Main-tier 1920x1080 10-bit SPS RBSP.
  fn build_main10_1080p_sps() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 4); // sps_vps_id
    w.write_bits(0, 3); // sps_max_sub_layers_minus1
    w.write_bit(true); // sps_temporal_id_nesting_flag
    // profile_tier_level
    w.write_bits(0, 2); // profile_space
    w.write_bit(false); // tier_flag = main
    w.write_bits(2, 5); // profile_idc = Main 10
    w.write_bits(0, 32); // profile_compatibility_flag
    w.write_bits(0, 48); // constraint indicator + reserved
    w.write_bits(120, 8); // level_idc = 4.0
    // no sub-layers (max_sub_layers_minus1 = 0) so nothing else.
    w.write_ue(0); // sps_seq_parameter_set_id
    w.write_ue(1); // chroma_format_idc (4:2:0)
    w.write_ue(1920); // pic_width_in_luma_samples
    w.write_ue(1080); // pic_height_in_luma_samples
    w.write_bit(false); // conformance_window_flag
    w.write_ue(2); // bit_depth_luma_minus8 → 10-bit
    w.write_ue(2); // bit_depth_chroma_minus8 → 10-bit
    w.into_bytes()
  }

  fn write_simple_tail_with_optional_timing(w: &mut BitWriter, include_timing: bool) {
    w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4
    w.write_bit(false); // sps_sub_layer_ordering_info_present_flag
    w.write_ue(0); // sps_max_dec_pic_buffering_minus1
    w.write_ue(0); // sps_max_num_reorder_pics
    w.write_ue(0); // sps_max_latency_increase_plus1
    w.write_ue(0); // log2_min_luma_coding_block_size_minus3
    w.write_ue(0); // log2_diff_max_min_luma_coding_block_size
    w.write_ue(0); // log2_min_luma_transform_block_size_minus2
    w.write_ue(0); // log2_diff_max_min_luma_transform_block_size
    w.write_ue(0); // max_transform_hierarchy_depth_inter
    w.write_ue(0); // max_transform_hierarchy_depth_intra
    w.write_bit(false); // scaling_list_enabled_flag
    w.write_bit(false); // amp_enabled_flag
    w.write_bit(false); // sample_adaptive_offset_enabled_flag
    w.write_bit(false); // pcm_enabled_flag
    w.write_ue(0); // num_short_term_ref_pic_sets
    w.write_bit(false); // long_term_ref_pics_present_flag
    w.write_bit(false); // sps_temporal_mvp_enabled_flag
    w.write_bit(false); // strong_intra_smoothing_enabled_flag
    w.write_bit(include_timing); // vui_parameters_present_flag
    if include_timing {
      w.write_bit(false); // aspect_ratio_info_present_flag
      w.write_bit(false); // overscan_info_present_flag
      w.write_bit(false); // video_signal_type_present_flag
      w.write_bit(false); // chroma_loc_info_present_flag
      w.write_bit(false); // neutral_chroma_indication_flag
      w.write_bit(false); // field_seq_flag
      w.write_bit(false); // frame_field_info_present_flag
      w.write_bit(false); // default_display_window_flag
      w.write_bit(true); // vui_timing_info_present_flag
      w.write_bits(1, 32); // vui_num_units_in_tick
      w.write_bits(30, 32); // vui_time_scale
      w.write_bit(false); // vui_poc_proportional_to_timing_flag
    }
  }

  fn build_main10_1080p_sps_with_timing() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 4);
    w.write_bits(0, 3);
    w.write_bit(true);
    w.write_bits(0, 2);
    w.write_bit(false);
    w.write_bits(2, 5);
    w.write_bits(0, 32);
    w.write_bits(0, 48);
    w.write_bits(120, 8);
    w.write_ue(0);
    w.write_ue(1);
    w.write_ue(1920);
    w.write_ue(1080);
    w.write_bit(false);
    w.write_ue(2);
    w.write_ue(2);
    write_simple_tail_with_optional_timing(&mut w, true);
    w.into_bytes()
  }

  struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
  }
  impl BitWriter {
    fn new() -> Self {
      Self {
        buf: Vec::new(),
        bit_index: 0,
      }
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

  #[test]
  fn parses_main10_1080p_sps() {
    let rbsp = build_main10_1080p_sps();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.profile_idc, 2);
    assert_eq!(sps.tier, HevcTier::Main);
    assert_eq!(sps.level_idc, 120);
    assert_eq!(sps.coded_width, 1920);
    assert_eq!(sps.coded_height, 1080);
    assert_eq!(sps.bit_depth_luma, 10);
    assert_eq!(sps.bit_depth_chroma, 10);
    assert_eq!(sps.chroma_format_idc, 1);
    assert_eq!(sps.default_duration_ns, None);
  }

  #[test]
  fn parses_vui_timing_default_duration() {
    let rbsp = build_main10_1080p_sps_with_timing();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.default_duration_ns, Some(33_333_333));
  }

  #[test]
  fn rejects_truncated() {
    assert!(matches!(parse(&[0u8; 4]), Err(ParseError::Malformed { .. })));
  }

  #[test]
  fn format_level_pretty_prints() {
    assert_eq!(format_level(120), "4.0");
    assert_eq!(format_level(153), "5.1");
    assert_eq!(format_level(0), "0");
  }

  #[test]
  fn format_profile_table() {
    assert_eq!(format_profile(1), "Main");
    assert_eq!(format_profile(2), "Main 10");
    assert_eq!(format_profile(3), "Main Still Picture");
    assert_eq!(format_profile(4), "Range Extensions");
    assert_eq!(format_profile(99), "Unknown");
  }

  #[test]
  fn chroma_factors_match_h265_table_6_1() {
    assert_eq!(chroma_subsampling_factors(1), (2, 2));
    assert_eq!(chroma_subsampling_factors(2), (2, 1));
    assert_eq!(chroma_subsampling_factors(3), (1, 1));
    assert_eq!(chroma_subsampling_factors(0), (1, 1));
  }
}
