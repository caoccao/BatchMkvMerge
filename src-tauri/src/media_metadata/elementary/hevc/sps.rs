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
//! - VUI sample aspect ratio, timing, and bitstream-restriction fields

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
  /// Cropped luma dimensions (mkvtoolnix's `get_width()` / `get_height()`).
  pub display_width: u32,
  pub display_height: u32,
  pub bit_depth_luma: u8,
  pub bit_depth_chroma: u8,
  /// Sample aspect ratio from the VUI (`0/0` when none was signalled).
  pub par_num: u32,
  pub par_den: u32,
  pub default_duration_ns: Option<u64>,
  pub min_spatial_segmentation_idc: u16,
  pub parallelism_type: u8,
  // ----- profile_tier_level fields the HEVCDecoderConfigurationRecord needs
  // (PARSER-255).  Ported from `profile_tier_copy`,
  // `../mkvtoolnix/src/common/hevc/util.cpp:62-103`.
  pub profile_space: u8,
  pub profile_compatibility_flag: u32,
  pub progressive_source_flag: bool,
  pub interlaced_source_flag: bool,
  pub non_packed_constraint_flag: bool,
  pub frame_only_constraint_flag: bool,
  pub max_sub_layers_minus1: u32,
  pub temporal_id_nesting_flag: bool,
}

impl HevcSps {
  /// Apply the VUI sample-aspect-ratio to the cropped luma dimensions, a port
  /// of `es_parser_c::get_display_dimensions` (PAR ≥ 1 stretches width, PAR < 1
  /// stretches height).  Returns the cropped dimensions unchanged when no
  /// usable PAR was found (PARSER-240).
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
/// `../mkvtoolnix/src/common/hevc/util.cpp`).  Index 0 (unspecified) and any
/// out-of-range `aspect_ratio_idc` yield no PAR.
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
const EXTENDED_SAR: u64 = 255;

/// Decoded SPS-tail fields the parser needs: sample aspect ratio and timing.
#[derive(Debug, Default, Clone, Copy)]
struct HevcTailInfo {
  par_num: u32,
  par_den: u32,
  default_duration_ns: Option<u64>,
  min_spatial_segmentation_idc: u16,
  parallelism_type: u8,
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
  let sps_temporal_id_nesting_flag = reader.read_bit()?;
  let ptl = parse_profile_tier_level(&mut reader, sps_max_sub_layers_minus1)?;
  let profile_idc = ptl.profile_idc;
  let tier = if ptl.tier_flag { HevcTier::High } else { HevcTier::Main };
  let level_idc = ptl.level_idc;
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
  // PARSER-239: consume scaling_list_data / short_term_ref_pic_set /
  // long_term_ref_pics so the VUI (and its timing + PAR) is reached on ordinary
  // streams.  Short reads in the tail make the SPS invalid, matching
  // mkvtoolnix's single exception boundary around the full SPS parse.
  let tail = parse_sps_tail(&mut reader, sps_max_sub_layers_minus1)?;

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
    par_num: tail.par_num,
    par_den: tail.par_den,
    default_duration_ns: tail.default_duration_ns,
    min_spatial_segmentation_idc: tail.min_spatial_segmentation_idc,
    parallelism_type: tail.parallelism_type,
    profile_space: ptl.profile_space,
    profile_compatibility_flag: ptl.profile_compatibility_flag,
    progressive_source_flag: ptl.progressive_source_flag,
    interlaced_source_flag: ptl.interlaced_source_flag,
    non_packed_constraint_flag: ptl.non_packed_constraint_flag,
    frame_only_constraint_flag: ptl.frame_only_constraint_flag,
    max_sub_layers_minus1: sps_max_sub_layers_minus1,
    temporal_id_nesting_flag: sps_temporal_id_nesting_flag,
  })
}

