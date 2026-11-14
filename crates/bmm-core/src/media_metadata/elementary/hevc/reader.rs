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

//! Top-level `HevcReader`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
  ChromaFormat, Dimensions2D, HevcTier as ModelHevcTier, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::mp4::codec_specific::hex_encode;
use crate::media_metadata::reader::Reader;

use super::nal::{
  self, NAL_UNIT_TYPE_AUD, NAL_UNIT_TYPE_END_OF_SEQ, NAL_UNIT_TYPE_END_OF_STREAM, NAL_UNIT_TYPE_FILLER,
  NAL_UNIT_TYPE_PPS, NAL_UNIT_TYPE_PREFIX_SEI, NAL_UNIT_TYPE_SPS, NAL_UNIT_TYPE_SUFFIX_SEI, NAL_UNIT_TYPE_VPS,
};
use super::sps::{self, HevcTier};
use super::vps;

const PROBE_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_PROBE_CHUNKS: usize = 50;

#[derive(Debug, Default, Clone, Copy)]
pub struct HevcReader;

impl HevcReader {
  pub(crate) fn probe_strict(src: &mut FileSource) -> Result<bool, ParseError> {
    let buf = read_probe_prefix(src, None)?;
    Ok(probe_annex_b(&buf, true))
  }

  fn probe_all(src: &mut FileSource) -> Result<bool, ParseError> {
    let buf = read_probe_prefix(src, None)?;
    Ok(probe_annex_b(&buf, false))
  }
}

impl Reader for HevcReader {
  fn name(&self) -> &'static str {
    "hevc"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    Self::probe_all(src)
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let buf = read_probe_prefix(src, Some(deadline))?;
    if starts_with_mpeg_ts_sync(&buf) {
      return Err(ParseError::Unrecognised);
    }
    let units = nal::split_nal_units(&buf);
    let headers = extract_headers(&units).ok_or(ParseError::Unrecognised)?;
    let sps_unit = headers.sps.ok_or(ParseError::Unrecognised)?;
    let rbsp = nal::strip_emulation_prevention(sps_unit.payload);
    let sps = sps::parse(&rbsp)?;
    let codec_private = headers.codec_private(&sps);

