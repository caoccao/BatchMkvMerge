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

//! AAC reader — pure-Rust port of `mkvtoolnix/src/common/aac.cpp` +
//! `src/input/r_aac.cpp`.
//!
//! Supports both multiplex types mkvtoolnix recognises:
//!
//! * **ADTS** (ISO/IEC 13818-7 §6.2) — the 7-/9-byte frame header. When the
//!   `channel_configuration` field is 0, the Program Config Element (PCE) that
//!   follows the fixed header is parsed to derive the channel count (mirrors
//!   `decode_adts_header` + `read_program_config_element`).
//! * **LOAS/LATM** — the 11-bit `0x2B7` LOAS sync word (`0x56 0xE0..0xFF`)
//!   framing an `AudioMuxElement` whose `StreamMuxConfig` carries an
//!   `AudioSpecificConfig` (mirrors `decode_loas_latm_header` +
//!   `latm_parser_c` + `header_c::parse_audio_specific_config`).
//!
//! ADTS fixed header layout:
//!
//! ```text
//! 12 bits sync (FFF)
//! 1  bit  MPEG version (0=MPEG-4, 1=MPEG-2)
//! 2  bits layer (== 00)
//! 1  bit  protection_absent
//! 2  bits profile (object_type - 1)
//! 4  bits sampling_frequency_index
//! 1  bit  private
//! 3  bits channel_configuration
//! 1  bit  original_copy
//! 1  bit  home
//! 1  bit  copyright_identification_bit
//! 1  bit  copyright_identification_start
//! 13 bits frame_length (including header)
//! 11 bits buffer_fullness
//! 2  bits number_of_raw_data_blocks
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;
/// Primary AAC probe in mkvtoolnix requires eight consecutive headers at the
/// start (`do_probe<aac_reader_c>(io, { 128 * 1024, 8, true })`).
const MIN_CONFIRM_FRAMES: usize = 8;

const ADTS_SYNC_WORD: u32 = 0xfff000;
const ADTS_SYNC_WORD_MASK: u32 = 0xfff000; // first 12 of 24 bits
const LOAS_SYNC_WORD: u32 = 0x56e000; // 0x2b7
const LOAS_SYNC_WORD_MASK: u32 = 0xffe000; // first 11 of 24 bits
const LOAS_FRAME_SIZE_MASK: u32 = 0x001fff; // last 13 of 24 bits

const ID_PCE: u64 = 0x05; // Table 4.71 "Syntactic elements"
const SYNC_EXTENSION_TYPE: u64 = 0x02b7;

// AUDIO_OBJECT_TYPE constants used by the ASC decode (see common/mp4.h).
const AOT_AAC_MAIN: u32 = 0x01;
const AOT_AAC_LC: u32 = 0x02;
const AOT_AAC_SSR: u32 = 0x03;
const AOT_AAC_LTP: u32 = 0x04;
const AOT_SBR: u32 = 0x05;
const AOT_AAC_SCALABLE: u32 = 0x06;
const AOT_TWINVQ: u32 = 0x07;
/// Parametric Stereo (HE-AACv2) — treated as an SBR-style extension
/// (`../mkvtoolnix/src/common/mp4.h:68`, `aac.cpp:1224-1232`).
const AOT_PS: u32 = 0x1d;
const AOT_ER_AAC_LC: u32 = 0x11;
const AOT_ER_AAC_LTP: u32 = 0x13;
const AOT_ER_AAC_SCALABLE: u32 = 0x14;
const AOT_ER_TWINVQ: u32 = 0x15;
const AOT_ER_BSAC: u32 = 0x16;
const AOT_ER_AAC_LD: u32 = 0x17;

/// `PROFILE_SBR` (`../mkvtoolnix/src/common/aac.h:37`).  Raw AAC identification
/// promotes low-sample-rate ADTS streams to this profile (PARSER-215).
const PROFILE_SBR: u32 = 4;

// ISO/IEC 14496-3 table 1.16 — Sampling Frequency Index (16 entries; the last
// three are reserved / escape and map to 0 here, matching mkvtoolnix).
const SAMPLE_RATE_TABLE: [u32; 16] = [
  96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350, 0, 0, 0,
];

// ISO/IEC 14496-3 table 1.17 — Channel Configuration (21 entries to match
// mkvtoolnix's `s_aac_channel_configuration`).
const CHANNEL_CONFIGURATION: [u32; 21] = [
  0, 1, 2, 3, 4, 5, 6, 8, // from ISO/IEC 14496-3
  0, 3, 4, 7, 8, 24, 8, 12, // from Rec. ITU-R BS.1196-7 & ISO/IEC 23008-3:2019
  10, 12, 14, 12, 14,
];

#[derive(Debug, Clone, Copy, Default)]
pub struct AacHeader {
  pub id: u8, // 0 = MPEG-4, 1 = MPEG-2
  pub profile: u32,
  pub sample_rate: u32,
  pub output_sample_rate: u32,
  pub channels: u32,
  pub sbr: bool,
  /// Parametric Stereo (HE-AACv2) signalled via the PS object type.
  pub ps: bool,
  pub samples_per_frame: u32,
  /// Total frame length in bytes (ADTS only; 0 for LOAS-derived headers).
  pub bytes: usize,
  pub is_valid: bool,
}

/// Kept for backwards compatibility with earlier call sites / tests.
pub type AdtsHeader = AacHeader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MultiplexType {
  Adts,
  LoasLatm,
}

// ---- Local big-endian bit cursor ---------------------------------------
//
// The shared `io::BitReader` has no position setter, but the AAC ASC / LATM
// decode needs to rewind (e.g. the SBR sync-extension peek). A small local
// MSB-first cursor keeps every bit-level operation self-contained in this
// file. Reads past the end return `None` (never panic on file input).

struct Bits<'a> {
  bytes: &'a [u8],
  pos: u64, // bit position from the start
}

impl<'a> Bits<'a> {
  fn new(bytes: &'a [u8]) -> Self {
    Self { bytes, pos: 0 }
  }

  fn position(&self) -> u64 {
    self.pos
  }

  fn set_position(&mut self, pos: u64) {
    self.pos = pos;
  }

  fn remaining(&self) -> u64 {
    ((self.bytes.len() as u64) * 8).saturating_sub(self.pos)
  }

  fn get_bits(&mut self, n: u32) -> Option<u64> {
    if n == 0 {
      return Some(0);
    }
    if n > 64 {
      return None;
    }
    if self.pos + n as u64 > (self.bytes.len() as u64) * 8 {
      return None;
    }
    let mut acc: u64 = 0;
    let mut remaining = n;
    while remaining > 0 {
      let byte_idx = (self.pos / 8) as usize;
      let bit_off = (self.pos % 8) as u32;
      let avail = 8 - bit_off;
      let take = remaining.min(avail);
      let shift = avail - take;
      let mask = ((1u32 << take) - 1) as u8;
      let chunk = (self.bytes[byte_idx] >> shift) & mask;
      acc = (acc << take) | (chunk as u64);
      self.pos += take as u64;
      remaining -= take;
    }
    Some(acc)
  }

  fn get_bit(&mut self) -> Option<bool> {
    Some(self.get_bits(1)? != 0)
  }

  /// Read `n` bits without advancing the cursor (mirrors `bits::reader_c::peek_bits`).
  /// Returns `0` when fewer than `n` bits remain so guard expressions stay total.
  fn peek_bits(&mut self, n: u32) -> u64 {
    let saved = self.pos;
    let value = self.get_bits(n).unwrap_or(0);
    self.pos = saved;
    value
  }

  fn skip_bits(&mut self, n: u64) -> Option<()> {
    let new = self.pos.checked_add(n)?;
    if new > (self.bytes.len() as u64) * 8 {
      return None;
    }
    self.pos = new;
    Some(())
  }