/// Walk the SPS body from `log2_max_pic_order_cnt_lsb_minus4` through to the
/// VUI, consuming the scaling-list, short-term reference-picture-set, and
/// long-term reference structures along the way
/// (`../mkvtoolnix/src/common/hevc/util.cpp:642-695`).
fn parse_sps_tail(reader: &mut BitReader<'_>, max_sub_layers_minus1: u32) -> Result<HevcTailInfo, ParseError> {
  let log2_max_pic_order_cnt_lsb = reader.read_ue()? + 4; // ...minus4 + 4
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
  if reader.read_bit()? {
    // scaling_list_enabled_flag
    if reader.read_bit()? {
      // sps_scaling_list_data_present_flag
      parse_scaling_list_data(reader)?;
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
  if num_short_term_ref_pic_sets > 64 {
    return Err(malformed("num_short_term_ref_pic_sets out of range"));
  }
  // `num_pics` per set is needed by the inter-prediction path of later sets.
  let mut num_pics_per_set = [0u32; 65];
  for idx in 0..num_short_term_ref_pic_sets {
    parse_short_term_ref_pic_set(reader, &mut num_pics_per_set, idx, num_short_term_ref_pic_sets)?;
  }
  let long_term_ref_pics_present = reader.read_bit()?;
  if long_term_ref_pics_present {
    let num_long_term_ref_pic_sets = reader.read_ue()?;
    for _ in 0..num_long_term_ref_pic_sets {
      reader.skip_bits(log2_max_pic_order_cnt_lsb as u64)?; // lt_ref_pic_poc_lsb_sps
      reader.skip_bits(1)?; // used_by_curr_pic_lt_sps_flag
    }
  }
  reader.skip_bits(2)?; // sps_temporal_mvp_enabled_flag + strong_intra_smoothing_enabled_flag
  let vui_parameters_present = reader.read_bit()?;
  if !vui_parameters_present {
    return Ok(HevcTailInfo::default());
  }
  parse_vui(reader, max_sub_layers_minus1)
}

/// Port of `scaling_list_data_copy` (`util.cpp:188-209`).
fn parse_scaling_list_data(reader: &mut BitReader<'_>) -> Result<(), ParseError> {
  for size_id in 0u32..4 {
    let matrix_count = if size_id == 3 { 2 } else { 6 };
    for _ in 0..matrix_count {
      // scaling_list_pred_mode_flag
      if !reader.read_bit()? {
        let _ = reader.read_ue()?; // scaling_list_pred_matrix_id_delta
      } else {
        let coef_num = std::cmp::min(64u32, 1u32 << (4 + (size_id << 1)));
        if size_id > 1 {
          let _ = reader.read_se()?; // scaling_list_dc_coef_minus8
        }
        for _ in 0..coef_num {
          let _ = reader.read_se()?; // scaling_list_delta_coef
        }
      }
    }
  }
  Ok(())
}

/// Port of `short_term_ref_pic_set_copy` (`util.cpp:212-345`).  Only the bit
/// positions and per-set `num_pics` (needed by the inter-prediction path of
/// later sets) are tracked — the decoded POC values are not needed for
/// identification.
fn parse_short_term_ref_pic_set(
  reader: &mut BitReader<'_>,
  num_pics_per_set: &mut [u32; 65],
  idx_rps: u32,
  num_short_term_ref_pic_sets: u32,
) -> Result<(), ParseError> {
  let inter_rps_pred_flag = if idx_rps > 0 { reader.read_bit()? } else { false };

  if inter_rps_pred_flag {
    let mut code = 0u32;
    if idx_rps == num_short_term_ref_pic_sets {
      code = reader.read_ue()?; // delta_idx_minus1
    }
    let ref_idx = idx_rps as i64 - 1 - code as i64;
    if ref_idx < 0 || ref_idx >= 64 {
      return Err(malformed("short_term_ref_pic_set ref_idx out of range"));
    }
    let _delta_rps_sign = reader.read_bit()?;
    let _abs_delta_rps_minus1 = reader.read_ue()?;
    let ref_num_pics = num_pics_per_set[ref_idx as usize];
    let mut num_pics = 0u32;
    for _ in 0..=ref_num_pics {
      let used_by_curr_pic = reader.read_bit()?;
      let mut use_delta = false;
      if !used_by_curr_pic {
        use_delta = reader.read_bit()?;
      }
      if used_by_curr_pic || use_delta {
        num_pics += 1;
      }
    }
    num_pics_per_set[idx_rps as usize] = num_pics;
  } else {
    let num_negative_pics = reader.read_ue()?;
    let num_positive_pics = reader.read_ue()?;
    if num_negative_pics > 16 || num_positive_pics > 16 || num_negative_pics + num_positive_pics > 16 {
      return Err(malformed("short_term_ref_pic_set picture count out of range"));
    }
    for _ in 0..num_negative_pics {
      let _ = reader.read_ue()?; // delta_poc_s0_minus1
      reader.skip_bits(1)?; // used_by_curr_pic_s0_flag
    }
    for _ in 0..num_positive_pics {
      let _ = reader.read_ue()?; // delta_poc_s1_minus1
      reader.skip_bits(1)?; // used_by_curr_pic_s1_flag
    }
    num_pics_per_set[idx_rps as usize] = num_negative_pics + num_positive_pics;
  }
  Ok(())
}

/// Read `n` bits without advancing the cursor (mkvtoolnix's `r.peek_bits`).
fn peek_bits(reader: &mut BitReader<'_>, n: u32) -> Result<u64, ParseError> {
  let pos = reader.position_bits();
  let value = reader.read_bits(n)?;
  reader.set_bit_position(pos);
  Ok(value)
}

fn malformed(reason: &'static str) -> ParseError {
  ParseError::Malformed {
    format: "hevc",
    offset: 0,
    reason: reason.to_string(),
  }
}

/// Decode the VUI parameters mkvtoolnix consumes, capturing the sample aspect
/// ratio, frame timing, and bitstream-restriction fields
/// (`vui_parameters_copy`, `util.cpp:346-436`).
fn parse_vui(reader: &mut BitReader<'_>, max_sub_layers_minus1: u32) -> Result<HevcTailInfo, ParseError> {
  let mut info = HevcTailInfo::default();
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
      reader.skip_bits(24)?;
    }
  }
  // chroma_loc_info_present_flag
  if reader.read_bit()? {
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
  }
  reader.skip_bits(3)?; // neutral_chroma + field_seq + frame_field_info
  // PARSER-256: mkvtoolnix special-cases a known-invalid default-display-window
  // bit pattern (`r.get_remaining_bits() >= 68 && r.peek_bits(21) == 0x100000`)
  // by signalling default_display_window_flag = 0 WITHOUT consuming the flag bit
  // or four bogus offsets, so it can still reach the timing info on broken
  // streams (`vui_parameters_copy`, util.cpp:386-393).
  let bogus_ddw = reader.remaining_bits() >= 68 && peek_bits(reader, 21)? == 0x100000;
  if !bogus_ddw && reader.read_bit()? {
    // default_display_window_flag
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
    let _ = reader.read_ue()?;
  }
  // vui_timing_info_present_flag
  if reader.read_bit()? {
    let num_units_in_tick = reader.read_bits(32)?;
    let time_scale = reader.read_bits(32)?;
    if num_units_in_tick != 0 && time_scale != 0 {
      // `sps_info_t::default_duration()` = num_units_in_tick * 1e9 / time_scale.
      info.default_duration_ns =
        Some((num_units_in_tick as u128 * 1_000_000_000u128 / time_scale as u128) as u64);
    }
    if reader.read_bit()? {
      // vui_poc_proportional_to_timing_flag
      let _ = reader.read_ue()?; // vui_num_ticks_poc_diff_one_minus1
    }
    if reader.read_bit()? {
      // vui_hrd_parameters_present_flag
      skip_hrd_parameters(reader, true, max_sub_layers_minus1)?;
    }
  }
  if reader.read_bit()? {
    // bitstream_restriction_flag
    let tiles_fixed_structure = reader.read_bit()?;
    reader.skip_bits(2)?; // motion_vectors_over_pic_boundaries_flag + restricted_ref_pic_lists_flag
    let min_spatial = reader.read_ue()?.min(0x0fff);
    info.min_spatial_segmentation_idc = min_spatial as u16;
    // The hvcC field is a compact two-bit advisory.  When the VUI says a
    // fixed tile structure is present alongside min_spatial_segmentation_idc,
    // carry that as tile-based parallelism; otherwise keep mkvtoolnix's zero.
    if tiles_fixed_structure && min_spatial > 0 {
      info.parallelism_type = 2;
    }
    let _ = reader.read_ue()?; // max_bytes_per_pic_denom
    let _ = reader.read_ue()?; // max_bits_per_min_cu_denom
    let _ = reader.read_ue()?; // log2_max_mv_length_horizontal
    let _ = reader.read_ue()?; // log2_max_mv_length_vertical
  }
  Ok(info)
}

