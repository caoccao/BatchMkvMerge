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

//! DTS reader. Pure-Rust port of `mkvtoolnix/src/common/dts.cpp` +
//! `src/input/r_dts.cpp`.
//!
//! Identification mirrors mkvtoolnix:
//! - [`detect`] tries the four (byte-swap × 14→16-bit) transform combinations
//!   and confirms each by fully decoding a frame header — replacing the old
//!   "any sync word claims the file" behaviour (PARSER-010, PARSER-013).
//! - [`find_consecutive_headers`] additionally requires five back-to-back
//!   agreeing frames on the raw buffer.
//! - The core sample-frequency table marks indices 14/15 invalid
//!   (PARSER-011); the channel-arrangement table reports 7/8/8 channels for
//!   arrangements 13/14/15 (PARSER-012).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::endian::get_u32_be;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;

const SYNC_CORE: u32 = 0x7ffe8001;
const SYNC_EXSS: u32 = 0x64582025;
const SYNC_X96: u32 = 0x1d95f262;
const SYNC_XLL: u32 = 0x41a29547;
const SYNC_LBR: u32 = 0x0a801921;
const SYNC_XCH: u32 = 0x5a5a5a5a;

// Extension mask bits (`extension_mask_e`).
const EXSS_CORE: u32 = 0x010;
const EXSS_XBR: u32 = 0x020;
const EXSS_XXCH: u32 = 0x040;
const EXSS_X96: u32 = 0x080;
const EXSS_LBR: u32 = 0x100;
const EXSS_XLL: u32 = 0x200;
const EXSS_RSV1: u32 = 0x400;
const EXSS_RSV2: u32 = 0x800;

const SPEAKER_PAIR_ALL_2: u32 = 0xae66;

/// `core_samplefreqs` — Hz, or 0 (invalid) for reserved indices 0/4/5/9/10/14/15.
/// mkvtoolnix uses -1 for invalid; we use 0 so callers can `> 0`-gate it.
const CORE_SAMPLE_FREQS: [u32; 16] = [
  0, 8000, 16000, 32000, 0, 0, 11025, 22050, 44100, 0, 0, 12000, 24000, 48000, 0, 0,
];

/// `s_substream_sample_rates` — used for the exss asset `max_sample_rate`.
const SUBSTREAM_SAMPLE_RATES: [u32; 16] = [
  8000, 16000, 32000, 64000, 128000, 22050, 44100, 88200, 176400, 352800, 12000, 24000, 48000, 96000, 192000, 384000,
];

/// `s_lbr_sampling_frequencies`.
const LBR_SAMPLE_FREQS: [u32; 16] = [
  8000, 16000, 32000, 0, 0, 22050, 44100, 0, 0, 0, 12000, 24000, 48000, 0, 0, 0,
];