  fn align_to_byte(&mut self) {
    let r = self.pos % 8;
    if r != 0 {
      self.pos += 8 - r;
    }
  }
}

fn get_uint24_be(bytes: &[u8]) -> u32 {
  ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | (bytes[2] as u32)
}

fn lookup_channels(channel_config: usize) -> u32 {
  if channel_config < CHANNEL_CONFIGURATION.len() {
    CHANNEL_CONFIGURATION[channel_config]
  } else {
    0
  }
}

// ---- ADTS --------------------------------------------------------------

/// Decode a single ADTS header. Returns `None` on any structural failure
/// (mirrors `parser_c::decode_adts_header` returning `failure`). When
/// `channel_configuration == 0`, the trailing Program Config Element is parsed
/// to recover the channel count rather than rejecting the frame.
pub fn decode_adts(bytes: &[u8]) -> Option<AacHeader> {
  let mut bc = Bits::new(bytes);

  if bc.get_bits(12)? != 0xfff {
    return None;
  }
  let id = bc.get_bits(1)? as u8; // 0 = MPEG-4, 1 = MPEG-2
  if bc.get_bits(2)? != 0 {
    return None; // layer must be 0
  }
  let protection_absent = bc.get_bit()?;
  let profile = bc.get_bits(2)? as u32;
  let sfreq_index = bc.get_bits(4)? as usize;
  if sfreq_index >= SAMPLE_RATE_TABLE.len() {
    return None;
  }
  bc.skip_bits(1)?; // private
  let channel_config = bc.get_bits(3)? as usize;
  let mut channels = lookup_channels(channel_config);
  bc.skip_bits(1 + 1)?; // original/copy & home
  bc.skip_bits(1 + 1)?; // copyright_id_bit & copyright_id_start

  let frame_bytes = bc.get_bits(13)? as usize;
  if frame_bytes > bytes.len() {
    return None; // need more data
  }

  bc.skip_bits(11)?; // adts_buffer_fullness
  bc.skip_bits(2)?; // no_raw_blocks_in_frame
  if !protection_absent {
    bc.skip_bits(16)?;
  }

  let header_byte_size = ((bc.position() + 7) / 8) as usize;
  if frame_bytes <= header_byte_size {
    return None;
  }

  // When channel_configuration == 0 a Program Config Element follows; parse
  // it to derive the channel count (mirrors decode_adts_header's PCE path).
  if channels == 0 {
    let data_start = bc.position();
    if bc.get_bits(3)? == ID_PCE {
      if let Some(pce_channels) = read_program_config_element(&mut bc) {
        channels = pce_channels;
      }
    }
    bc.set_position(data_start); // mkvtoolnix rewinds before reading data
  }

  Some(AacHeader {
    id,
    profile,
    sample_rate: SAMPLE_RATE_TABLE[sfreq_index],
    output_sample_rate: 0,
    channels,
    sbr: false,
    ps: false,
    samples_per_frame: 1024,
    bytes: frame_bytes,
    is_valid: true,
  })
}

/// Port of `header_c::read_program_config_element`. Returns the channel count
/// or `None` on any read failure.
fn read_program_config_element(bc: &mut Bits) -> Option<u32> {
  bc.skip_bits(4)?; // element_instance_tag
  let _object_type = bc.get_bits(2)?;
  let _sr_idx = bc.get_bits(4)?;
  let num_front_chan = bc.get_bits(4)? as u32;
  let num_side_chan = bc.get_bits(4)? as u32;
  let num_back_chan = bc.get_bits(4)? as u32;
  let num_lfe_chan = bc.get_bits(2)? as u32;
  let num_assoc_data = bc.get_bits(3)? as u32;
  let num_valid_cc = bc.get_bits(4)? as u32;

  if bc.get_bit()? {
    bc.skip_bits(4)?; // mono_mixdown_element_number
  }
  if bc.get_bit()? {
    bc.skip_bits(4)?; // stereo_mixdown_element_number
  }
  if bc.get_bit()? {
    bc.skip_bits(2 + 1)?; // matrix_mixdown_idx, pseudo_surround_enable
  }

  let mut channels = num_front_chan + num_side_chan + num_back_chan + num_lfe_chan;

  for _ in 0..(num_front_chan + num_side_chan + num_back_chan) {
    if bc.get_bit()? {
      channels += 1; // *_element_is_cpe
    }
    bc.skip_bits(4)?; // *_element_tag_select
  }
  bc.skip_bits((num_lfe_chan as u64) * 4)?;
  bc.skip_bits((num_assoc_data as u64) * 4)?;
  bc.skip_bits((num_valid_cc as u64) * (1 + 4))?;

  bc.align_to_byte();
  let comment_field_bytes = bc.get_bits(8)?;
  bc.skip_bits(comment_field_bytes * 8)?;

  Some(channels)
}

// ---- AudioSpecificConfig (used by LOAS/LATM) ---------------------------

fn read_object_type(bc: &mut Bits) -> Option<u32> {
  let ot = bc.get_bits(5)? as u32;
  if ot == 31 {
    Some(32 + bc.get_bits(6)? as u32)
  } else {
    Some(ot)
  }
}

fn read_sample_rate(bc: &mut Bits) -> Option<u32> {
  let idx = bc.get_bits(4)? as usize;
  if idx == 0x0f {
    Some(bc.get_bits(24)? as u32)
  } else if idx < SAMPLE_RATE_TABLE.len() {
    Some(SAMPLE_RATE_TABLE[idx])
  } else {
    Some(0)
  }
}

/// Minimal port of `header_c::parse_audio_specific_config`. Decodes
/// object_type / sample_rate / channels, handles the GA-specific config for
/// the AAC family (including the PCE when channel_configuration is 0) and
/// follows the SBR sync extension. Returns `None` on any read failure.
fn parse_audio_specific_config(bc: &mut Bits, look_for_sync_extension: bool) -> Option<AacHeader> {
  let mut object_type = read_object_type(bc)?;
  if object_type == 0 {
    return None;
  }

  let mut header = AacHeader {
    profile: object_type - 1,
    samples_per_frame: 1024,
    ..AacHeader::default()
  };
  let mut sbr = false;
  let mut ps = false;
  let mut output_sample_rate = 0u32;
  let mut extension_object_type = 0u32;

  let sample_rate = read_sample_rate(bc)?;
  let channel_config = bc.get_bits(4)? as usize;
  let mut channels = lookup_channels(channel_config);

  // Explicit SBR signalling — and PS (HE-AACv2), which mkvtoolnix treats as
  // an SBR-style extension when its bitstream guard passes (PARSER-214,
  // `../mkvtoolnix/src/common/aac.cpp:1224-1232`).
  let enter_sbr_extension = object_type == AOT_SBR
    || (object_type == AOT_PS && !((bc.peek_bits(3) & 0x03) != 0 && (bc.peek_bits(9) & 0x3f) == 0));
  if enter_sbr_extension {
    sbr = true;
    if object_type == AOT_PS {
      ps = true;
    }
    output_sample_rate = read_sample_rate(bc)?;
    extension_object_type = object_type;
    object_type = read_object_type(bc)?;
  }

  let is_ga_object = matches!(
    object_type,
    AOT_AAC_MAIN
      | AOT_AAC_LC
      | AOT_AAC_SSR
      | AOT_AAC_LTP
      | AOT_AAC_SCALABLE
      | AOT_TWINVQ
      | AOT_ER_AAC_LC
      | AOT_ER_AAC_LTP
      | AOT_ER_AAC_SCALABLE
      | AOT_ER_TWINVQ
      | AOT_ER_BSAC
      | AOT_ER_AAC_LD
  );

  if is_ga_object {
    // GASpecificConfig: frame_length_flag (1), depends_on_core_coder (1)
    // [+ core_coder_delay 14], extension_flag (1).
    let frame_length_flag = bc.get_bit()?;
    if bc.get_bit()? {
      bc.skip_bits(14)?; // core_coder_delay
    }
    let _extension_flag = bc.get_bit()?;
    if object_type != AOT_SBR && object_type != AOT_ER_AAC_LD {
      header.samples_per_frame = if frame_length_flag { 960 } else { 1024 };
    } else if object_type == AOT_ER_AAC_LD {
      header.samples_per_frame = if frame_length_flag { 480 } else { 512 };
    }
    if channels == 0 {
      channels = read_program_config_element(bc).unwrap_or(0);
    }
  }

  // Implicit SBR sync extension.
  if look_for_sync_extension && extension_object_type != AOT_SBR && bc.remaining() >= 16 {
    let prior = bc.position();
    let sync_extension_type = bc.get_bits(11)?;
    if sync_extension_type == SYNC_EXTENSION_TYPE {
      extension_object_type = read_object_type(bc)?;
      if extension_object_type == AOT_SBR {
        sbr = bc.get_bit()?;
        if sbr {
          output_sample_rate = read_sample_rate(bc)?;
        }
      }
    } else {
      bc.set_position(prior);
    }
  }

  // mkvtoolnix promotes 22.05–24 kHz streams to implicit SBR.
  if (22_050..=24_000).contains(&sample_rate) {
    output_sample_rate = 2 * sample_rate;
    sbr = true;
  }

  header.id = 0; // ASC implies MPEG-4
  header.sample_rate = sample_rate;
  header.output_sample_rate = output_sample_rate;
  header.channels = channels;
  header.sbr = sbr;
  header.ps = ps;
  header.is_valid = true;
  Some(header)
}