fn skip_hrd_parameters(
  reader: &mut BitReader<'_>,
  common_inf_present: bool,
  max_sub_layers_minus1: u32,
) -> Result<(), ParseError> {
  if max_sub_layers_minus1 > 7 {
    return Err(malformed("max_sub_layers_minus1 out of range"));
  }
  let mut nal_hrd_parameters_present = false;
  let mut vcl_hrd_parameters_present = false;
  let mut sub_pic_hrd_params_present = false;
  if common_inf_present {
    nal_hrd_parameters_present = reader.read_bit()?;
    vcl_hrd_parameters_present = reader.read_bit()?;
    if nal_hrd_parameters_present || vcl_hrd_parameters_present {
      sub_pic_hrd_params_present = reader.read_bit()?;
      if sub_pic_hrd_params_present {
        reader.skip_bits(8)?; // tick_divisor_minus2
        reader.skip_bits(5)?; // du_cpb_removal_delay_increment_length_minus1
        reader.skip_bits(1)?; // sub_pic_cpb_params_in_pic_timing_sei_flag
        reader.skip_bits(5)?; // dpb_output_delay_du_length_minus1
      }
      reader.skip_bits(4)?; // bit_rate_scale
      reader.skip_bits(4)?; // cpb_size_scale
      if sub_pic_hrd_params_present {
        reader.skip_bits(4)?; // cpb_size_du_scale
      }
      reader.skip_bits(5)?; // initial_cpb_removal_delay_length_minus1
      reader.skip_bits(5)?; // au_cpb_removal_delay_length_minus1
      reader.skip_bits(5)?; // dpb_output_delay_length_minus1
    }
  }

  for _ in 0..=max_sub_layers_minus1 {
    let fixed_pic_rate_general = reader.read_bit()?;
    let mut fixed_pic_rate_within_cvs = fixed_pic_rate_general;
    if !fixed_pic_rate_general {
      fixed_pic_rate_within_cvs = reader.read_bit()?;
    }
    let mut low_delay_hrd = false;
    if fixed_pic_rate_within_cvs {
      let _ = reader.read_ue()?; // elemental_duration_in_tc_minus1
    } else {
      low_delay_hrd = reader.read_bit()?;
    }
    let mut cpb_cnt_minus1 = 0;
    if !low_delay_hrd {
      cpb_cnt_minus1 = reader.read_ue()?;
      if cpb_cnt_minus1 > 31 {
        return Err(malformed("cpb_cnt_minus1 out of range"));
      }
    }
    if nal_hrd_parameters_present {
      skip_sub_layer_hrd_parameters(reader, cpb_cnt_minus1 + 1, sub_pic_hrd_params_present)?;
    }
    if vcl_hrd_parameters_present {
      skip_sub_layer_hrd_parameters(reader, cpb_cnt_minus1 + 1, sub_pic_hrd_params_present)?;
    }
  }
  Ok(())
}