/// `channel_arrangements[].num_channels` (DTS core AMODE → channel count).
const CHANNEL_ARRANGEMENTS: [u32; 16] = [1, 2, 2, 2, 2, 3, 3, 4, 4, 5, 6, 6, 6, 7, 8, 8];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameType {
  Termination,
  Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LfeType {
  None,
  Lfe128,
  Lfe64,
  Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtsType {
  Normal,
  HighResolution,
  MasterAudio,
  Express,
  Es,
  X96_24,
}

#[derive(Debug, Clone, Default)]
struct Asset {
  asset_offset: usize,
  asset_size: usize,
  max_sample_rate: u32,
  num_channels_total: u32,
  one_to_one_map_channel_to_speaker: bool,
  embedded_stereo: bool,
  embedded_6ch: bool,
  coding_mode: u32,
  extension_mask: u32,
  core_size: usize,
  xbr_size: usize,
  xxch_size: usize,
  x96_size: usize,
  lbr_size: usize,
  xll_size: usize,
  core_offset: usize,
  xbr_offset: usize,
  xxch_offset: usize,
  x96_offset: usize,
  lbr_offset: usize,
  xll_offset: usize,
  lbr_sync_present: bool,
  xll_sync_present: bool,
  xll_sync_offset: usize,
}

/// Port of `mtx::dts::header_t` — only the fields used for identification.
#[derive(Debug, Clone)]
pub struct Header {
  frametype: FrameType,
  deficit_sample_count: u32,
  crc_present: bool,
  num_pcm_sample_blocks: u32,
  frame_byte_size: usize,
  audio_channels: i32,
  core_sampling_frequency: u32,
  extension_sampling_frequency: Option<u32>,
  extension_audio_descriptor: u32,
  extended_coding: bool,
  audio_sync_word_in_sub_sub: bool,
  lfe_type: LfeType,
  predictor_history_flag: bool,
  encoder_software_revision: u32,
  copy_history: u32,
  source_pcm_resolution: i32,
  source_surround_in_es: bool,
  pub dts_type: DtsType,

  has_core: bool,
  has_exss: bool,
  has_xch: bool,
  exss_offset: usize,
  exss_header_size: usize,
  exss_part_size: usize,

  static_fields_present: bool,
  mix_metadata_enabled: bool,
  reference_clock_code: u32,
  substream_frame_duration: u32,
  substream_size_bits: u32,
  num_presentations: u32,
  num_assets: u32,
  num_mixing_configurations: u32,
  num_mixing_channels: [u32; 5],
  substream_assets: Vec<Asset>,
}

impl Default for Header {
  fn default() -> Self {
    Header {
      frametype: FrameType::Normal,
      deficit_sample_count: 0,
      crc_present: false,
      num_pcm_sample_blocks: 0,
      frame_byte_size: 0,
      audio_channels: 0,
      core_sampling_frequency: 0,
      extension_sampling_frequency: None,
      extension_audio_descriptor: 0,
      extended_coding: false,
      audio_sync_word_in_sub_sub: false,
      lfe_type: LfeType::None,
      predictor_history_flag: false,
      encoder_software_revision: 0,
      copy_history: 0,
      source_pcm_resolution: 0,
      source_surround_in_es: false,
      dts_type: DtsType::Normal,
      has_core: false,
      has_exss: false,
      has_xch: false,
      exss_offset: 0,
      exss_header_size: 0,
      exss_part_size: 0,
      static_fields_present: false,
      mix_metadata_enabled: false,
      reference_clock_code: 0,
      substream_frame_duration: 0,
      substream_size_bits: 0,
      num_presentations: 1,
      num_assets: 1,
      num_mixing_configurations: 0,
      num_mixing_channels: [0; 5],
      substream_assets: Vec::new(),
    }
  }
}

impl Header {
  /// `get_core_num_audio_channels` — core channels + LFE + XCh extension.
  fn core_num_audio_channels(&self) -> u32 {
    let mut total = self.audio_channels.max(0) as u32;
    if matches!(self.lfe_type, LfeType::Lfe64 | LfeType::Lfe128) {
      total += 1;
    }
    if self.has_xch {
      total += 1;
    }
    total
  }

  /// `get_total_num_audio_channels`.
  fn total_num_audio_channels(&self) -> u32 {
    if self.has_exss && self.num_assets > 0 {
      if let Some(a) = self.substream_assets.first() {
        if a.num_channels_total > 0 {
          return a.num_channels_total;
        }
      }
    }
    self.core_num_audio_channels()
  }

  /// `get_effective_sampling_frequency`.
  fn effective_sampling_frequency(&self) -> u32 {
    match self.extension_sampling_frequency {
      Some(f) if f != 0 => f,
      _ => self.core_sampling_frequency,
    }
  }

  /// Comparable packet length in nanoseconds — used by the `operator==`
  /// port in [`Header::same_as`]. `None` when the rate is unknown.
  fn packet_length_ns(&self) -> Option<i128> {
    if self.has_core {
      let mut samples = self.num_pcm_sample_blocks as i128 * 32;
      if self.frametype == FrameType::Termination {
        samples -= samples.min(self.deficit_sample_count as i128);
      }
      if self.core_sampling_frequency == 0 {
        return None;
      }
      Some(samples * 1_000_000_000 / self.core_sampling_frequency as i128)
    } else {
      const PERIODS: [i128; 3] = [32000, 44100, 48000];
      if self.reference_clock_code < 3 {
        Some(self.substream_frame_duration as i128 * 1_000_000_000 / PERIODS[self.reference_clock_code as usize])
      } else {
        None
      }
    }
  }

  /// Port of `operator==` (used for consecutive-frame agreement).
  fn same_as(&self, other: &Header) -> bool {
    self.core_sampling_frequency == other.core_sampling_frequency
      && self.lfe_type == other.lfe_type
      && self.audio_channels == other.audio_channels
      && self.packet_length_ns() == other.packet_length_ns()
  }

  fn codec_name(&self) -> &'static str {
    match self.dts_type {
      DtsType::MasterAudio => "DTS-HD Master Audio",
      DtsType::HighResolution => "DTS-HD High Resolution",
      DtsType::Express => "DTS Express",
      DtsType::Es => "DTS-ES",
      DtsType::X96_24 => "DTS 96/24",
      DtsType::Normal => "DTS",
    }
  }

  /// Port of `decode_core_header`.
  fn decode_core(&mut self, buf: &[u8], allow_no_exss_search: bool) -> bool {
    let size = buf.len();
    let mut br = BitReader::new(buf);
    if br.skip_bits(32).is_err() {
      return false;
    }

    let res = (|| -> Option<()> {
      self.frametype = if br.read_bit().ok()? {
        FrameType::Normal
      } else {
        FrameType::Termination
      };
      self.deficit_sample_count = (br.read_bits(5).ok()? as u32 + 1) % 32;
      self.crc_present = br.read_bit().ok()?;
      self.num_pcm_sample_blocks = br.read_bits(7).ok()? as u32 + 1;
      self.frame_byte_size = br.read_bits(14).ok()? as usize + 1;

      if self.frame_byte_size < 96 {
        return None;
      }

      let t = br.read_bits(6).ok()? as usize;
      if t >= 16 {
        self.audio_channels = -1;
      } else {
        self.audio_channels = CHANNEL_ARRANGEMENTS[t] as i32;
      }

      self.core_sampling_frequency = CORE_SAMPLE_FREQS[br.read_bits(4).ok()? as usize];
      br.read_bits(5).ok()?; // transmission bitrate index
      br.read_bit().ok()?; // embedded_down_mix
      br.read_bit().ok()?; // embedded_dynamic_range
      br.read_bit().ok()?; // embedded_time_stamp
      br.read_bit().ok()?; // auxiliary_data
      br.read_bit().ok()?; // hdcd_master
      self.extension_audio_descriptor = br.read_bits(3).ok()? as u32;
      self.extended_coding = br.read_bit().ok()?;
      self.audio_sync_word_in_sub_sub = br.read_bit().ok()?;
      self.lfe_type = match br.read_bits(2).ok()? {
        0 => LfeType::None,
        1 => LfeType::Lfe128,
        2 => LfeType::Lfe64,
        _ => LfeType::Invalid,
      };
      self.predictor_history_flag = br.read_bit().ok()?;

      if self.crc_present {
        br.read_bits(16).ok()?;
      }

      br.read_bit().ok()?; // multirate_interpolator
      self.encoder_software_revision = br.read_bits(4).ok()? as u32;
      self.copy_history = br.read_bits(2).ok()? as u32;

      match br.read_bits(3).ok()? {
        0 => {
          self.source_pcm_resolution = 16;
          self.source_surround_in_es = false;
        }
        1 => {
          self.source_pcm_resolution = 16;
          self.source_surround_in_es = true;
        }
        2 => {
          self.source_pcm_resolution = 20;
          self.source_surround_in_es = false;
        }
        3 => {
          self.source_pcm_resolution = 20;
          self.source_surround_in_es = true;
        }
        5 => {
          self.source_pcm_resolution = 24;
          self.source_surround_in_es = true;
        }
        6 => {
          self.source_pcm_resolution = 24;
          self.source_surround_in_es = false;
        }
        _ => return None, // spr_invalid4 / spr_invalid7
      }

      br.read_bit().ok()?; // front_sum_difference
      br.read_bit().ok()?; // surround_sum_difference
      let _dng_bit_pos = br.position_bits();
      let _t = br.read_bits(4).ok()?;
      Some(())
    })();

    if res.is_none() {
      return false;
    }

    self.has_core = true;
    self.has_exss = false;
    self.exss_part_size = 0;
    self.exss_offset = self.frame_byte_size;
    self.dts_type = DtsType::Normal;

    if self.extended_coding && self.extension_audio_descriptor == 0 && self.frame_byte_size <= size {
      self.locate_and_decode_xch_header(&buf[..self.frame_byte_size]);
    }

    // x96k (2) or xch_x96k (3) extension audio descriptor.
    if self.extended_coding && (self.extension_audio_descriptor == 2 || self.extension_audio_descriptor == 3) {
      self.dts_type = DtsType::X96_24;
    }

    if self.exss_offset + 9 > size {
      return allow_no_exss_search;
    }

    let next = get_u32_be(&buf[self.exss_offset..]);
    if next == SYNC_EXSS {
      return self.decode_exss(&buf[self.exss_offset..]);
    }
    if next == SYNC_X96 {
      // decode_x96_header always fails in mkvtoolnix.
      return false;
    }

    if self.dts_type == DtsType::Normal && self.source_surround_in_es {
      self.dts_type = DtsType::Es;
    }

    true
  }

  /// Port of `locate_and_decode_xch_header`.
  fn locate_and_decode_xch_header(&mut self, buf: &[u8]) {
    let size = buf.len();
    if size < 96 + 4 {
      return;
    }
    let mut pos = size - 96;
    let mut sync = get_u32_be(&buf[pos..]);
    loop {
      if sync == SYNC_XCH {
        let mut bc = BitReader::new(&buf[pos..]);
        if bc.skip_bits(32).is_ok() {
          if let Ok(v) = bc.read_bits(10) {
            let primary_frame_byte_size = v as usize + 1;
            if pos + primary_frame_byte_size == size {
              let _audio_mode = bc.read_bits(4);
              self.has_xch = true;
              return;
            }
          }
        }
      }
      if pos == 0 {
        break;
      }
      pos -= 1;
      sync = (sync >> 8) | ((buf[pos] as u32) << 24);
    }
  }

  /// Port of `decode_exss_header`.
  fn decode_exss(&mut self, buf: &[u8]) -> bool {
    let mut br = BitReader::new(buf);
    self.decode_exss_inner(&mut br).unwrap_or(false)
  }

  fn decode_exss_inner(&mut self, br: &mut BitReader<'_>) -> Option<bool> {
    br.skip_bits(32).ok()?; // sync word
    br.skip_bits(8).ok()?; // user defined
    let substream_index = br.read_bits(2).ok()? as u32;
    let mut header_size_bits = 8u32;
    self.substream_size_bits = 16;

    if br.read_bit().ok()? {
      header_size_bits = 12;
      self.substream_size_bits = 20;
    }

    self.exss_header_size = br.read_bits(header_size_bits).ok()? as usize + 1;
    self.exss_part_size = br.read_bits(self.substream_size_bits).ok()? as usize + 1;
    self.frame_byte_size += self.exss_part_size;
    self.has_exss = true;

    self.num_presentations = 1;
    self.num_assets = 1;
    self.num_mixing_configurations = 0;
    self.static_fields_present = br.read_bit().ok()?;

    if self.static_fields_present {
      self.reference_clock_code = br.read_bits(2).ok()? as u32;
      self.substream_frame_duration = (br.read_bits(3).ok()? as u32 + 1) * 512;
      if br.read_bit().ok()? {
        br.skip_bits(32 + 4).ok()?; // timestamp data
      }

      self.num_presentations = br.read_bits(3).ok()? as u32 + 1;
      self.num_assets = br.read_bits(3).ok()? as u32 + 1;

      let mut active_substream_mask = [0u32; 8];
      for pres_idx in 0..self.num_presentations.min(8) as usize {
        active_substream_mask[pres_idx] = br.read_bits(substream_index + 1).ok()? as u32;
      }

      for pres_idx in 0..self.num_presentations.min(8) as usize {
        for subs_idx in 0..=substream_index {
          if active_substream_mask[pres_idx] & (1 << subs_idx) != 0 {
            br.skip_bits(8).ok()?;
          }
        }
      }

      self.mix_metadata_enabled = br.read_bit().ok()?;
      if self.mix_metadata_enabled {
        br.skip_bits(2).ok()?; // mixing metadata adjustment level
        let speaker_mask_num_bits = (br.read_bits(2).ok()? as u32 + 1) << 2;
        self.num_mixing_configurations = br.read_bits(2).ok()? as u32 + 1;

        for mix_idx in 0..self.num_mixing_configurations.min(5) as usize {
          self.num_mixing_channels[mix_idx] = count_channels_for_mask(br.read_bits(speaker_mask_num_bits).ok()? as u32);
        }
      }
    }

    self.substream_assets = vec![Asset::default(); self.num_assets as usize];

    let mut offset = self.exss_header_size;
    for asset_idx in 0..self.num_assets as usize {
      let asset = &mut self.substream_assets[asset_idx];
      asset.asset_offset = offset;
      asset.asset_size = br.read_bits(self.substream_size_bits).ok()? as usize + 1;
      offset += asset.asset_size;
    }

    for asset_idx in 0..self.num_assets as usize {
      if !self.decode_asset(br, asset_idx)? {
        return Some(false);
      }
    }

    Some(true)
  }

  /// Port of `decode_asset`.
  fn decode_asset(&mut self, br: &mut BitReader<'_>, asset_idx: usize) -> Option<bool> {
    let descriptor_pos = br.position_bits();
    let descriptor_size = br.read_bits(9).ok()? as u64 + 1;
    br.read_bits(3).ok()?; // asset_index

    if self.static_fields_present {
      if br.read_bit().ok()? {
        br.skip_bits(4).ok()?; // asset type descriptor
      }
      if br.read_bit().ok()? {
        br.skip_bits(24).ok()?; // language descriptor
      }
      if br.read_bit().ok()? {
        let n = (br.read_bits(10).ok()? as u64 + 1) * 8;
        br.skip_bits(n).ok()?; // additional textual information
      }

      br.read_bits(5).ok()?; // pcm_bit_res
      {
        let idx = br.read_bits(4).ok()? as usize;
        self.substream_assets[asset_idx].max_sample_rate = SUBSTREAM_SAMPLE_RATES[idx];
      }
      let num_channels_total = br.read_bits(8).ok()? as u32 + 1;
      let one_to_one = br.read_bit().ok()?;
      self.substream_assets[asset_idx].num_channels_total = num_channels_total;
      self.substream_assets[asset_idx].one_to_one_map_channel_to_speaker = one_to_one;

      if one_to_one {
        if num_channels_total > 2 {
          self.substream_assets[asset_idx].embedded_stereo = br.read_bit().ok()?;
        }
        if num_channels_total > 6 {
          self.substream_assets[asset_idx].embedded_6ch = br.read_bit().ok()?;
        }

        let mut speaker_mask_num_bits = 16u32;
        if br.read_bit().ok()? {
          speaker_mask_num_bits = (br.read_bits(2).ok()? as u32 + 1) << 2;
          br.skip_bits(speaker_mask_num_bits as u64).ok()?;
        }

        let num_speaker_remapping_sets = br.read_bits(3).ok()? as usize;
        let mut num_speakers = [0u32; 8];
        for s in num_speakers.iter_mut().take(num_speaker_remapping_sets.min(8)) {
          *s = count_channels_for_mask(br.read_bits(speaker_mask_num_bits).ok()? as u32);
        }

        for set_idx in 0..num_speaker_remapping_sets.min(8) {
          let num_channels_for_remapping = br.read_bits(5).ok()? as u32 + 1;
          for _ in 0..num_speakers[set_idx] {
            let remap_channel_mask = br.read_bits(num_channels_for_remapping).ok()? as u32;
            let num_remapping_codes = remap_channel_mask.count_ones();
            br.skip_bits((num_remapping_codes * 5) as u64).ok()?;
          }
        }
      } else {
        self.substream_assets[asset_idx].embedded_stereo = false;
        self.substream_assets[asset_idx].embedded_6ch = false;
        br.read_bits(3).ok()?; // representation_type
      }
    }

    let drc_present = br.read_bit().ok()?;
    if drc_present {
      br.skip_bits(8).ok()?;
    }

    if br.read_bit().ok()? {
      br.read_bits(5).ok()?; // extension dialog normalization gain
    }

    if drc_present && self.substream_assets[asset_idx].embedded_stereo {
      br.skip_bits(8).ok()?;
    }

    if self.mix_metadata_enabled && br.read_bit().ok()? {
      br.skip_bits(1).ok()?; // external mixing flag
      br.skip_bits(6).ok()?; // post mixing / replacement gain adjustment

      if br.read_bits(2).ok()? == 3 {
        br.skip_bits(8).ok()?;
      } else {
        br.skip_bits(3).ok()?;
      }

      if br.read_bit().ok()? {
        for mix_idx in 0..self.num_mixing_configurations.min(5) as usize {
          br.skip_bits((6 * self.num_mixing_channels[mix_idx]) as u64).ok()?;
        }
      } else {
        br.skip_bits((6 * self.num_mixing_configurations) as u64).ok()?;
      }

      let mut num_channels_downmix = self.substream_assets[asset_idx].num_channels_total;
      if self.substream_assets[asset_idx].embedded_6ch {
        num_channels_downmix += 6;
      }
      if self.substream_assets[asset_idx].embedded_stereo {
        num_channels_downmix += 2;
      }

      for mix_idx in 0..self.num_mixing_configurations.min(5) as usize {
        for _ in 0..num_channels_downmix {
          let mixing_map_mask = br.read_bits(self.num_mixing_channels[mix_idx]).ok()? as u32;
          let num_mixing_coefficients = mixing_map_mask.count_ones();
          br.skip_bits((6 * num_mixing_coefficients) as u64).ok()?;
        }
      }
    }

    let coding_mode = br.read_bits(2).ok()? as u32;
    self.substream_assets[asset_idx].coding_mode = coding_mode;
    match coding_mode {
      0 => {
        let mask = br.read_bits(12).ok()? as u32;
        self.substream_assets[asset_idx].extension_mask = mask;
        if mask & EXSS_CORE != 0 {
          self.substream_assets[asset_idx].core_size = br.read_bits(14).ok()? as usize + 1;
          if br.read_bit().ok()? {
            br.skip_bits(2).ok()?;
          }
        }
        if mask & EXSS_XBR != 0 {
          self.substream_assets[asset_idx].xbr_size = br.read_bits(14).ok()? as usize + 1;
        }
        if mask & EXSS_XXCH != 0 {
          self.substream_assets[asset_idx].xxch_size = br.read_bits(14).ok()? as usize + 1;
        }
        if mask & EXSS_X96 != 0 {
          self.substream_assets[asset_idx].x96_size = br.read_bits(12).ok()? as usize + 1;
        }
        if mask & EXSS_LBR != 0 {
          self.parse_lbr_parameters(br, asset_idx)?;
        }
        if mask & EXSS_XLL != 0 {
          self.parse_xll_parameters(br, asset_idx)?;
        }
        if mask & EXSS_RSV1 != 0 {
          br.skip_bits(16).ok()?;
        }
        if mask & EXSS_RSV2 != 0 {
          br.skip_bits(16).ok()?;
        }
      }
      1 => {
        self.substream_assets[asset_idx].extension_mask = EXSS_XLL;
        self.parse_xll_parameters(br, asset_idx)?;
      }
      2 => {
        self.substream_assets[asset_idx].extension_mask = EXSS_LBR;
        self.parse_lbr_parameters(br, asset_idx)?;
      }
      3 => {
        self.substream_assets[asset_idx].extension_mask = 0;
        br.skip_bits(14).ok()?;
        br.skip_bits(8).ok()?;
        if br.read_bit().ok()? {
          br.skip_bits(3).ok()?;
        }
      }
      _ => unreachable!(),
    }

    let mask = self.substream_assets[asset_idx].extension_mask;
    if mask & EXSS_XLL != 0 {
      br.read_bits(3).ok()?; // hd_stream_id
    }

    if !self.set_extension_offsets(asset_idx) {
      return Some(false);
    }

    if mask & EXSS_LBR != 0 && !self.decode_lbr_header(br, asset_idx)? {
      return Some(false);
    }
    if mask & EXSS_XLL != 0 && !self.decode_xll_header(br, asset_idx)? {
      return Some(false);
    }

    if mask & EXSS_XLL != 0 {
      self.dts_type = DtsType::MasterAudio;
    } else if mask & EXSS_X96 != 0 {
      self.dts_type = DtsType::HighResolution;
      self.extension_sampling_frequency = Some(96_000);
    } else if mask & (EXSS_XBR | EXSS_XXCH) != 0 {
      self.dts_type = DtsType::HighResolution;
    } else if mask & EXSS_LBR != 0 {
      self.dts_type = DtsType::Express;
    }

    br.set_bit_position(descriptor_pos + descriptor_size * 8);

    Some(true)
  }

  fn parse_lbr_parameters(&mut self, br: &mut BitReader<'_>, asset_idx: usize) -> Option<()> {
    self.substream_assets[asset_idx].lbr_size = br.read_bits(14).ok()? as usize + 1;
    self.substream_assets[asset_idx].lbr_sync_present = br.read_bit().ok()?;
    if self.substream_assets[asset_idx].lbr_sync_present {
      br.skip_bits(2).ok()?;
    }
    Some(())
  }

  fn parse_xll_parameters(&mut self, br: &mut BitReader<'_>, asset_idx: usize) -> Option<()> {
    self.substream_assets[asset_idx].xll_size = br.read_bits(self.substream_size_bits).ok()? as usize + 1;
    self.substream_assets[asset_idx].xll_sync_present = br.read_bit().ok()?;

    if self.substream_assets[asset_idx].xll_sync_present {
      br.skip_bits(4).ok()?;
      let xll_delay_num_bits = br.read_bits(5).ok()? as u32 + 1;
      br.read_bits(xll_delay_num_bits).ok()?; // xll_delay_num_frames
      self.substream_assets[asset_idx].xll_sync_offset = br.read_bits(self.substream_size_bits).ok()? as usize;
    } else {
      self.substream_assets[asset_idx].xll_sync_offset = 0;
    }
    Some(())
  }

  fn decode_lbr_header(&mut self, br: &mut BitReader<'_>, asset_idx: usize) -> Option<bool> {
    let lbr_offset = self.substream_assets[asset_idx].lbr_offset;
    br.set_bit_position((lbr_offset * 8) as u64);
    if self.substream_assets[asset_idx].lbr_sync_present && br.read_bits(32).ok()? as u32 != SYNC_LBR {
      return Some(false);
    }

    let format_info_code = br.read_bits(8).ok()? as u32;
    if format_info_code == 2 {
      // decoder_init
      self.core_sampling_frequency = LBR_SAMPLE_FREQS[(br.read_bits(8).ok()? as usize) & 0x0f];
    } else if format_info_code != 1 {
      // not sync_only
      return Some(false);
    }
    Some(true)
  }

  fn decode_xll_header(&mut self, br: &mut BitReader<'_>, asset_idx: usize) -> Option<bool> {
    let xll_offset = self.substream_assets[asset_idx].xll_offset;
    let xll_sync_offset = self.substream_assets[asset_idx].xll_sync_offset;
    br.set_bit_position(((xll_offset + xll_sync_offset) * 8) as u64);
    if self.substream_assets[asset_idx].xll_sync_present && br.read_bits(32).ok()? as u32 != SYNC_XLL {
      return Some(false);
    }

    if self.substream_assets[asset_idx].extension_mask & EXSS_LBR == 0 {
      let max_sample_rate = self.substream_assets[0].max_sample_rate;
      if !self.has_core {
        self.core_sampling_frequency = max_sample_rate;
      } else {
        self.extension_sampling_frequency = Some(max_sample_rate);
      }
    }
    Some(true)
  }

  /// Port of `set_extension_offsets` + `set_one_extension_offset`.
  fn set_extension_offsets(&mut self, asset_idx: usize) -> bool {
    let mut offset = self.substream_assets[asset_idx].asset_offset;
    let mut size = self.substream_assets[asset_idx].asset_size;
    let mask = self.substream_assets[asset_idx].extension_mask;

    let mut step = |wanted: u32, dst_offset: &mut usize, size_in_asset: usize| -> bool {
      if mask & wanted == 0 {
        return true;
      }
      if (offset & 3) != 0 || size_in_asset > size {
        return false;
      }
      *dst_offset = offset;
      offset += size_in_asset;
      size -= size_in_asset;
      true
    };

    let a = &self.substream_assets[asset_idx];
    let (core_size, xbr_size, xxch_size, x96_size, lbr_size, xll_size) =
      (a.core_size, a.xbr_size, a.xxch_size, a.x96_size, a.lbr_size, a.xll_size);
    let mut core_offset = a.core_offset;
    let mut xbr_offset = a.xbr_offset;
    let mut xxch_offset = a.xxch_offset;
    let mut x96_offset = a.x96_offset;
    let mut lbr_offset = a.lbr_offset;
    let mut xll_offset = a.xll_offset;

    let ok = step(EXSS_CORE, &mut core_offset, core_size)
      && step(EXSS_XBR, &mut xbr_offset, xbr_size)
      && step(EXSS_XXCH, &mut xxch_offset, xxch_size)
      && step(EXSS_X96, &mut x96_offset, x96_size)
      && step(EXSS_LBR, &mut lbr_offset, lbr_size)
      && step(EXSS_XLL, &mut xll_offset, xll_size);

    let a = &mut self.substream_assets[asset_idx];
    a.core_offset = core_offset;
    a.xbr_offset = xbr_offset;
    a.xxch_offset = xxch_offset;
    a.x96_offset = x96_offset;
    a.lbr_offset = lbr_offset;
    a.xll_offset = xll_offset;
    ok
  }
}

/// Port of `count_channels_for_mask`.
fn count_channels_for_mask(mask: u32) -> u32 {
  mask.count_ones() + (mask & SPEAKER_PAIR_ALL_2).count_ones()
}

/// Port of `find_sync_word` — scans for a core (0x7ffe8001) or exss
/// (0x64582025) sync word at byte granularity.
pub fn find_sync_word(buf: &[u8]) -> Option<usize> {
  if buf.len() < 4 {
    return None;
  }
  let mut offset = 0usize;
  let mut sync = get_u32_be(buf);
  loop {
    if sync == SYNC_CORE || sync == SYNC_EXSS {
      return Some(offset);
    }
    if offset + 4 >= buf.len() {
      return None;
    }
    sync = (sync << 8) | (buf[offset + 4] as u32);
    offset += 1;
  }
}

/// Port of `find_header` / `find_header_internal`.
fn find_header(buf: &[u8], allow_no_exss_search: bool) -> Option<(usize, Header)> {
  if buf.len() < 15 {
    return None;
  }
  let offset = find_sync_word(buf)?;
  let sync = get_u32_be(&buf[offset..]);
  let mut header = Header::default();
  let ok = if sync == SYNC_CORE {
    header.decode_core(&buf[offset..], allow_no_exss_search)
  } else if sync == SYNC_EXSS {
    header.decode_exss(&buf[offset..])
  } else {
    false
  };
  if ok { Some((offset, header)) } else { None }
}

/// Port of `find_consecutive_headers`.
pub fn find_consecutive_headers(buf: &[u8], num: u32) -> Option<usize> {
  let (pos, mut header) = find_header(buf, false)?;
  if num == 1 {
    return Some(pos);
  }
  let mut base = pos;

  loop {
    let mut offset = header.frame_byte_size;
    let mut i = 0u32;
    while i < num - 1 {
      if buf.len() < 2 + base + offset {
        break;
      }
      match find_header(&buf[base + offset..], false) {
        Some((0, new_header)) => {
          if new_header.same_as(&header) {
            offset += new_header.frame_byte_size;
            i += 1;
            continue;
          }
          break;
        }
        _ => break,
      }
    }

    if i == num - 1 {
      return Some(base);
    }

    base += 1;
    match find_header(&buf[base.min(buf.len())..], false) {
      Some((p, h)) => {
        header = h;
        base += p;
      }
      None => return None,
    }

    if base >= buf.len().saturating_sub(5) {
      break;
    }
  }

  None
}

/// Port of `convert_14_to_16_bits` (operating directly on the byte stream).
/// Each 16-byte group (eight 14-bit big-endian words) packs to 14 bytes.
pub fn convert_14_to_16_bits(src: &[u8]) -> Vec<u8> {
  let groups = src.len() / 16;
  let mut out = Vec::with_capacity(groups * 14);
  for g in 0..groups {
    let base = g * 16;
    let mut acc: u128 = 0;
    for w in 0..8 {
      let word = (((src[base + w * 2] as u16) << 8) | src[base + w * 2 + 1] as u16) & 0x3fff;
      acc = (acc << 14) | word as u128;
    }
    for byte_i in (0..14).rev() {
      out.push(((acc >> (byte_i * 8)) & 0xff) as u8);
    }
  }
  out
}

/// Swap adjacent byte pairs (16-bit byte-swap). Mirrors `swap_buffer(.., 2)`.
pub fn swap_buffer_16(src: &[u8]) -> Vec<u8> {
  let mut out = src.to_vec();
  let mut i = 0;
  while i + 1 < out.len() {
    out.swap(i, i + 1);
    i += 2;
  }
  out
}

/// Port of `mtx::dts::detect`. Returns `(convert_14_to_16, swap_bytes)` for
/// the transform combination under which a DTS header decodes, or `None`.
pub fn detect(src: &[u8]) -> Option<(bool, bool)> {
  let len = src.len() & !0xf;
  if len == 0 {
    return None;
  }
  let base = &src[..len];

  for swap in [false, true] {
    let swapped = if swap { swap_buffer_16(base) } else { base.to_vec() };
    for c1416 in [false, true] {
      let test = if c1416 {
        convert_14_to_16_bits(&swapped)
      } else {
        swapped.clone()
      };
      if find_header(&test, false).is_some() {
        return Some((c1416, swap));
      }
    }
  }
  None
}

/// Apply the [`detect`] transform to a buffer before frame decoding (mirrors
/// `dts_reader_c::decode_buffer`).
fn apply_transform(src: &[u8], convert_14_to_16: bool, swap_bytes: bool) -> Vec<u8> {
  let len = src.len() & !0xf;
  let mut buf = if swap_bytes {
    swap_buffer_16(&src[..len])
  } else {
    src[..len].to_vec()
  };
  if convert_14_to_16 {
    buf = convert_14_to_16_bits(&buf);
  }
  buf
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DtsReader;

impl Reader for DtsReader {
  fn name(&self) -> &'static str {
    "dts"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut probe)?;
    src.seek_to(0)?;
    if read < 16 {
      return Ok(false);
    }
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    let bytes = &probe[start..read];
    Ok(detect(bytes).is_some() || find_consecutive_headers(bytes, 5).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut probe)?;
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    let bytes = &probe[start..read];

    let (c1416, swap) = detect(bytes).ok_or(ParseError::Unrecognised)?;
    let transformed = apply_transform(bytes, c1416, swap);
    let (_offset, header) = find_header(&transformed, false).ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Dts;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let mut audio = AudioTrackProperties::default();
    let rate = header.effective_sampling_frequency();
    if rate > 0 {
      audio.sampling_frequency = Some(rate as f64);
    }
    let channels = header.total_num_audio_channels();
    if channels > 0 {
      audio.channels = Some(channels);
    }
    if header.source_pcm_resolution > 0 {
      audio.bit_depth = Some(header.source_pcm_resolution as u32);
    }

    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: "A_DTS".to_string(),
        name: Some(header.codec_name().to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        audio: Some(audio),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

#[cfg(test)]
mod test_support {
  /// MSB-first bit writer for building synthetic DTS frames in tests.
  pub struct BitWriter {
    pub bytes: Vec<u8>,
    bit_pos: usize,
  }

  impl BitWriter {
    pub fn new() -> Self {
      BitWriter {
        bytes: Vec::new(),
        bit_pos: 0,
      }
    }

    pub fn put(&mut self, n: u32, value: u64) {
      for i in (0..n).rev() {
        let bit = ((value >> i) & 1) as u8;
        let byte_idx = self.bit_pos / 8;
        if byte_idx >= self.bytes.len() {
          self.bytes.push(0);
        }
        if bit != 0 {
          self.bytes[byte_idx] |= 0x80 >> (self.bit_pos % 8);
        }
        self.bit_pos += 1;
      }
    }
  }
}

#[cfg(test)]
pub(crate) fn build_dts_core_frame(amode: u8, sfreq_idx: u8) -> Vec<u8> {
  use test_support::BitWriter;
  let mut w = BitWriter::new();
  // 32-bit sync word 0x7ffe8001.
  w.put(32, SYNC_CORE as u64);
  w.put(1, 1); // frametype = normal
  w.put(5, 0); // deficit
  w.put(1, 0); // crc_present = no
  w.put(7, 0); // num_pcm_sample_blocks - 1
  w.put(14, 95); // frame_byte_size - 1 = 95 → 96 bytes
  w.put(6, amode as u64); // AMODE
  w.put(4, sfreq_idx as u64); // SFREQ
  w.put(5, 0); // transmission bitrate index
  w.put(1, 0); // embedded_down_mix
  w.put(1, 0); // embedded_dynamic_range
  w.put(1, 0); // embedded_time_stamp
  w.put(1, 0); // auxiliary_data
  w.put(1, 0); // hdcd_master
  w.put(3, 0); // extension_audio_descriptor
  w.put(1, 0); // extended_coding
  w.put(1, 0); // audio_sync_word_in_sub_sub
  w.put(2, 0); // lfe_type = none
  w.put(1, 0); // predictor_history_flag
  w.put(1, 0); // multirate_interpolator
  w.put(4, 0); // encoder_software_revision
  w.put(2, 0); // copy_history
  w.put(3, 0); // source_pcm_resolution → spr_16
  w.put(1, 0); // front_sum_difference
  w.put(1, 0); // surround_sum_difference
  w.put(4, 0); // dialog_normalization_gain
  let mut bytes = w.bytes;
  // Pad to frame_byte_size (96) plus the 9-byte exss-search lookahead.
  bytes.resize(96 + 16, 0);
  bytes
}

#[cfg(test)]
pub(crate) fn build_dts_stream(frames: usize, amode: u8, sfreq_idx: u8) -> Vec<u8> {
  // Each frame is 96 bytes; concatenate `frames` of them, then a trailing
  // 16-byte zero pad so the last frame's exss lookahead has room.
  let mut bytes = Vec::new();
  let one = build_dts_core_frame(amode, sfreq_idx);
  for _ in 0..frames {
    bytes.extend_from_slice(&one[..96]);
  }
  bytes.resize(bytes.len() + 16, 0);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn find_sync_word_locates_core_and_exss() {
    let mut bytes = vec![0xAAu8; 8];
    bytes.extend([0x7F, 0xFE, 0x80, 0x01]);
    assert_eq!(find_sync_word(&bytes), Some(8));

    let mut bytes = vec![0x00u8; 4];
    bytes.extend([0x64, 0x58, 0x20, 0x25]);
    assert_eq!(find_sync_word(&bytes), Some(4));

    assert_eq!(find_sync_word(&[0xAA; 64]), None);
    assert_eq!(find_sync_word(&[0x00, 0x01]), None);
  }

  #[test]
  fn decode_core_extracts_channels_and_rate() {
    // amode 6 = 3 channels, sfreq idx 13 = 48000.
    let frame = build_dts_core_frame(6, 13);
    let (off, h) = find_header(&frame, false).unwrap();
    assert_eq!(off, 0);
    assert_eq!(h.audio_channels, 3);
    assert_eq!(h.core_sampling_frequency, 48_000);
    assert_eq!(h.total_num_audio_channels(), 3);
    assert_eq!(h.effective_sampling_frequency(), 48_000);
    assert_eq!(h.dts_type, DtsType::Normal);
  }

  // ---- PARSER-011: invalid sample-rate indices --------------------------

  #[test]
  fn sample_freq_indices_14_and_15_are_invalid() {
    assert_eq!(CORE_SAMPLE_FREQS[14], 0);
    assert_eq!(CORE_SAMPLE_FREQS[15], 0);
    let frame = build_dts_core_frame(2, 14);
    let (_off, h) = find_header(&frame, false).unwrap();
    assert_eq!(h.core_sampling_frequency, 0);
    // Reported rate is suppressed when invalid.
    assert_eq!(h.effective_sampling_frequency(), 0);
  }

  // ---- PARSER-012: channel arrangement 13/14/15 -------------------------

  #[test]
  fn channel_arrangement_high_layouts() {
    assert_eq!(CHANNEL_ARRANGEMENTS[13], 7);
    assert_eq!(CHANNEL_ARRANGEMENTS[14], 8);
    assert_eq!(CHANNEL_ARRANGEMENTS[15], 8);
    let frame = build_dts_core_frame(13, 13);
    let (_off, h) = find_header(&frame, false).unwrap();
    assert_eq!(h.audio_channels, 7);
  }

  #[test]
  fn amode_16_or_above_is_unknown_channels() {
    // amode 16 is invalid (>= 16) → audio_channels = -1.
    let frame = build_dts_core_frame(16, 13);
    let (_off, h) = find_header(&frame, false).unwrap();
    assert_eq!(h.audio_channels, -1);
    assert_eq!(h.total_num_audio_channels(), 0);
  }

  #[test]
  fn frame_byte_size_below_96_rejected() {
    use test_support::BitWriter;
    let mut w = BitWriter::new();
    w.put(32, SYNC_CORE as u64);
    w.put(1, 1);
    w.put(5, 0);
    w.put(1, 0);
    w.put(7, 0);
    w.put(14, 10); // frame_byte_size = 11 < 96
    w.put(6, 2);
    w.put(4, 13);
    w.put(5, 0);
    let mut bytes = w.bytes;
    bytes.resize(128, 0);
    assert!(find_header(&bytes, false).is_none());
  }

  #[test]
  fn invalid_source_pcm_resolution_rejected() {
    use test_support::BitWriter;
    let mut w = BitWriter::new();
    w.put(32, SYNC_CORE as u64);
    w.put(1, 1);
    w.put(5, 0);
    w.put(1, 0);
    w.put(7, 0);
    w.put(14, 95);
    w.put(6, 2);
    w.put(4, 13);
    w.put(5, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(3, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(2, 0);
    w.put(1, 0);
    w.put(1, 0);
    w.put(4, 0);
    w.put(2, 0);
    w.put(3, 4); // spr_invalid4
    let mut bytes = w.bytes;
    bytes.resize(128, 0);
    assert!(find_header(&bytes, false).is_none());
  }

  // ---- PARSER-010: frame validation in probe ----------------------------

  #[test]
  fn probe_rejects_bare_sync_word() {
    // A core sync word followed by garbage that fails to decode as a header.
    let mut bytes = vec![0u8; 128];
    bytes[0] = 0x7F;
    bytes[1] = 0xFE;
    bytes[2] = 0x80;
    bytes[3] = 0x01;
    // frame_byte_size bits left as 1 (< 96) → decode_core rejects.
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!DtsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn minimal_exss_header_decodes() {
    // An exss sync word followed by zeros decodes to a valid (degenerate)
    // extension substream with no assets/extensions — this matches
    // mkvtoolnix's `decode_exss_header`, which only rejects on truncation
    // or a failed asset/extension sub-decode.
    let mut bytes = vec![0u8; 128];
    bytes[0] = 0x64;
    bytes[1] = 0x58;
    bytes[2] = 0x20;
    bytes[3] = 0x25;
    let (off, h) = find_header(&bytes, false).unwrap();
    assert_eq!(off, 0);
    assert!(h.has_exss);
    assert!(!h.has_core);
    assert_eq!(h.dts_type, DtsType::Normal);
  }

  #[test]
  fn probe_accepts_valid_core_frame() {
    let bytes = build_dts_core_frame(2, 13);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(DtsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn find_consecutive_headers_requires_five() {
    let five = build_dts_stream(6, 2, 13);
    assert_eq!(find_consecutive_headers(&five, 5), Some(0));

    let two = build_dts_stream(2, 2, 13);
    assert_eq!(find_consecutive_headers(&two, 5), None);
  }

  #[test]
  fn find_consecutive_headers_num_one_is_just_find() {
    let one = build_dts_core_frame(2, 13);
    assert_eq!(find_consecutive_headers(&one, 1), Some(0));
  }

  // ---- PARSER-013: byte-swap + 14-bit normalisation ---------------------

  #[test]
  fn detect_plain_big_endian() {
    let frame = build_dts_stream(1, 2, 13);
    assert_eq!(detect(&frame), Some((false, false)));
  }

  #[test]
  fn detect_byte_swapped_stream() {
    let be = build_dts_stream(1, 2, 13);
    let le = swap_buffer_16(&be);
    // The raw LE buffer has no plain core sync, so detect must find it only
    // after un-swapping.
    assert_eq!(detect(&le), Some((false, true)));
  }

  #[test]
  fn detect_14_bit_stream() {
    let be = build_dts_core_frame(2, 13);
    // The 16-bit form must be a multiple of 14 bytes (each 14-byte group
    // packs into a 16-byte 14-bit group) and long enough for the frame plus
    // the 9-byte exss lookahead after frame_byte_size (96).
    let mut be16 = be[..96].to_vec();
    be16.resize(112, 0); // 8 groups of 14 bytes
    let packed = pack_16_to_14(&be16); // 128 bytes (8 groups of 16)
    // Round-trips back through the converter.
    let converted = convert_14_to_16_bits(&packed);
    assert_eq!(&converted[..96], &be16[..96]);
    // detect must report 14→16 conversion (on the non-swapped buffer).
    assert_eq!(detect(&packed), Some((true, false)));
  }

  #[test]
  fn read_headers_emits_dts_track() {
    let bytes = build_dts_stream(2, 6, 13);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.dts", 0);
    DtsReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Dts);
    assert_eq!(out.tracks[0].codec.id, "A_DTS");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("DTS"));
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.channels, Some(3));
    assert_eq!(audio.sampling_frequency, Some(48_000.0));
    assert_eq!(audio.bit_depth, Some(16));
  }

  #[test]
  fn read_headers_rejects_garbage() {
    let bytes = vec![0u8; 256];
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.dts", 0);
    assert!(
      DtsReader
        .read_headers(&mut s, &Deadline::new(60_000), &mut out)
        .is_err()
    );
  }

  #[test]
  fn count_channels_for_mask_matches_definition() {
    assert_eq!(count_channels_for_mask(0), 0);
    // Bit 0 set, not in SPEAKER_PAIR_ALL_2 (0xae66) → counts once.
    assert_eq!(count_channels_for_mask(0x0001), 1);
    // Bit 1 (0x2) is in 0xae66 → counts twice (a pair).
    assert_eq!(count_channels_for_mask(0x0002), 2);
  }

  #[test]
  fn swap_buffer_16_swaps_pairs() {
    assert_eq!(swap_buffer_16(&[1, 2, 3, 4]), vec![2, 1, 4, 3]);
    // Odd trailing byte is left in place.
    assert_eq!(swap_buffer_16(&[1, 2, 3]), vec![2, 1, 3]);
  }

  /// Inverse of [`convert_14_to_16_bits`] used only by tests: pack 16-bit
  /// big-endian words into 14-bit big-endian words (8 words → 16 bytes from
  /// 7 words → 14 bytes input).
  fn pack_16_to_14(src: &[u8]) -> Vec<u8> {
    let groups = src.len() / 14;
    let mut out = Vec::with_capacity(groups * 16);
    for g in 0..groups {
      let base = g * 14;
      // 14 bytes = 112 bits → split into 8 chunks of 14 bits.
      let mut acc: u128 = 0;
      for k in 0..14 {
        acc = (acc << 8) | src[base + k] as u128;
      }
      for w in 0..8 {
        let shift = (7 - w) * 14;
        let word = ((acc >> shift) & 0x3fff) as u16;
        out.push((word >> 8) as u8);
        out.push((word & 0xff) as u8);
      }
    }
    out
  }
}