/// Decode a bare MPEG-4 AudioSpecificConfig payload.  Container readers that
/// carry AAC sequence headers (RealMedia/FLV/MP4-like wrappers) use this so
/// object type, SBR, sample-rate and channel handling stays identical to the
/// elementary AAC reader.
pub(crate) fn parse_audio_specific_config_bytes(bytes: &[u8]) -> Option<AacHeader> {
  let mut bc = Bits::new(bytes);
  parse_audio_specific_config(&mut bc, true)
}

pub(crate) fn codec_config_from_header(header: &AacHeader, raw: &[u8]) -> AudioCodecConfig {
  AudioCodecConfig {
    profile_name: Some(format_aac_profile(header.profile)),
    aac_object_type: Some(header.profile + 1),
    aac_frame_length: Some(header.samples_per_frame),
    aac_sbr_present: Some(header.sbr),
    aac_ps_present: Some(header.ps),
    raw_hex: Some(raw.iter().map(|b| format!("{:02x}", b)).collect()),
    ..AudioCodecConfig::default()
  }
}

// ---- LOAS / LATM -------------------------------------------------------

/// `latm_parser_c::get_value()`: 2-bit byte count + that many bytes.
fn latm_get_value(bc: &mut Bits) -> Option<u64> {
  let num_bytes = bc.get_bits(2)? + 1;
  bc.get_bits((8 * num_bytes) as u32)
}

struct LatmResult {
  header: AacHeader,
  frame_length: usize,
}

/// Port of `latm_parser_c::parse` → `parse_audio_mux_element` →
/// `parse_stream_mux_config`. Only `audio_mux_version == 0` /
/// `audio_mux_version_a == 0` (the DVB-common case mkvtoolnix supports) is
/// handled; anything else returns `None`. `use_same_stream_mux == 1` cannot be
/// resolved by a single-frame decode (no prior config), so it bails too.
fn parse_audio_mux_element(bc: &mut Bits) -> Option<LatmResult> {
  let use_same_stream_mux = bc.get_bit()?;
  if use_same_stream_mux {
    return None;
  }

  let (header, frame_length_type, fixed_frame_length) = parse_stream_mux_config(bc)?;
  if !header.is_valid {
    return None;
  }

  // parse_payload_length_info (audio_mux_version_a == 0 path).
  let frame_length = match frame_length_type {
    0 => {
      let mut length = 0u64;
      loop {
        let tmp = bc.get_bits(8)?;
        length += tmp;
        if tmp != 255 {
          break;
        }
      }
      length
    }
    1 => fixed_frame_length,
    3 | 5 | 7 => bc.get_bits(2)?,
    _ => 0,
  };

  Some(LatmResult {
    header,
    frame_length: frame_length as usize,
  })
}

/// Returns `(header, frame_length_type, fixed_frame_length)`.
fn parse_stream_mux_config(bc: &mut Bits) -> Option<(AacHeader, u64, u64)> {
  let audio_mux_version = bc.get_bit()?;
  let audio_mux_version_a = if audio_mux_version { bc.get_bit()? } else { false };
  if audio_mux_version_a {
    return None; // not supported
  }
  if audio_mux_version {
    latm_get_value(bc)?; // tara_buffer_fullness
  }

  bc.skip_bits(1 + 6)?; // all_stream_same_time_framing, num_sub_frames

  if bc.get_bits(4)? != 0 {
    return None; // more than one program not supported
  }
  if bc.get_bits(3)? != 0 {
    return None; // more than one layer not supported
  }

  let header = if !audio_mux_version {
    parse_audio_specific_config(bc, false)?
  } else {
    let asc_length = latm_get_value(bc)?;
    let prior = bc.position();
    let h = parse_audio_specific_config(bc, true)?;
    let used = bc.position() - prior;
    if used < asc_length {
      bc.skip_bits(asc_length - used)?;
    }
    h
  };

  let frame_length_type = bc.get_bits(3)?;
  let mut fixed_frame_length = 0u64;
  match frame_length_type {
    0 => {
      bc.skip_bits(8)?; // buffer_fullness
    }
    1 => {
      fixed_frame_length = bc.get_bits(9)?;
    }
    3 | 4 | 5 => {
      bc.skip_bits(6)?; // CELP frame length table index
    }
    6 | 7 => {
      bc.skip_bits(1)?; // HVXC frame length table index
    }
    _ => {}
  }

  if bc.get_bit()? {
    // other_data
    if audio_mux_version {
      latm_get_value(bc)?;
    } else {
      loop {
        let escape = bc.get_bit()?;
        bc.skip_bits(8)?;
        if !escape {
          break;
        }
      }
    }
  }

  if bc.get_bit()? {
    bc.skip_bits(8)?; // config_crc
  }

  Some((header, frame_length_type, fixed_frame_length))
}

/// Port of `parser_c::decode_loas_latm_header`. Returns the decoded header and
/// the total LOAS frame length in bytes (the amount to advance), or `None` on
/// any structural failure.
pub fn decode_loas_latm(bytes: &[u8]) -> Option<(AacHeader, usize)> {
  if bytes.len() < 3 {
    return None;
  }
  let value = get_uint24_be(bytes);
  if (value & LOAS_SYNC_WORD_MASK) != LOAS_SYNC_WORD {
    return None;
  }
  let loas_frame_size = (value & LOAS_FRAME_SIZE_MASK) as usize;
  let loas_frame_end = loas_frame_size + 3;
  if loas_frame_end > bytes.len() {
    return None; // need more data
  }

  let mut bc = Bits::new(&bytes[..loas_frame_end]);
  bc.skip_bits(3 * 8)?; // the 3-byte LOAS sync/length prefix

  let result = parse_audio_mux_element(&mut bc)?;

  let end_of_header_bit_pos = bc.position();
  let decoded_frame_end_bits = end_of_header_bit_pos + (result.frame_length as u64 * 8);
  if decoded_frame_end_bits > (loas_frame_end as u64 * 8) {
    return None;
  }
  if !result.header.is_valid {
    return None;
  }

  Some((result.header, loas_frame_end))
}