fn skip_sub_layer_hrd_parameters(
  reader: &mut BitReader<'_>,
  cpb_count: u32,
  sub_pic_hrd_params_present: bool,
) -> Result<(), ParseError> {
  if cpb_count == 0 || cpb_count > 32 {
    return Err(malformed("cpb_count out of range"));
  }
  for _ in 0..cpb_count {
    let _ = reader.read_ue()?; // bit_rate_value_minus1
    let _ = reader.read_ue()?; // cpb_size_value_minus1
    if sub_pic_hrd_params_present {
      let _ = reader.read_ue()?; // cpb_size_du_value_minus1
      let _ = reader.read_ue()?; // bit_rate_du_value_minus1
    }
    reader.skip_bits(1)?; // cbr_flag
  }
  Ok(())
}

/// The general profile/tier/level fields the configuration record carries
/// (PARSER-255).
#[derive(Debug, Default, Clone, Copy)]
struct ProfileTierLevel {
  profile_space: u8,
  tier_flag: bool,
  profile_idc: u8,
  profile_compatibility_flag: u32,
  progressive_source_flag: bool,
  interlaced_source_flag: bool,
  non_packed_constraint_flag: bool,
  frame_only_constraint_flag: bool,
  level_idc: u8,
}

fn parse_profile_tier_level(
  reader: &mut BitReader<'_>,
  max_sub_layers_minus1: u32,
) -> Result<ProfileTierLevel, ParseError> {
  let mut ptl = ProfileTierLevel::default();
  // profile_space (2) | tier_flag (1) | profile_idc (5)
  ptl.profile_space = reader.read_bits(2)? as u8;
  ptl.tier_flag = reader.read_bit()?;
  ptl.profile_idc = reader.read_bits(5)? as u8;
  // general_profile_compatibility_flag[0..32] = 32 bits
  ptl.profile_compatibility_flag = reader.read_bits(32)? as u32;
  // general_{progressive,interlaced,non_packed,frame_only} constraint flags,
  // then 44 reserved bits (`profile_tier_copy`, util.cpp:74-78).
  ptl.progressive_source_flag = reader.read_bit()?;
  ptl.interlaced_source_flag = reader.read_bit()?;
  ptl.non_packed_constraint_flag = reader.read_bit()?;
  ptl.frame_only_constraint_flag = reader.read_bit()?;
  reader.skip_bits(44)?; // general_reserved_zero_44bits
  ptl.level_idc = reader.read_bits(8)? as u8;

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
      reader.skip_bits(2 + 1 + 5 + 32 + 4 + 44)?; // same shape as top
    }
    if sub_layer_level[i as usize] {
      reader.skip_bits(8)?;
    }
  }
  Ok(ptl)
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
    write_simple_tail_with_optional_timing(&mut w, false);
    w.into_bytes()
  }

  fn build_main10_1080p_sps_without_tail() -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 4); // sps_vps_id
    w.write_bits(0, 3); // sps_max_sub_layers_minus1
    w.write_bit(true); // sps_temporal_id_nesting_flag
    w.write_bits(0, 2); // profile_space
    w.write_bit(false); // tier_flag = main
    w.write_bits(2, 5); // profile_idc = Main 10
    w.write_bits(0, 32); // profile_compatibility_flag
    w.write_bits(0, 48); // constraint indicator + reserved
    w.write_bits(120, 8); // level_idc = 4.0
    w.write_ue(0); // sps_seq_parameter_set_id
    w.write_ue(1); // chroma_format_idc (4:2:0)
    w.write_ue(1920); // pic_width_in_luma_samples
    w.write_ue(1080); // pic_height_in_luma_samples
    w.write_bit(false); // conformance_window_flag
    w.write_ue(2); // bit_depth_luma_minus8
    w.write_ue(2); // bit_depth_chroma_minus8
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
      w.write_bit(false); // vui_hrd_parameters_present_flag
      w.write_bit(false); // bitstream_restriction_flag
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

  /// Write an SPS tail that carries scaling-list data, one short-term ref pic
  /// set, and a long-term ref set before the VUI — exercising the structures
  /// the parser must consume to reach the VUI (PARSER-239).  The VUI carries
  /// the given aspect-ratio idc / explicit PAR plus 30 fps timing.
  fn write_complex_tail(w: &mut BitWriter, aspect_idc: u8, par: Option<(u16, u16)>) {
    w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4 → lsb width 8
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
    w.write_bit(true); // scaling_list_enabled_flag
    w.write_bit(true); // sps_scaling_list_data_present_flag
    // scaling_list_data: every entry uses prediction-from-reference (mode 0).
    for size_id in 0..4 {
      let matrix_count = if size_id == 3 { 2 } else { 6 };
      for _ in 0..matrix_count {
        w.write_bit(false); // scaling_list_pred_mode_flag = 0
        w.write_ue(0); // scaling_list_pred_matrix_id_delta
      }
    }
    w.write_bit(false); // amp_enabled_flag
    w.write_bit(false); // sample_adaptive_offset_enabled_flag
    w.write_bit(false); // pcm_enabled_flag
    w.write_ue(1); // num_short_term_ref_pic_sets
    // short_term_ref_pic_set(0): explicit, 1 negative pic.
    w.write_ue(1); // num_negative_pics
    w.write_ue(0); // num_positive_pics
    w.write_ue(0); // delta_poc_s0_minus1
    w.write_bit(true); // used_by_curr_pic_s0_flag
    w.write_bit(true); // long_term_ref_pics_present_flag
    w.write_ue(1); // num_long_term_ref_pic_sets
    w.write_bits(0, 8); // lt_ref_pic_poc_lsb_sps (log2_max_pic_order_cnt_lsb = 8)
    w.write_bit(false); // used_by_curr_pic_lt_sps_flag
    w.write_bit(false); // sps_temporal_mvp_enabled_flag
    w.write_bit(false); // strong_intra_smoothing_enabled_flag
    w.write_bit(true); // vui_parameters_present_flag
    w.write_bit(true); // aspect_ratio_info_present_flag
    w.write_bits(aspect_idc as u64, 8);
    if let Some((num, den)) = par {
      w.write_bits(num as u64, 16);
      w.write_bits(den as u64, 16);
    }
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
    w.write_bit(false); // vui_hrd_parameters_present_flag
    w.write_bit(false); // bitstream_restriction_flag
  }

  fn build_main10_1080p_sps_with_structures(aspect_idc: u8, par: Option<(u16, u16)>) -> Vec<u8> {
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
    write_complex_tail(&mut w, aspect_idc, par);
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

  /// Build a Main-tier SPS with explicit profile_tier_level fields so the
  /// PARSER-255 capture path can be verified.
  fn build_sps_with_ptl(profile_space: u8, compat: u32, progressive: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 4); // sps_vps_id
    w.write_bits(0, 3); // sps_max_sub_layers_minus1
    w.write_bit(true); // sps_temporal_id_nesting_flag
    w.write_bits(profile_space as u64, 2);
    w.write_bit(false); // tier_flag
    w.write_bits(2, 5); // profile_idc
    w.write_bits(compat as u64, 32); // general_profile_compatibility_flag
    w.write_bit(progressive); // progressive_source_flag
    w.write_bit(false); // interlaced_source_flag
    w.write_bit(false); // non_packed_constraint_flag
    w.write_bit(false); // frame_only_constraint_flag
    w.write_bits(0, 44); // general_reserved_zero_44bits
    w.write_bits(120, 8); // level_idc
    w.write_ue(0); // sps_seq_parameter_set_id
    w.write_ue(1); // chroma_format_idc
    w.write_ue(1920);
    w.write_ue(1080);
    w.write_bit(false); // conformance_window_flag
    w.write_ue(2);
    w.write_ue(2);
    write_simple_tail_with_optional_timing(&mut w, false);
    w.into_bytes()
  }

  fn build_main10_1080p_sps_with_bitstream_restriction() -> Vec<u8> {
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
    write_tail_with_bitstream_restriction(&mut w, true, 0x123);
    w.into_bytes()
  }

  fn write_tail_with_bitstream_restriction(w: &mut BitWriter, tiles_fixed: bool, min_spatial: u32) {
    w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4
    w.write_bit(false); // sps_sub_layer_ordering_info_present_flag
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_bit(false); // scaling_list_enabled_flag
    w.write_bit(false); // amp_enabled_flag
    w.write_bit(false); // sample_adaptive_offset_enabled_flag
    w.write_bit(false); // pcm_enabled_flag
    w.write_ue(0); // num_short_term_ref_pic_sets
    w.write_bit(false); // long_term_ref_pics_present_flag
    w.write_bit(false); // sps_temporal_mvp_enabled_flag
    w.write_bit(false); // strong_intra_smoothing_enabled_flag
    w.write_bit(true); // vui_parameters_present_flag
    w.write_bit(false); // aspect_ratio_info_present_flag
    w.write_bit(false); // overscan_info_present_flag
    w.write_bit(false); // video_signal_type_present_flag
    w.write_bit(false); // chroma_loc_info_present_flag
    w.write_bit(false); // neutral_chroma_indication_flag
    w.write_bit(false); // field_seq_flag
    w.write_bit(false); // frame_field_info_present_flag
    w.write_bit(false); // default_display_window_flag
    w.write_bit(false); // vui_timing_info_present_flag
    w.write_bit(true); // bitstream_restriction_flag
    w.write_bit(tiles_fixed); // tiles_fixed_structure_flag
    w.write_bit(true); // motion_vectors_over_pic_boundaries_flag
    w.write_bit(false); // restricted_ref_pic_lists_flag
    w.write_ue(min_spatial); // min_spatial_segmentation_idc
    w.write_ue(0); // max_bytes_per_pic_denom
    w.write_ue(0); // max_bits_per_min_cu_denom
    w.write_ue(0); // log2_max_mv_length_horizontal
    w.write_ue(0); // log2_max_mv_length_vertical
  }

  /// Build an SPS whose VUI carries the known-invalid default-display-window
  /// pattern (`peek_bits(21) == 0x100000`) immediately followed by valid timing.
  /// mkvtoolnix's workaround must reinterpret that 1-bit as
  /// vui_timing_info_present_flag and recover the 30 fps timing (PARSER-256).
  fn build_sps_bogus_ddw() -> Vec<u8> {
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
    // tail up to the VUI (no scaling list / ref-pic sets)
    w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4
    w.write_bit(false); // sps_sub_layer_ordering_info_present_flag
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_bit(false); // scaling_list_enabled_flag
    w.write_bit(false); // amp_enabled_flag
    w.write_bit(false); // sample_adaptive_offset_enabled_flag
    w.write_bit(false); // pcm_enabled_flag
    w.write_ue(0); // num_short_term_ref_pic_sets
    w.write_bit(false); // long_term_ref_pics_present_flag
    w.write_bit(false); // sps_temporal_mvp_enabled_flag
    w.write_bit(false); // strong_intra_smoothing_enabled_flag
    w.write_bit(true); // vui_parameters_present_flag
    w.write_bit(false); // aspect_ratio_info_present_flag
    w.write_bit(false); // overscan_info_present_flag
    w.write_bit(false); // video_signal_type_present_flag
    w.write_bit(false); // chroma_loc_info_present_flag
    w.write_bit(false); // neutral_chroma_indication_flag
    w.write_bit(false); // field_seq_flag
    w.write_bit(false); // frame_field_info_present_flag
    // The next 21 bits = 0x100000: a `1` that the buggy reader saw as
    // default_display_window_flag, then the top 20 zero bits of num_units.
    w.write_bit(true); // really vui_timing_info_present_flag
    w.write_bits(0, 20); // num_units_in_tick high 20 bits
    w.write_bits(1, 12); // num_units_in_tick low 12 bits → 1
    w.write_bits(30, 32); // vui_time_scale → 30
    w.write_bit(false); // vui_poc_proportional_to_timing_flag
    w.write_bit(false); // vui_hrd_parameters_present_flag
    w.write_bit(false); // bitstream_restriction_flag
    w.write_bits(0, 8); // padding so remaining_bits at the peek point ≥ 68
    w.into_bytes()
  }

  #[test]
  fn captures_profile_tier_level_fields() {
    let rbsp = build_sps_with_ptl(1, 0xABCD_0000, true);
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.profile_space, 1);
    assert_eq!(sps.profile_compatibility_flag, 0xABCD_0000);
    assert!(sps.progressive_source_flag);
    assert!(!sps.interlaced_source_flag);
    assert!(!sps.non_packed_constraint_flag);
    assert!(!sps.frame_only_constraint_flag);
    assert_eq!(sps.max_sub_layers_minus1, 0);
    assert!(sps.temporal_id_nesting_flag);
    // profile_idc / dims still resolve with the new field capture in place.
    assert_eq!(sps.profile_idc, 2);
    assert_eq!(sps.coded_width, 1920);
  }

  #[test]
  fn bogus_default_display_window_preserves_timing() {
    let rbsp = build_sps_bogus_ddw();
    let sps = parse(&rbsp).unwrap();
    // The workaround recovers the 30 fps timing instead of degrading to none.
    assert_eq!(sps.default_duration_ns, Some(33_333_333));
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
  fn parses_bitstream_restriction_for_hvcc_fields() {
    let rbsp = build_main10_1080p_sps_with_bitstream_restriction();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.min_spatial_segmentation_idc, 0x123);
    assert_eq!(sps.parallelism_type, 2);
  }

  // ---- PARSER-239: reach the VUI past scaling-list / ref-pic-set blocks --

  #[test]
  fn reaches_vui_timing_past_scaling_list_and_ref_pic_sets() {
    let rbsp = build_main10_1080p_sps_with_structures(1, None);
    let sps = parse(&rbsp).unwrap();
    // 1 / 30 → 33.33 ms; reached only by consuming the intervening structures.
    assert_eq!(sps.default_duration_ns, Some(33_333_333));
  }

  // ---- PARSER-240: VUI sample aspect ratio → display dimensions ---------

  #[test]
  fn extracts_par_and_display_dimensions() {
    // aspect_ratio_idc 14 → 4:3 PAR → display 2560×1080.
    let rbsp = build_main10_1080p_sps_with_structures(14, None);
    let sps = parse(&rbsp).unwrap();
    assert_eq!((sps.par_num, sps.par_den), (4, 3));
    assert_eq!(sps.display_dimensions(), (2560, 1080));
    assert_eq!(sps.default_duration_ns, Some(33_333_333));
  }

  #[test]
  fn extended_par_extracts_explicit_ratio() {
    let rbsp = build_main10_1080p_sps_with_structures(255, Some((1, 2)));
    let sps = parse(&rbsp).unwrap();
    assert_eq!((sps.par_num, sps.par_den), (1, 2));
    // PAR < 1 stretches height: 1080 × 2 = 2160.
    assert_eq!(sps.display_dimensions(), (1920, 2160));
  }

  #[test]
  fn no_par_leaves_display_equal_to_cropped() {
    let rbsp = build_main10_1080p_sps_with_timing();
    let sps = parse(&rbsp).unwrap();
    assert_eq!(sps.par_num, 0);
    assert_eq!(sps.display_dimensions(), (1920, 1080));
  }

  #[test]
  fn rejects_truncated() {
    assert!(matches!(parse(&[0u8; 4]), Err(ParseError::Malformed { .. })));
  }

  #[test]
  fn rejects_truncated_tail_after_bit_depth() {
    let rbsp = build_main10_1080p_sps_without_tail();
    assert!(parse(&rbsp).is_err());
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