    out.container.format = ContainerFormat::Hevc;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    // PARSER-240: pixel dimensions are the cropped luma size; display
    // dimensions apply the VUI sample aspect ratio when one is present.
    let (display_width, display_height) = sps.display_dimensions();
    let video = VideoTrackProperties {
      pixel_dimensions: Some(Dimensions2D {
        width: sps.display_width,
        height: sps.display_height,
      }),
      display_dimensions: Some(Dimensions2D {
        width: display_width,
        height: display_height,
      }),
      default_duration_ns: sps.default_duration_ns,
      codec_config: Some(VideoCodecConfig {
        profile_idc: Some(sps.profile_idc as u32),
        profile_name: Some(sps::format_profile(sps.profile_idc).to_string()),
        level_idc: Some(sps.level_idc as u32),
        level_name: Some(sps::format_level(sps.level_idc)),
        tier: Some(match sps.tier {
          HevcTier::Main => ModelHevcTier::Main,
          HevcTier::High => ModelHevcTier::High,
        }),
        chroma_format: Some(map_chroma(sps.chroma_format_idc)),
        bit_depth_luma: Some(sps.bit_depth_luma as u32),
        bit_depth_chroma: Some(sps.bit_depth_chroma as u32),
        coded_dimensions: Some(Dimensions2D {
          width: sps.coded_width,
          height: sps.coded_height,
        }),
        raw_hex: Some(hex_encode(&codec_private)),
        is_elementary_stream: Some(true),
        ..VideoCodecConfig::default()
      }),
      ..VideoTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_MPEGH/ISO/HEVC".to_string(),
        name: Some("HEVC/H.265".to_string()),
        codec_private: Some(CodecPrivate::from_bytes(&codec_private)),
      },
      properties: TrackProperties {
        common,
        video: Some(video),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

fn read_probe_prefix(src: &mut FileSource, deadline: Option<&Deadline>) -> Result<Vec<u8>, ParseError> {
  src.seek_to(0)?;
  let mut out = Vec::new();
  let mut chunk = vec![0u8; PROBE_CHUNK_BYTES];
  for _ in 0..MAX_PROBE_CHUNKS {
    if let Some(d) = deadline {
      d.check("hevc-probe")?;
    }
    let read = src.read_at_most(&mut chunk)?;
    if read == 0 {
      break;
    }
    out.extend_from_slice(&chunk[..read]);
    if read < PROBE_CHUNK_BYTES {
      break;
    }
    let units = nal::split_nal_units(&out);
    if extract_headers(&units).is_some() {
      break;
    }
  }
  src.seek_to(0)?;
  Ok(out)
}

fn probe_annex_b(buf: &[u8], require_headers_at_start: bool) -> bool {
  if buf.len() < 7 || starts_with_mpeg_ts_sync(buf) {
    return false;
  }
  if require_headers_at_start && !starts_with_annex_b_start_code(buf) {
    return false;
  }
  let units = nal::split_nal_units(buf);
  extract_headers(&units).is_some()
}

fn starts_with_mpeg_ts_sync(buf: &[u8]) -> bool {
  buf.first() == Some(&0x47)
}

fn starts_with_annex_b_start_code(buf: &[u8]) -> bool {
  buf.starts_with(&[0x00, 0x00, 0x01]) || buf.starts_with(&[0x00, 0x00, 0x00, 0x01])
}

pub(crate) fn codec_private_from_annex_b(buf: &[u8]) -> Option<(Vec<u8>, sps::HevcSps)> {
  let units = nal::split_nal_units(buf);
  let headers = extract_headers(&units)?;
  let sps_unit = headers.sps?;
  let rbsp = nal::strip_emulation_prevention(sps_unit.payload);
  let sps = sps::parse(&rbsp).ok()?;
  let codec_private = headers.codec_private(&sps);
  Some((codec_private, sps))
}

#[derive(Debug, Clone, Copy)]
struct HevcHeaders<'a> {
  vps: Option<nal::HevcNalUnit<'a>>,
  sps: Option<nal::HevcNalUnit<'a>>,
  pps: Option<nal::HevcNalUnit<'a>>,
}

impl<'a> HevcHeaders<'a> {
  fn codec_private(&self, sps: &sps::HevcSps) -> Vec<u8> {
    let arrays = [
      (NAL_UNIT_TYPE_VPS, self.vps.expect("validated VPS")),
      (NAL_UNIT_TYPE_SPS, self.sps.expect("validated SPS")),
      (NAL_UNIT_TYPE_PPS, self.pps.expect("validated PPS")),
    ];
    // HEVCDecoderConfigurationRecord byte layout — port of `hevcc_c::pack`
    // (`../mkvtoolnix/src/common/hevc/hevcc.cpp:293-352`).  The reserved high
    // bits MUST be filled (`0x0f`/`0x3f`/`0x1f`) and chroma precedes the
    // bit-depth bytes (PARSER-255).
    let mut out = vec![0u8; 23];
    out[0] = 1; // configurationVersion
    // byte 1: general_profile_space(2) | general_tier_flag(1) | profile_idc(5)
    let tier_bit: u8 = if sps.tier == HevcTier::High { 1 } else { 0 };
    out[1] = (sps.profile_space << 6) | (tier_bit << 5) | (sps.profile_idc & 0x1f);
    // bytes 2-5: general_profile_compatibility_flag (32 bits)
    out[2..6].copy_from_slice(&sps.profile_compatibility_flag.to_be_bytes());
    // byte 6 (top nibble): progressive / interlaced / non-packed / frame-only
    // constraint flags; the low nibble + bytes 7-11 are the 44 reserved bits.
    out[6] = (u8::from(sps.progressive_source_flag) << 7)
      | (u8::from(sps.interlaced_source_flag) << 6)
      | (u8::from(sps.non_packed_constraint_flag) << 5)
      | (u8::from(sps.frame_only_constraint_flag) << 4);
    out[12] = sps.level_idc;
    // byte 13: reserved 4 bits (1111) | min_spatial_segmentation_idc high bits
    // (12-bit value from the VUI bitstream-restriction block).
    let min_spatial = sps.min_spatial_segmentation_idc.min(0x0fff);
    out[13] = 0xf0 | ((min_spatial >> 8) as u8 & 0x0f);
    // byte 14: min_spatial_segmentation_idc low byte.
    out[14] = min_spatial as u8;
    // byte 15: reserved 6 bits (111111) | parallelism_type(2).
    out[15] = 0xfc | (sps.parallelism_type & 0x03);
    // byte 16: reserved 6 bits (111111) | chroma_format_idc(2).
    out[16] = 0xfc | (sps.chroma_format_idc & 0x03);
    // byte 17: reserved 5 bits (11111) | bit_depth_luma_minus8(3).
    out[17] = 0xf8 | sps.bit_depth_luma.saturating_sub(8);
    // byte 18: reserved 5 bits (11111) | bit_depth_chroma_minus8(3).
    out[18] = 0xf8 | sps.bit_depth_chroma.saturating_sub(8);
    // bytes 19-20: avgFrameRate / reserved = 0.
    // byte 21: reserved(2)=0 | numTemporalLayers(3) | temporalIdNested(1) |
    //          lengthSizeMinusOne(2) = 3 (4-byte NAL length).
    out[21] = (((sps.max_sub_layers_minus1 as u8).wrapping_add(1) & 0x07) << 3)
      | (u8::from(sps.temporal_id_nesting_flag) << 2)
      | 0x03;
    out[22] = arrays.len() as u8;
    for (nal_type, unit) in arrays {
      let bytes = nal_bytes(unit);
      out.push(0x80 | (nal_type & 0x3f));
      out.extend_from_slice(&1u16.to_be_bytes());
      out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
      out.extend_from_slice(&bytes);
    }
    out
  }
}

fn extract_headers<'a>(units: &'a [nal::HevcNalUnit<'a>]) -> Option<HevcHeaders<'a>> {
  let mut headers = HevcHeaders {
    vps: None,
    sps: None,
    pps: None,
  };
  let mut configuration_record_ready = false;
  let mut first_access_unit_parsing_slices = false;
  let mut first_access_unit_parsed = false;
  for unit in units {
    if hevc_flushes_incomplete_frame(unit.nal_unit_type) && first_access_unit_parsing_slices {
      first_access_unit_parsed = true;
    }
    if unit.nal_unit_type == NAL_UNIT_TYPE_VPS && headers.vps.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if vps::parse(&rbsp).is_ok() {
        headers.vps = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_SPS && headers.sps.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if sps::parse(&rbsp).is_ok_and(|sps| sps.display_width > 0 && sps.display_height > 0) {
        headers.sps = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_PPS && headers.pps.is_none() {
      headers.pps = Some(*unit);
    }
    let headers_ready = headers.vps.is_some() && headers.sps.is_some() && headers.pps.is_some();
    if headers_ready && hevc_sets_configuration_record_ready(unit.nal_unit_type) {
      configuration_record_ready = true;
    }
    if hevc_is_vcl(unit.nal_unit_type) {
      first_access_unit_parsing_slices = true;
    }
  }
  if configuration_record_ready && first_access_unit_parsed {
    Some(headers)
  } else {
    None
  }
}

fn hevc_is_vcl(nal_unit_type: u8) -> bool {
  nal_unit_type <= 31
}

fn hevc_flushes_incomplete_frame(nal_unit_type: u8) -> bool {
  match nal_unit_type {
    NAL_UNIT_TYPE_VPS
    | NAL_UNIT_TYPE_SPS
    | NAL_UNIT_TYPE_PPS
    | NAL_UNIT_TYPE_AUD
    | NAL_UNIT_TYPE_END_OF_STREAM
    | NAL_UNIT_TYPE_PREFIX_SEI => true,
    NAL_UNIT_TYPE_END_OF_SEQ | NAL_UNIT_TYPE_FILLER | NAL_UNIT_TYPE_SUFFIX_SEI => false,
    45..=47 | 56..=63 => false,
    _ => !hevc_is_vcl(nal_unit_type),
  }
}

fn hevc_sets_configuration_record_ready(nal_unit_type: u8) -> bool {
  hevc_is_vcl(nal_unit_type)
    || !matches!(
      nal_unit_type,
      NAL_UNIT_TYPE_VPS
        | NAL_UNIT_TYPE_SPS
        | NAL_UNIT_TYPE_PPS
        | NAL_UNIT_TYPE_AUD
        | NAL_UNIT_TYPE_END_OF_SEQ
        | NAL_UNIT_TYPE_END_OF_STREAM
        | NAL_UNIT_TYPE_FILLER
        | NAL_UNIT_TYPE_PREFIX_SEI
        | NAL_UNIT_TYPE_SUFFIX_SEI
    ) && !matches!(nal_unit_type, 45..=47 | 56..=63)
}

fn nal_bytes(unit: nal::HevcNalUnit<'_>) -> Vec<u8> {
  let mut bytes = Vec::with_capacity(unit.payload.len() + 2);
  bytes.push((unit.nal_unit_type & 0x3f) << 1 | (unit.layer_id >> 5));
  bytes.push(((unit.layer_id & 0x1f) << 3) | (unit.temporal_id_plus1 & 0x07));
  bytes.extend_from_slice(unit.payload);
  bytes
}

fn map_chroma(idc: u8) -> ChromaFormat {
  match idc {
    0 => ChromaFormat::Monochrome,
    1 => ChromaFormat::Yuv420,
    2 => ChromaFormat::Yuv422,
    3 => ChromaFormat::Yuv444,
    _ => ChromaFormat::Other,
  }
}

#[cfg(test)]
pub(crate) fn build_test_main10_stream() -> Vec<u8> {
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

  fn write_simple_tail(w: &mut BitWriter) {
    w.write_ue(4);
    w.write_bit(false);
    for _ in 0..9 {
      w.write_ue(0);
    }
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_ue(0);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
  }

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
  write_simple_tail(&mut w);
  let sps = w.into_bytes();

  let mut bytes = Vec::new();
  bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x40, 0x01]);
  bytes.extend_from_slice(&[0b0000_1100, 0b0000_0100, 0x80]);
  bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x42, 0x01]);
  bytes.extend(sps);
  bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x44, 0x01, 0x80]);
  bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0x80]);
  bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x46, 0x01, 0x50]);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  /// Construct an HEVC elementary stream with VPS + SPS (Main 10, 1920x1080).
  fn build_main10_stream() -> Vec<u8> {
    build_main10_stream_with_sps(build_main10_1080p_sps_rbsp())
  }

  fn build_main10_stream_with_timing() -> Vec<u8> {
    build_main10_stream_with_sps(build_main10_1080p_sps_rbsp_with_timing())
  }

  fn build_main10_stream_with_sps(sps: Vec<u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    // VPS NAL (type 32)
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x40, 0x01]); // header (type=32, layer=0, temp_id=1)
    bytes.extend_from_slice(&[0b0000_1100, 0b0000_0100, 0x80]);
    // SPS NAL (type 33)
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x42, 0x01]);
    bytes.extend(sps);
    // PPS NAL (type 34)
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x44, 0x01, 0x80]);
    // VCL NAL (type 19) followed by an AUD boundary so parser acceptance
    // mirrors headers_parsed() requiring a flushed first access unit.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0x80]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x46, 0x01, 0x50]);
    bytes
  }

  fn build_main10_stream_with_aud_only_after_parameter_sets() -> Vec<u8> {
    let mut bytes = build_main10_stream();
    let vcl_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x26]).unwrap();
    let aud_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x46]).unwrap();
    bytes.drain(vcl_pos..aud_pos);
    bytes
  }

  fn build_main10_1080p_sps_rbsp() -> Vec<u8> {
    build_main10_1080p_sps_rbsp_with_crop(None)
  }

  fn build_main10_1080p_sps_rbsp_with_crop(crop_bottom: Option<u32>) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 4);
    w.write_bits(0, 3);
    w.write_bit(true);
    w.write_bits(0, 2);
    w.write_bit(false); // main tier
    w.write_bits(2, 5); // profile = Main 10
    w.write_bits(0, 32);
    w.write_bits(0, 48);
    w.write_bits(120, 8); // level 4.0
    w.write_ue(0); // sps_seq_parameter_set_id
    w.write_ue(1); // chroma_format_idc
    w.write_ue(1920); // pic_width_in_luma_samples
    w.write_ue(1080); // pic_height_in_luma_samples
    if let Some(bottom) = crop_bottom {
      w.write_bit(true); // conformance_window_flag
      w.write_ue(0); // crop_left
      w.write_ue(0); // crop_right
      w.write_ue(0); // crop_top
      w.write_ue(bottom);
    } else {
      w.write_bit(false); // conformance_window_flag
    }
    w.write_ue(2); // bit_depth_luma_minus8
    w.write_ue(2); // bit_depth_chroma_minus8
    write_simple_tail(&mut w, false);
    w.into_bytes()
  }

  fn build_main10_1080p_sps_rbsp_with_timing() -> Vec<u8> {
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
    write_simple_tail(&mut w, true);
    w.into_bytes()
  }

  fn write_simple_tail(w: &mut BitWriter, include_timing: bool) {
    w.write_ue(4);
    w.write_bit(false);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_ue(0);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(include_timing); // VUI present
    if include_timing {
      w.write_bit(false); // aspect ratio
      w.write_bit(false); // overscan
      w.write_bit(false); // video signal
      w.write_bit(false); // chroma loc
      w.write_bit(false); // neutral chroma
      w.write_bit(false); // field seq
      w.write_bit(false); // frame field info
      w.write_bit(false); // default display window
      w.write_bit(true); // timing info
      w.write_bits(1, 32);
      w.write_bits(30, 32);
      w.write_bit(false); // poc proportional timing
      w.write_bit(false); // hrd parameters
      w.write_bit(false); // bitstream restriction
    }
  }

  fn build_main10_1080p_sps_rbsp_with_bitstream_restriction() -> Vec<u8> {
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
    write_tail_with_bitstream_restriction(&mut w, 0x123);
    w.into_bytes()
  }

  fn write_tail_with_bitstream_restriction(w: &mut BitWriter, min_spatial: u32) {
    w.write_ue(4);
    w.write_bit(false);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_ue(0);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(false);
    w.write_bit(true); // VUI present
    w.write_bit(false); // aspect ratio
    w.write_bit(false); // overscan
    w.write_bit(false); // video signal
    w.write_bit(false); // chroma loc
    w.write_bit(false); // neutral chroma
    w.write_bit(false); // field seq
    w.write_bit(false); // frame field info
    w.write_bit(false); // default display window
    w.write_bit(false); // timing info
    w.write_bit(true); // bitstream restriction
    w.write_bit(true); // tiles_fixed_structure_flag
    w.write_bit(true); // motion_vectors_over_pic_boundaries_flag
    w.write_bit(false); // restricted_ref_pic_lists_flag
    w.write_ue(min_spatial);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
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
  fn probe_accepts_stream_with_vps_or_sps() {
    let bytes = build_main10_stream();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(HevcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_main10_dims() {
    let bytes = build_main10_stream();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.hevc", 0);
    HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    let cfg = v.codec_config.as_ref().unwrap();
    assert_eq!(cfg.profile_idc, Some(2));
    assert_eq!(cfg.level_name.as_deref(), Some("4.0"));
    assert_eq!(cfg.bit_depth_luma, Some(10));
    assert!(cfg.raw_hex.as_ref().unwrap().starts_with("01"));
    assert!(out.tracks[0].codec.codec_private.is_some());
  }

  #[test]
  fn probe_and_read_headers_find_headers_after_64k_prefix() {
    let mut bytes = vec![0x00u8; 70 * 1024];
    bytes.extend(build_main10_stream());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(HevcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(!HevcReader::probe_strict(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("late.hevc", 0);
    HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Hevc);
    assert_eq!(out.tracks.len(), 1);
  }

  #[test]
  fn read_headers_extracts_vui_timing_duration() {
    let bytes = build_main10_stream_with_timing();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.hevc", 0);
    HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.default_duration_ns, Some(33_333_333));
  }

  // PARSER-255: the configuration record places chroma at byte 16 and the two
  // bit-depth bytes at 17/18 (not 18/19/20), and fills the reserved high bits.
  #[test]
  fn codec_private_uses_correct_hvcc_offsets() {
    let bytes = build_main10_stream();
    let units = nal::split_nal_units(&bytes);
    let headers = extract_headers(&units).unwrap();
    let sps_unit = headers.sps.unwrap();
    let rbsp = nal::strip_emulation_prevention(sps_unit.payload);
    let sps = sps::parse(&rbsp).unwrap();
    let cp = headers.codec_private(&sps);
    assert_eq!(cp[0], 1); // configurationVersion
    assert_eq!(cp[1], 0x02); // profile_space=0, tier=0, profile_idc=2
    assert_eq!(cp[12], 120); // level_idc
    assert_eq!(cp[13], 0xf0); // reserved nibble + min_spatial high bits
    assert_eq!(cp[15], 0xfc); // reserved 6 bits + parallelism_type=0
    assert_eq!(cp[16], 0xfd); // reserved 6 bits + chroma_format_idc=1 (4:2:0)
    assert_eq!(cp[17], 0xfa); // reserved 5 bits + bit_depth_luma_minus8=2
    assert_eq!(cp[18], 0xfa); // reserved 5 bits + bit_depth_chroma_minus8=2
    // Bytes 19-20 are avgFrameRate/reserved — must NOT carry the bit depths.
    assert_eq!(cp[19], 0x00);
    assert_eq!(cp[20], 0x00);
    // numTemporalLayers=1 << 3 | temporalIdNested=1 << 2 | lengthSizeMinusOne=3.
    assert_eq!(cp[21], 0x0f);
    assert_eq!(cp[22], 3); // num arrays (VPS+SPS+PPS)
  }

  #[test]
  fn codec_private_carries_bitstream_restriction_fields() {
    let bytes = build_main10_stream_with_sps(build_main10_1080p_sps_rbsp_with_bitstream_restriction());
    let units = nal::split_nal_units(&bytes);
    let headers = extract_headers(&units).unwrap();
    let sps_unit = headers.sps.unwrap();
    let rbsp = nal::strip_emulation_prevention(sps_unit.payload);
    let sps = sps::parse(&rbsp).unwrap();
    let cp = headers.codec_private(&sps);
    assert_eq!(cp[13], 0xf1);
    assert_eq!(cp[14], 0x23);
    assert_eq!(cp[15], 0xfe);
  }

  #[test]
  fn probe_rejects_incomplete_header_sets() {
    let mut bytes = build_main10_stream();
    let pps_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x44]).unwrap();
    bytes.truncate(pps_pos);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!HevcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_unflushed_first_access_unit() {
    let mut bytes = build_main10_stream();
    let aud_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x46]).unwrap();
    bytes.truncate(aud_pos);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!HevcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_aud_only_after_parameter_sets() {
    let bytes = build_main10_stream_with_aud_only_after_parameter_sets();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!HevcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_mpeg_ts_sync_prefixed_stream() {
    let mut bytes = vec![0x47];
    bytes.extend(build_main10_stream());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(!HevcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("sync-prefixed.hevc", 0);
    let err = HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn probe_rejects_sps_with_zero_cropped_height() {
    let bytes = build_main10_stream_with_sps(build_main10_1080p_sps_rbsp_with_crop(Some(540)));

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(!HevcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("overcrop.hevc", 0);
    let err = HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn read_headers_returns_unrecognised_without_sps() {
    let bytes = vec![0x00, 0x00, 0x00, 0x01, 0x4E, 0x01]; // SEI NAL only
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.hevc", 0);
    let err = HevcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn map_chroma_table() {
    assert_eq!(map_chroma(0), ChromaFormat::Monochrome);
    assert_eq!(map_chroma(1), ChromaFormat::Yuv420);
    assert_eq!(map_chroma(2), ChromaFormat::Yuv422);
    assert_eq!(map_chroma(3), ChromaFormat::Yuv444);
    assert_eq!(map_chroma(7), ChromaFormat::Other);
  }
}