// ---- consecutive-frame probe (port of find_consecutive_frames) ---------

/// Port of `parser_c::find_consecutive_frames`. Scans for a base offset at
/// which `num_required_frames` consecutive headers (ADTS or LOAS/LATM) decode
/// successfully and whose fixed fields agree. Returns the base offset.
pub fn find_consecutive_frames(buffer: &[u8], num_required_frames: usize) -> Option<usize> {
  let buffer_size = buffer.len();
  if buffer_size < 9 {
    return None;
  }

  let mut base = 0usize;
  while base + 8 < buffer_size {
    let value = get_uint24_be(&buffer[base..]);
    let is_adts = (value & ADTS_SYNC_WORD_MASK) == ADTS_SYNC_WORD;
    let is_loas = (value & LOAS_SYNC_WORD_MASK) == LOAS_SYNC_WORD;

    if !is_adts && !is_loas {
      base += 1;
      continue;
    }

    // Shortcut: require a second compatible header right after this one
    // before running the (more expensive) full parse.
    if is_loas {
      let loas_frame_size = (value & LOAS_FRAME_SIZE_MASK) as usize;
      if loas_frame_size == 0 || (base + loas_frame_size + 3 + 3) > buffer_size {
        base += 1;
        continue;
      }
      let next = get_uint24_be(&buffer[base + 3 + loas_frame_size..]);
      if (next & LOAS_SYNC_WORD_MASK) != LOAS_SYNC_WORD {
        base += 1;
        continue;
      }
    } else {
      // adts_frame_size: 2@b3 + 8@b4 + 3@b5.
      let adts_frame_size = (((buffer[base + 3] & 0x03) as usize) << 11)
        | ((buffer[base + 4] as usize) << 3)
        | ((buffer[base + 5] as usize) >> 5);
      if adts_frame_size < 7 || (base + adts_frame_size + 8) > buffer_size {
        base += 1;
        continue;
      }
      let next = get_uint24_be(&buffer[base + adts_frame_size..]);
      if (next & ADTS_SYNC_WORD_MASK) != ADTS_SYNC_WORD {
        base += 1;
        continue;
      }
    }

    // Full parse from `base`, requiring a frame at the very first byte and
    // collecting up to num_required_frames frames.
    if let Some(frames) = collect_frames(&buffer[base..], num_required_frames) {
      if frames.len() >= num_required_frames {
        if frames.len() == 1 {
          return Some(base);
        }
        // Faithful port of mkvtoolnix's mismatch check: a frame is only
        // a mismatch when ALL of id/profile/channels/sample_rate differ
        // from the first frame (note the `&&` in the C++ source).
        let first = frames[0];
        let mut mismatch = false;
        for frame in frames.iter().skip(1) {
          if frame.id != first.id
            && frame.profile != first.profile
            && frame.channels != first.channels
            && frame.sample_rate != first.sample_rate
          {
            mismatch = true;
            break;
          }
        }
        if !mismatch {
          return Some(base);
        }
      }
    }

    base += 1;
  }

  None
}

/// Decode consecutive frames starting at the first byte of `buffer`, fixing the
/// multiplex type from the first frame, stopping after `max_frames` frames or
/// the first decode failure. Returns `None` if no valid frame starts at byte 0.
fn collect_frames(buffer: &[u8], max_frames: usize) -> Option<Vec<AacHeader>> {
  let multiplex = if decode_adts(buffer).is_some() {
    MultiplexType::Adts
  } else if decode_loas_latm(buffer).is_some() {
    MultiplexType::LoasLatm
  } else {
    return None;
  };

  let mut frames = Vec::with_capacity(max_frames);
  let mut position = 0usize;
  while position < buffer.len() && frames.len() < max_frames {
    let remaining = &buffer[position..];
    let decoded = match multiplex {
      MultiplexType::Adts => decode_adts(remaining).map(|h| (h, h.bytes.max(1))),
      MultiplexType::LoasLatm => decode_loas_latm(remaining).map(|(h, len)| (h, len.max(1))),
    };
    match decoded {
      Some((header, advance)) => {
        frames.push(header);
        position += advance;
      }
      None => break,
    }
  }

  Some(frames)
}

/// First valid header at-or-after a base offset where `num_required_frames`
/// consecutive ADTS *or* LOAS/LATM frames decode. Mirrors mkvtoolnix's
/// `find_consecutive_frames` + `parser_c::get_frame()` (it then reads the first
/// frame's header). Recognises both multiplex types, so LOAS/LATM-framed AAC —
/// the form MPEG-TS stream type `0x11` commonly carries — is covered as well as
/// ADTS, and a lone accidental ADTS-looking header is rejected.
pub fn find_first_header_with_frames(bytes: &[u8], num_required_frames: usize) -> Option<(usize, AacHeader)> {
  let base = find_consecutive_frames(bytes, num_required_frames)?;
  let remaining = &bytes[base..];
  if let Some(h) = decode_adts(remaining) {
    return Some((base, h));
  }
  if let Some((h, _)) = decode_loas_latm(remaining) {
    return Some((base, h));
  }
  None
}

/// First valid header at-or-after the consecutive-frame base offset. Returns
/// the `(offset, header)` pair.
fn find_first_valid_header(bytes: &[u8]) -> Option<(usize, AacHeader)> {
  find_first_header_with_frames(bytes, MIN_CONFIRM_FRAMES)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AacReader;

impl Reader for AacReader {
  fn name(&self) -> &'static str {
    "aac"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut probe)?;
    src.seek_to(0)?;
    if read < 9 {
      return Ok(false);
    }
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    Ok(find_consecutive_frames(&probe[start..read], MIN_CONFIRM_FRAMES).is_some())
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
    let (_offset, mut header) = find_first_valid_header(bytes).ok_or(ParseError::Unrecognised)?;

    // PARSER-215: raw AAC identification promotes ADTS headers with sample
    // rates up to 24 kHz to the SBR profile before emitting metadata
    // (`../mkvtoolnix/src/input/r_aac.cpp:73-76`).  `aac_reader_c::identify`
    // then reports `aac_is_sbr` from `profile == PROFILE_SBR`, so reflect the
    // promotion in both the profile and the SBR flag.
    if header.sample_rate > 0 && header.sample_rate <= 24_000 {
      header.profile = PROFILE_SBR;
      header.sbr = true;
    }

    out.container.format = ContainerFormat::Aac;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);

    let output_sampling_frequency = if header.output_sample_rate > 0 {
      Some(header.output_sample_rate as f64)
    } else {
      None
    };

    let codec_config = AudioCodecConfig {
      aac_object_type: Some(header.profile + 1),
      aac_frame_length: Some(header.samples_per_frame),
      aac_sbr_present: Some(header.sbr),
      aac_ps_present: Some(header.ps),
      ..AudioCodecConfig::default()
    };
    let audio = AudioTrackProperties {
      channels: if header.channels > 0 {
        Some(header.channels)
      } else {
        None
      },
      sampling_frequency: if header.sample_rate > 0 {
        Some(header.sample_rate as f64)
      } else {
        None
      },
      output_sampling_frequency,
      codec_config: Some(codec_config),
      ..AudioTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: "A_AAC".to_string(),
        name: Some(format_aac_profile(header.profile)),
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

pub(crate) fn format_aac_profile(profile: u32) -> String {
  match profile {
    0 => "AAC Main",
    1 => "AAC LC",
    2 => "AAC SSR",
    3 => "AAC LTP",
    4 => "AAC SBR",
    _ => "AAC",
  }
  .to_string()
}

// ---- test helpers ------------------------------------------------------

#[cfg(test)]
pub(crate) fn build_adts_frame(profile: u8, sr_index: u8, channel_config: u8) -> Vec<u8> {
  // 7-byte ADTS header + 1 byte body so frame_length = 8.
  build_adts_frame_with_len(profile, sr_index, channel_config, 8)
}

#[cfg(test)]
pub(crate) fn build_adts_frame_with_len(profile: u8, sr_index: u8, channel_config: u8, frame_length: u16) -> Vec<u8> {
  let mut bytes = vec![0u8; frame_length as usize];
  bytes[0] = 0xFF;
  bytes[1] = 0xF1; // sync + MPEG-4 + layer 0 + protection_absent
  bytes[2] = (profile << 6) | (sr_index << 2) | ((channel_config >> 2) & 0x01);
  bytes[3] = ((channel_config & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03);
  bytes[4] = ((frame_length >> 3) & 0xFF) as u8;
  bytes[5] = (((frame_length & 0x07) << 5) | 0x1F) as u8;
  bytes[6] = 0xFC;
  bytes
}

#[cfg(test)]
pub(crate) fn build_adts_stream(frames: usize, profile: u8, sr_index: u8, ch: u8) -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..frames {
    bytes.extend(build_adts_frame(profile, sr_index, ch));
  }
  bytes
}

/// Build a single LOAS/LATM frame carrying an AAC-LC AudioSpecificConfig with
/// the given sample-rate index and channel configuration. Crate-visible so the
/// MPEG-TS reader tests can synthesise LOAS/LATM streams (stream type 0x11).
#[cfg(test)]
pub(crate) fn build_loas_latm_frame(sr_index: u8, channel_config: u8, payload_data_len: usize) -> Vec<u8> {
  let mut w = TestBitWriter::new();
  w.put_bits(1, 0); // use_same_stream_mux = 0 -> parse stream mux config
  // --- StreamMuxConfig ---
  w.put_bits(1, 0); // audio_mux_version = 0
  w.put_bits(1, 0); // all_stream_same_time_framing
  w.put_bits(6, 0); // num_sub_frames
  w.put_bits(4, 0); // num_program
  w.put_bits(3, 0); // num_layer
  // --- AudioSpecificConfig (object_type LC) ---
  w.put_bits(5, AOT_AAC_LC as u64); // object_type
  w.put_bits(4, sr_index as u64); // sampling_frequency_index
  w.put_bits(4, channel_config as u64); // channel_configuration
  // GASpecificConfig
  w.put_bits(1, 0); // frame_length_flag
  w.put_bits(1, 0); // depends_on_core_coder
  w.put_bits(1, 0); // extension_flag
  // back in StreamMuxConfig:
  w.put_bits(3, 0); // frame_length_type = 0
  w.put_bits(8, payload_data_len as u64); // buffer_fullness
  w.put_bits(1, 0); // other_data_present = 0
  w.put_bits(1, 0); // crc_present = 0
  // --- PayloadLengthInfo (type 0): bytes summing to len ---
  let mut remaining = payload_data_len;
  loop {
    let chunk = remaining.min(255);
    w.put_bits(8, chunk as u64);
    remaining -= chunk;
    if chunk != 255 {
      break;
    }
  }
  // --- PayloadMux: the AAC payload bytes ---
  w.byte_align();
  let mut payload = w.into_bytes();
  payload.extend(vec![0u8; payload_data_len]);

  let loas_frame_size = payload.len();
  assert!(loas_frame_size <= LOAS_FRAME_SIZE_MASK as usize);

  // 3-byte LOAS header: 11-bit sync (0x2B7) + 13-bit frame size.
  let value: u32 = LOAS_SYNC_WORD | (loas_frame_size as u32 & LOAS_FRAME_SIZE_MASK);
  let mut frame = vec![
    ((value >> 16) & 0xFF) as u8,
    ((value >> 8) & 0xFF) as u8,
    (value & 0xFF) as u8,
  ];
  frame.extend(payload);
  frame
}

/// Concatenate `frames` LOAS/LATM frames into one stream.
#[cfg(test)]
pub(crate) fn build_loas_latm_stream(frames: usize, sr_index: u8, channel_config: u8) -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..frames {
    bytes.extend(build_loas_latm_frame(sr_index, channel_config, 16));
  }
  bytes
}

/// Minimal MSB-first bit writer used only by tests to synthesise PCE / LOAS
/// payloads. Kept here (not in io/) so the change stays inside aac.rs.
#[cfg(test)]
struct TestBitWriter {
  bytes: Vec<u8>,
  bit_count: u32, // bits used in the current (last) byte, 0..8
}

#[cfg(test)]
impl TestBitWriter {
  fn new() -> Self {
    Self {
      bytes: Vec::new(),
      bit_count: 0,
    }
  }

  fn put_bits(&mut self, n: u32, value: u64) {
    for i in (0..n).rev() {
      let bit = ((value >> i) & 1) as u8;
      if self.bit_count == 0 {
        self.bytes.push(0);
      }
      let last = self.bytes.last_mut().unwrap();
      *last |= bit << (7 - self.bit_count);
      self.bit_count = (self.bit_count + 1) % 8;
    }
  }

  fn byte_align(&mut self) {
    if self.bit_count != 0 {
      self.bit_count = 0;
    }
  }

  fn into_bytes(self) -> Vec<u8> {
    self.bytes
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn decode_adts_handles_lc_48k_stereo() {
    let frame = build_adts_frame(1, 3, 2);
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.profile, 1);
    assert_eq!(h.sample_rate, 48_000);
    assert_eq!(h.channels, 2);
    assert_eq!(h.bytes, 8);
  }

  #[test]
  fn decode_adts_handles_71_layout_via_channel_config_7() {
    let frame = build_adts_frame(1, 3, 7);
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.channels, 8);
  }

  #[test]
  fn decode_adts_rejects_invalid_sync() {
    let mut frame = build_adts_frame(1, 3, 2);
    frame[0] = 0xFE;
    assert!(decode_adts(&frame).is_none());
  }

  #[test]
  fn decode_adts_rejects_layer_nonzero() {
    let mut frame = build_adts_frame(1, 3, 2);
    frame[1] |= 0x04; // set layer bit
    assert!(decode_adts(&frame).is_none());
  }

  #[test]
  fn decode_adts_reserved_sr_index_yields_zero_rate() {
    // sr_index 13 is reserved -> SAMPLE_RATE_TABLE[13] == 0. mkvtoolnix
    // accepts the header but with a 0 sample rate; we mirror that.
    let mut frame = build_adts_frame(1, 3, 2);
    frame[2] = (1 << 6) | (13 << 2);
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.sample_rate, 0);
  }

  // ---- PARSER-008: channel_config == 0 + PCE -------------------------

  fn build_adts_frame_with_pce(profile: u8, sr_index: u8, front_cpe_pairs: u8) -> Vec<u8> {
    let mut w = TestBitWriter::new();
    // The raw_data_block starts with a 3-bit syntactic element id; PCE = 5.
    w.put_bits(3, ID_PCE);
    w.put_bits(4, 0); // element_instance_tag
    w.put_bits(2, 1); // object_type
    w.put_bits(4, sr_index as u64); // sampling_frequency_index
    w.put_bits(4, front_cpe_pairs as u64); // num_front_channel_elements
    w.put_bits(4, 0); // num_side
    w.put_bits(4, 0); // num_back
    w.put_bits(2, 0); // num_lfe
    w.put_bits(3, 0); // num_assoc_data
    w.put_bits(4, 0); // num_valid_cc
    w.put_bits(1, 0); // mono_mixdown_present
    w.put_bits(1, 0); // stereo_mixdown_present
    w.put_bits(1, 0); // matrix_mixdown_present
    for _ in 0..front_cpe_pairs {
      w.put_bits(1, 1); // front_element_is_cpe = 1 (stereo pair)
      w.put_bits(4, 0); // front_element_tag_select
    }
    w.byte_align();
    w.put_bits(8, 0); // comment_field_bytes = 0
    let pce = w.into_bytes();

    let frame_len = (7 + pce.len()) as u16;
    let mut frame = build_adts_frame_with_len(profile, sr_index, 0, frame_len);
    frame[7..7 + pce.len()].copy_from_slice(&pce);
    frame
  }

  #[test]
  fn decode_adts_channel_config_zero_uses_pce() {
    // 1 front CPE pair => 2 channels.
    let frame = build_adts_frame_with_pce(1, 3, 1);
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.channels, 2, "PCE-derived channel count");
    assert_eq!(h.sample_rate, 48_000);
  }

  #[test]
  fn decode_adts_channel_config_zero_three_pairs_is_six_channels() {
    // 3 front CPE pairs => 6 channels.
    let frame = build_adts_frame_with_pce(1, 3, 3);
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.channels, 6);
  }

  // ---- PARSER-009: consecutive-frame probe ---------------------------

  #[test]
  fn find_consecutive_frames_requires_eight_frames() {
    let bytes = build_adts_stream(8, 1, 3, 2);
    assert_eq!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES), Some(0));
  }

  #[test]
  fn find_consecutive_frames_skips_prefix_garbage() {
    let mut bytes = vec![0x00u8; 16];
    bytes.extend(build_adts_stream(8, 1, 3, 2));
    assert_eq!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES), Some(16));
  }

  #[test]
  fn find_consecutive_frames_rejects_single_isolated_header() {
    // A single ADTS-looking header followed by garbage must NOT match.
    let mut bytes = build_adts_frame(1, 3, 2);
    bytes.extend(vec![0x00u8; 64]);
    assert!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn find_consecutive_frames_rejects_two_frames_when_eight_required() {
    let bytes = build_adts_stream(2, 1, 3, 2);
    assert!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn probe_accepts_aac_stream() {
    let bytes = build_adts_stream(10, 1, 3, 2);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(AacReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_single_header() {
    let mut bytes = build_adts_frame(1, 3, 2);
    bytes.extend(vec![0x00u8; 64]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!AacReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_populates_track() {
    let bytes = build_adts_stream(10, 1, 3, 2);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.aac", 0);
    AacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Aac);
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.channels, Some(2));
    assert_eq!(audio.sampling_frequency, Some(48_000.0));
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_object_type, Some(2)); // LC = profile 1 + 1
    assert_eq!(cfg.aac_sbr_present, Some(false)); // 48 kHz not promoted
  }

  #[test]
  fn read_headers_promotes_low_rate_adts_to_sbr() {
    // PARSER-215: 16 kHz ADTS (sr_index 8) is identified as AAC SBR.
    let bytes = build_adts_stream(10, 1, 8, 2); // LC, 16 kHz, stereo
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.aac", 0);
    AacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_object_type, Some(PROFILE_SBR + 1)); // promoted to SBR
    assert_eq!(cfg.aac_sbr_present, Some(true));
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("AAC SBR"));
    // The core sampling frequency is still reported as-is.
    assert_eq!(audio.sampling_frequency, Some(16_000.0));
  }

  // ---- PARSER-007: LOAS/LATM -----------------------------------------

  /// Build one LOAS frame: 11-bit sync 0x2B7 + 13-bit frame size, then the
  /// AudioMuxElement payload, padded to `payload_data_len` bytes of data.
  fn build_loas_frame(sr_index: u8, channel_config: u8, payload_data_len: usize) -> Vec<u8> {
    let mut w = TestBitWriter::new();
    w.put_bits(1, 0); // use_same_stream_mux = 0 -> parse stream mux config
    // --- StreamMuxConfig ---
    w.put_bits(1, 0); // audio_mux_version = 0
    w.put_bits(1, 0); // all_stream_same_time_framing
    w.put_bits(6, 0); // num_sub_frames
    w.put_bits(4, 0); // num_program
    w.put_bits(3, 0); // num_layer
    // --- AudioSpecificConfig (object_type LC) ---
    w.put_bits(5, AOT_AAC_LC as u64); // object_type
    w.put_bits(4, sr_index as u64); // sampling_frequency_index
    w.put_bits(4, channel_config as u64); // channel_configuration
    // GASpecificConfig
    w.put_bits(1, 0); // frame_length_flag
    w.put_bits(1, 0); // depends_on_core_coder
    w.put_bits(1, 0); // extension_flag
    // back in StreamMuxConfig:
    w.put_bits(3, 0); // frame_length_type = 0
    w.put_bits(8, payload_data_len as u64); // buffer_fullness
    w.put_bits(1, 0); // other_data_present = 0
    w.put_bits(1, 0); // crc_present = 0
    // --- PayloadLengthInfo (type 0): bytes summing to len ---
    let mut remaining = payload_data_len;
    loop {
      let chunk = remaining.min(255);
      w.put_bits(8, chunk as u64);
      remaining -= chunk;
      if chunk != 255 {
        break;
      }
    }
    // --- PayloadMux: the AAC payload bytes ---
    w.byte_align();
    let mut payload = w.into_bytes();
    payload.extend(vec![0u8; payload_data_len]);

    let loas_frame_size = payload.len();
    assert!(loas_frame_size <= LOAS_FRAME_SIZE_MASK as usize);

    // 3-byte LOAS header: 11-bit sync (0x2B7) + 13-bit frame size.
    let value: u32 = LOAS_SYNC_WORD | (loas_frame_size as u32 & LOAS_FRAME_SIZE_MASK);
    let mut frame = vec![
      ((value >> 16) & 0xFF) as u8,
      ((value >> 8) & 0xFF) as u8,
      (value & 0xFF) as u8,
    ];
    frame.extend(payload);
    frame
  }

  #[test]
  fn decode_loas_latm_recovers_sample_rate_and_channels() {
    let frame = build_loas_frame(3, 2, 16); // 48 kHz, stereo
    let (h, advance) = decode_loas_latm(&frame).unwrap();
    assert!(h.is_valid);
    assert_eq!(h.sample_rate, 48_000);
    assert_eq!(h.channels, 2);
    assert_eq!(advance, frame.len());
  }

  #[test]
  fn decode_loas_latm_rejects_non_loas_sync() {
    let mut frame = build_loas_frame(3, 2, 16);
    frame[0] = 0x00; // break the sync word
    assert!(decode_loas_latm(&frame).is_none());
  }

  #[test]
  fn probe_accepts_loas_latm_stream() {
    let mut bytes = Vec::new();
    for _ in 0..(MIN_CONFIRM_FRAMES + 2) {
      bytes.extend(build_loas_frame(3, 2, 16));
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(AacReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_populates_loas_latm_track() {
    let mut bytes = Vec::new();
    for _ in 0..(MIN_CONFIRM_FRAMES + 2) {
      bytes.extend(build_loas_frame(4, 1, 24)); // 44.1 kHz, mono
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.aac", 0);
    AacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.sampling_frequency, Some(44_100.0));
    assert_eq!(audio.channels, Some(1));
  }

  #[test]
  fn format_aac_profile_table() {
    assert_eq!(format_aac_profile(0), "AAC Main");
    assert_eq!(format_aac_profile(1), "AAC LC");
    assert_eq!(format_aac_profile(2), "AAC SSR");
    assert_eq!(format_aac_profile(3), "AAC LTP");
    assert_eq!(format_aac_profile(4), "AAC SBR");
    assert_eq!(format_aac_profile(7), "AAC");
  }

  // ---- extra coverage: ADTS edge cases -------------------------------

  #[test]
  fn decode_adts_with_crc_protection_present() {
    // protection_absent = 0 adds a 16-bit CRC after the fixed header, so
    // the header is 9 bytes; the frame must be larger than that.
    let frame_len: u16 = 16;
    let mut frame = build_adts_frame_with_len(1, 3, 2, frame_len);
    frame[1] = 0xF0; // clear protection_absent (bit 0 of byte 1)
    let h = decode_adts(&frame).unwrap();
    assert_eq!(h.channels, 2);
    assert_eq!(h.bytes, 16);
  }

  #[test]
  fn decode_adts_rejects_frame_length_smaller_than_header() {
    // frame_length 7 with CRC present => header is 9 bytes > 7 => reject.
    let mut frame = build_adts_frame_with_len(1, 3, 2, 7);
    frame[1] = 0xF0; // protection present
    assert!(decode_adts(&frame).is_none());
  }

  #[test]
  fn decode_adts_rejects_frame_length_beyond_buffer() {
    // Claim a 4096-byte frame but only supply 8 bytes => need-more-data.
    let frame = build_adts_frame_with_len(1, 3, 2, 4096);
    assert!(decode_adts(&frame[..8]).is_none());
  }

  #[test]
  fn decode_adts_truncated_returns_none() {
    let frame = build_adts_frame(1, 3, 2);
    assert!(decode_adts(&frame[..3]).is_none());
  }

  // ---- extra coverage: AudioSpecificConfig parsing -------------------

  /// Decode a bare AudioSpecificConfig (used to exercise object-type /
  /// sample-rate edge cases without the LOAS framing).
  fn parse_asc(bytes: &[u8], look_for_sync_extension: bool) -> Option<AacHeader> {
    let mut bc = Bits::new(bytes);
    parse_audio_specific_config(&mut bc, look_for_sync_extension)
  }

  #[test]
  fn asc_explicit_sbr_object_type() {
    // object_type 5 (SBR): 5 bits = 00101, output sr_index 4 bits,
    // then re-read object_type LC, sr_index, channel_config, GA flags.
    let mut w = TestBitWriter::new();
    w.put_bits(5, AOT_SBR as u64); // object_type SBR
    w.put_bits(4, 3); // sample_rate index 48 kHz (core)
    w.put_bits(4, 2); // channel_config stereo
    w.put_bits(4, 3); // output sample rate index (read_sample_rate)
    w.put_bits(5, AOT_AAC_LC as u64); // inner object_type LC
    // GASpecificConfig:
    w.put_bits(1, 0); // frame_length_flag
    w.put_bits(1, 0); // depends_on_core_coder
    w.put_bits(1, 0); // extension_flag
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, false).unwrap();
    assert!(h.sbr);
    assert_eq!(h.channels, 2);
  }

  #[test]
  fn asc_ps_object_type_treated_as_sbr_extension() {
    // PARSER-214: object_type 29 (Parametric Stereo / HE-AACv2) is decoded as
    // an SBR-style extension — output sample rate is read and the inner object
    // type follows, and the PS flag is set.
    let mut w = TestBitWriter::new();
    w.put_bits(5, AOT_PS as u64); // object_type PS
    w.put_bits(4, 3); // core sample rate index 48 kHz
    w.put_bits(4, 2); // channel_config stereo
    w.put_bits(4, 3); // output sample rate index (read_sample_rate)
    w.put_bits(5, AOT_AAC_LC as u64); // inner object_type LC
    w.put_bits(1, 0); // GA frame_length_flag
    w.put_bits(1, 0); // depends_on_core_coder
    w.put_bits(1, 0); // extension_flag
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, false).unwrap();
    assert!(h.sbr);
    assert!(h.ps);
    assert_eq!(h.output_sample_rate, 48_000);
    assert_eq!(h.channels, 2);
    // profile stays object_type(PS) - 1, matching mkvtoolnix.
    assert_eq!(h.profile, AOT_PS - 1);
    // The shared codec config surfaces aac_ps_present.
    assert_eq!(codec_config_from_header(&h, &asc).aac_ps_present, Some(true));
  }

  #[test]
  fn asc_non_ps_reports_ps_absent() {
    let mut w = TestBitWriter::new();
    w.put_bits(5, AOT_AAC_LC as u64);
    w.put_bits(4, 3); // 48 kHz
    w.put_bits(4, 2); // stereo
    w.put_bits(1, 0);
    w.put_bits(1, 0);
    w.put_bits(1, 0);
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, false).unwrap();
    assert!(!h.ps);
    assert_eq!(codec_config_from_header(&h, &asc).aac_ps_present, Some(false));
  }

  #[test]
  fn asc_escape_object_type_and_sample_rate() {
    // object_type escape: 5 bits = 31 then 6 more bits -> 32 + n.
    // sample_rate escape: index 0x0f then 24 explicit bits.
    let mut w = TestBitWriter::new();
    w.put_bits(5, 31); // escape
    w.put_bits(6, 0); // -> object_type 32 (USAC etc.) -> not GA
    w.put_bits(4, 0x0f); // sample-rate escape
    w.put_bits(24, 44_100); // explicit sample rate
    w.put_bits(4, 2); // channel_config stereo
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, false).unwrap();
    assert_eq!(h.sample_rate, 44_100);
    assert_eq!(h.channels, 2);
    assert_eq!(h.profile, 31); // object_type(32) - 1
  }

  #[test]
  fn asc_implicit_sbr_for_low_sample_rate() {
    // 24 kHz LC -> mkvtoolnix promotes to implicit SBR with doubled rate.
    let mut w = TestBitWriter::new();
    w.put_bits(5, AOT_AAC_LC as u64);
    w.put_bits(4, 6); // index 6 -> 24 kHz
    w.put_bits(4, 2); // stereo
    w.put_bits(1, 0); // frame_length_flag
    w.put_bits(1, 0); // depends_on_core_coder
    w.put_bits(1, 0); // extension_flag
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, false).unwrap();
    assert!(h.sbr);
    assert_eq!(h.output_sample_rate, 48_000);
  }

  #[test]
  fn asc_sync_extension_enables_sbr() {
    // LC ASC followed by the 0x2B7 sync extension signalling SBR.
    let mut w = TestBitWriter::new();
    w.put_bits(5, AOT_AAC_LC as u64);
    w.put_bits(4, 3); // 48 kHz
    w.put_bits(4, 2); // stereo
    w.put_bits(1, 0); // frame_length_flag
    w.put_bits(1, 0); // depends_on_core_coder
    w.put_bits(1, 0); // extension_flag
    // sync extension:
    w.put_bits(11, SYNC_EXTENSION_TYPE);
    w.put_bits(5, AOT_SBR as u64); // extension object type SBR
    w.put_bits(1, 1); // sbr_present_flag
    w.put_bits(4, 3); // extension sample-rate index
    w.byte_align();
    let asc = w.into_bytes();
    let h = parse_asc(&asc, true).unwrap();
    assert!(h.sbr);
    assert_eq!(h.channels, 2);
  }

  #[test]
  fn asc_rejects_object_type_zero() {
    let asc = [0x00u8, 0x00];
    assert!(parse_asc(&asc, false).is_none());
  }

  // ---- extra coverage: LATM stream-mux-config branches ---------------

  /// Build a LOAS frame with a fully configurable StreamMuxConfig so the
  /// less-common frame-length types and trailing flags get exercised.
  #[allow(clippy::too_many_arguments)]
  fn build_loas_frame_cfg(
    audio_mux_version: u8,
    frame_length_type: u8,
    fixed_frame_length: u16,
    other_data: bool,
    crc_present: bool,
    payload_data_len: usize,
  ) -> Vec<u8> {
    let mut w = TestBitWriter::new();
    w.put_bits(1, 0); // use_same_stream_mux = 0
    // --- StreamMuxConfig ---
    w.put_bits(1, audio_mux_version as u64);
    if audio_mux_version == 1 {
      w.put_bits(1, 0); // audio_mux_version_a = 0
      // get_value(): 2-bit byte count + bytes (tara_buffer_fullness).
      w.put_bits(2, 0); // 1 byte follows
      w.put_bits(8, 0);
    }
    w.put_bits(1, 0); // all_stream_same_time_framing
    w.put_bits(6, 0); // num_sub_frames
    w.put_bits(4, 0); // num_program
    w.put_bits(3, 0); // num_layer

    // AudioSpecificConfig (LC stereo 48 kHz). For audio_mux_version 1 it is
    // length-prefixed via get_value().
    let mut asc = TestBitWriter::new();
    asc.put_bits(5, AOT_AAC_LC as u64);
    asc.put_bits(4, 3); // 48 kHz
    asc.put_bits(4, 2); // stereo
    asc.put_bits(1, 0); // frame_length_flag
    asc.put_bits(1, 0); // depends_on_core_coder
    asc.put_bits(1, 0); // extension_flag
    let asc_bits: u64 = 5 + 4 + 4 + 1 + 1 + 1;

    if audio_mux_version == 1 {
      // get_value() for asc_length in *bits*.
      w.put_bits(2, 0); // 1 byte
      w.put_bits(8, asc_bits);
    }
    // Inline the ASC bits.
    for_each_bit(&asc, asc_bits, &mut w);

    // frame_length_type
    w.put_bits(3, frame_length_type as u64);
    match frame_length_type {
      0 => w.put_bits(8, 0), // buffer_fullness
      1 => w.put_bits(9, fixed_frame_length as u64),
      3 | 4 | 5 => w.put_bits(6, 0),
      6 | 7 => w.put_bits(1, 0),
      _ => {}
    }

    // other_data
    w.put_bits(1, other_data as u64);
    if other_data {
      if audio_mux_version == 1 {
        w.put_bits(2, 0);
        w.put_bits(8, 0);
      } else {
        // single escape-terminated byte
        w.put_bits(1, 0); // escape = 0 -> stop
        w.put_bits(8, 0);
      }
    }

    // crc_present
    w.put_bits(1, crc_present as u64);
    if crc_present {
      w.put_bits(8, 0); // config_crc
    }

    // PayloadLengthInfo
    match frame_length_type {
      0 => {
        let mut remaining = payload_data_len;
        loop {
          let chunk = remaining.min(255);
          w.put_bits(8, chunk as u64);
          remaining -= chunk;
          if chunk != 255 {
            break;
          }
        }
      }
      3 | 5 | 7 => {
        w.put_bits(2, payload_data_len as u64);
      }
      _ => {} // type 1: fixed; payload length is fixed_frame_length
    }

    w.byte_align();
    let mut payload = w.into_bytes();
    let data_len = if frame_length_type == 1 {
      fixed_frame_length as usize
    } else {
      payload_data_len
    };
    payload.extend(vec![0u8; data_len]);

    let loas_frame_size = payload.len();
    let value: u32 = LOAS_SYNC_WORD | (loas_frame_size as u32 & LOAS_FRAME_SIZE_MASK);
    let mut frame = vec![
      ((value >> 16) & 0xFF) as u8,
      ((value >> 8) & 0xFF) as u8,
      (value & 0xFF) as u8,
    ];
    frame.extend(payload);
    frame
  }

  /// Copy the first `n_bits` of `src` into `dst` bit-for-bit.
  fn for_each_bit(src: &TestBitWriter, n_bits: u64, dst: &mut TestBitWriter) {
    let bytes = &src.bytes;
    for i in 0..n_bits {
      let byte = bytes[(i / 8) as usize];
      let bit = (byte >> (7 - (i % 8))) & 1;
      dst.put_bits(1, bit as u64);
    }
  }

  #[test]
  fn decode_loas_latm_fixed_frame_length_type() {
    let frame = build_loas_frame_cfg(0, 1, 12, false, false, 0);
    let (h, advance) = decode_loas_latm(&frame).unwrap();
    assert_eq!(h.sample_rate, 48_000);
    assert_eq!(h.channels, 2);
    assert_eq!(advance, frame.len());
  }

  #[test]
  fn decode_loas_latm_celp_frame_length_type() {
    // frame_length_type 3 (CELP) -> 2-bit payload length.
    let frame = build_loas_frame_cfg(0, 3, 0, false, false, 1);
    let (h, _advance) = decode_loas_latm(&frame).unwrap();
    assert_eq!(h.channels, 2);
  }

  #[test]
  fn decode_loas_latm_with_other_data_and_crc() {
    let frame = build_loas_frame_cfg(0, 0, 0, true, true, 8);
    let (h, _advance) = decode_loas_latm(&frame).unwrap();
    assert_eq!(h.sample_rate, 48_000);
  }

  #[test]
  fn decode_loas_latm_audio_mux_version_one() {
    let frame = build_loas_frame_cfg(1, 0, 0, false, false, 8);
    let (h, _advance) = decode_loas_latm(&frame).unwrap();
    assert_eq!(h.sample_rate, 48_000);
    assert_eq!(h.channels, 2);
  }

  #[test]
  fn decode_loas_latm_rejects_use_same_stream_mux() {
    // A frame whose AudioMuxElement sets use_same_stream_mux = 1 cannot be
    // resolved by a single-frame decode.
    let mut w = TestBitWriter::new();
    w.put_bits(1, 1); // use_same_stream_mux = 1
    w.byte_align();
    let payload = w.into_bytes();
    let loas_frame_size = payload.len();
    let value: u32 = LOAS_SYNC_WORD | (loas_frame_size as u32 & LOAS_FRAME_SIZE_MASK);
    let mut frame = vec![
      ((value >> 16) & 0xFF) as u8,
      ((value >> 8) & 0xFF) as u8,
      (value & 0xFF) as u8,
    ];
    frame.extend(payload);
    assert!(decode_loas_latm(&frame).is_none());
  }

  #[test]
  fn decode_loas_latm_needs_more_data_when_truncated() {
    let frame = build_loas_frame(3, 2, 16);
    // Claim full size but cut the buffer short of loas_frame_end.
    assert!(decode_loas_latm(&frame[..4]).is_none());
    assert!(decode_loas_latm(&frame[..2]).is_none());
  }

  // ---- extra coverage: find_consecutive_frames guards ----------------

  #[test]
  fn find_consecutive_frames_too_small_buffer() {
    assert!(find_consecutive_frames(&[0xFF, 0xF1, 0x00], MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn find_consecutive_frames_loas_zero_size_shortcut() {
    // LOAS sync with a zero frame size must be skipped by the shortcut.
    let mut bytes = vec![0x56u8, 0xE0, 0x00]; // sync, frame_size 0
    bytes.extend(vec![0x00u8; 32]);
    assert!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn find_consecutive_frames_adts_oversized_shortcut() {
    // ADTS sync but the claimed frame size runs past the buffer.
    let frame = build_adts_frame_with_len(1, 3, 2, 4096);
    let mut bytes = frame[..9].to_vec();
    bytes.extend(vec![0x00u8; 16]);
    assert!(find_consecutive_frames(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn collect_frames_returns_none_for_garbage() {
    let bytes = vec![0x12u8; 64];
    assert!(collect_frames(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }
}
