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

//! IVF container reader — port of `mkvtoolnix/src/input/r_ivf.cpp` and
//! `common/ivf.{h,cpp}`.
//!
//! Layout of the 32-byte fixed header (all multi-byte fields little-endian):
//!
//! ```text
//! offset  size   field
//! 0       4      "DKIF" magic
//! 4       2      version (usually 0)
//! 6       2      header_size (usually 32)
//! 8       4      fourcc (e.g. "AV01", "VP80", "VP90")
//! 12      2      width
//! 14      2      height
//! 16      4      frame_rate_num
//! 20      4      frame_rate_den
//! 24      4      frame_count
//! 28      4      unused
//! ```
//!
//! mkvtoolnix accepts only V_AV1 / V_VP8 / V_VP9 at probe time.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::obu;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{BlockAdditionMapping, Dimensions2D, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

pub const MAGIC: [u8; 4] = *b"DKIF";
const HEADER_LEN: usize = 32;
const FRAME_HEADER_LEN: usize = 12;
const METADATA_TYPE_ITUT_T35: usize = 4;
const DOVI_T35_HEADER: [u8; 9] = [0x00, 0x3b, 0x00, 0x00, 0x08, 0x00, 0x37, 0xcd, 0x08];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
  pub version: u16,
  pub header_size: u16,
  pub fourcc: [u8; 4],
  pub width: u16,
  pub height: u16,
  pub frame_rate_num: u32,
  pub frame_rate_den: u32,
  pub frame_count: u32,
}

impl FileHeader {
  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < HEADER_LEN || bytes[..4] != MAGIC {
      return None;
    }
    Some(Self {
      version: u16::from_le_bytes([bytes[4], bytes[5]]),
      header_size: u16::from_le_bytes([bytes[6], bytes[7]]),
      fourcc: [bytes[8], bytes[9], bytes[10], bytes[11]],
      width: u16::from_le_bytes([bytes[12], bytes[13]]),
      height: u16::from_le_bytes([bytes[14], bytes[15]]),
      frame_rate_num: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
      frame_rate_den: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
      frame_count: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
    })
  }

  /// mkvtoolnix's `probe_file` claims the container on the `DKIF` magic plus a
  /// supported codec FourCC alone (`r_ivf.cpp:30-40`) — it does not inspect the
  /// dimensions or frame rate.
  pub fn has_supported_codec(&self) -> bool {
    IvfCodec::from_fourcc(&self.fourcc).is_some()
  }

  /// `read_headers` sets `m_ok = width && height && frame_rate_num &&
  /// frame_rate_den` (`r_ivf.cpp:50`); only an `m_ok` file contributes a track.
  pub fn dimensions_ok(&self) -> bool {
    self.width != 0 && self.height != 0 && self.frame_rate_num != 0 && self.frame_rate_den != 0
  }

  pub fn is_valid(&self) -> bool {
    self.dimensions_ok() && self.has_supported_codec()
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvfCodec {
  Vp8,
  Vp9,
  Av1,
}

impl IvfCodec {
  pub fn from_fourcc(f: &[u8; 4]) -> Option<Self> {
    match f {
      b"VP80" => Some(Self::Vp8),
      b"VP90" => Some(Self::Vp9),
      b"AV01" => Some(Self::Av1),
      _ => None,
    }
  }

  pub fn codec_id(self) -> &'static str {
    match self {
      Self::Vp8 => "V_VP8",
      Self::Vp9 => "V_VP9",
      Self::Av1 => "V_AV1",
    }
  }

  pub fn codec_name(self) -> &'static str {
    match self {
      Self::Vp8 => "VP8",
      Self::Vp9 => "VP9",
      Self::Av1 => "AV1",
    }
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IvfReader;

impl Reader for IvfReader {
  fn name(&self) -> &'static str {
    "ivf"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = [0u8; HEADER_LEN];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    if read < HEADER_LEN {
      return Ok(false);
    }
    Ok(match FileHeader::parse(&buf) {
      Some(h) => h.has_supported_codec(),
      None => false,
    })
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = [0u8; HEADER_LEN];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    if read < HEADER_LEN {
      return Err(ParseError::Unrecognised);
    }
    let header = FileHeader::parse(&buf).ok_or(ParseError::Unrecognised)?;
    let codec = IvfCodec::from_fourcc(&header.fourcc).ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Ivf;
    out.container.recognized = true;
    out.container.supported = true;

    // mkvtoolnix identifies the container even when `m_ok` is false, but only
    // an `m_ok` file (nonzero dimensions and frame rate) yields a track
    // (`add_available_track_ids` / `create_packetizer` gate on `m_ok`,
    // `r_ivf.cpp:61-86`).
    if !header.dimensions_ok() {
      return Ok(());
    }

    let default_duration_ns = if header.frame_rate_num > 0 && header.frame_rate_den > 0 {
      Some((1_000_000_000u64 * header.frame_rate_den as u64) / header.frame_rate_num as u64)
    } else {
      None
    };
    let dovi_config = if codec == IvfCodec::Av1 {
      read_first_frame(src, deadline).ok().flatten().and_then(|frame| {
        av1_dovi_config_from_frame(&frame, header.width as u32, header.height as u32, default_duration_ns)
      })
    } else {
      None
    };

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    if dovi_config.is_some() {
      common.max_block_addition_id = Some(4);
    }

    let mut video = VideoTrackProperties {
      pixel_dimensions: Some(Dimensions2D {
        width: header.width as u32,
        height: header.height as u32,
      }),
      display_dimensions: Some(Dimensions2D {
        width: header.width as u32,
        height: header.height as u32,
      }),
      default_duration_ns,
      ..VideoTrackProperties::default()
    };
    if let Some(dovi) = dovi_config {
      // PARSER-187: mirror mkvtoolnix's `create_dovi_block_addition_mapping`
      // (`common/dovi_meta.cpp:324-353`) — the AV1 Dolby Vision configuration
      // record is carried as a block-addition mapping, not as the decoder
      // configuration record.  Profile 10 (> 7) keys the mapping by the `dvvC`
      // FOURCC, matching the MP4 path (`codec_specific/dvcc.rs`).
      video.block_addition_mappings.push(BlockAdditionMapping {
        id_type: "dvvC".to_owned(),
        data_hex: hex_encode(&dovi.raw_config),
        ..Default::default()
      });
    }

    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: codec.codec_id().to_string(),
        name: Some(codec.codec_name().to_string()),
        codec_private: None,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoviConfig {
  raw_config: Vec<u8>,
}

fn read_first_frame(src: &mut FileSource, deadline: &Deadline) -> Result<Option<Vec<u8>>, ParseError> {
  src.seek_to(HEADER_LEN as u64)?;
  let mut frame_header = [0u8; FRAME_HEADER_LEN];
  if src.read_at_most(&mut frame_header)? < FRAME_HEADER_LEN {
    return Ok(None);
  }
  let frame_size = u32::from_le_bytes([frame_header[0], frame_header[1], frame_header[2], frame_header[3]]) as usize;
  if frame_size == 0 || frame_size as u64 > deadline.max_element_size() {
    return Ok(None);
  }
  deadline.check("ivf-first-frame")?;
  let mut frame = vec![0u8; frame_size];
  if src.read_at_most(&mut frame)? < frame_size {
    return Ok(None);
  }
  Ok(Some(frame))
}

fn av1_dovi_config_from_frame(
  frame: &[u8],
  width: u32,
  height: u32,
  default_duration_ns: Option<u64>,
) -> Option<DoviConfig> {
  let seq = obu::find_sequence_header(frame).and_then(|body| obu::decode_sequence_header(body).ok())?;
  let duration = default_duration_ns.filter(|d| *d >= 1_000_000).unwrap_or(40_000_000);
  let (color_primaries, transfer_characteristics, matrix_coefficients) = av1_color_triplet(&seq);
  let raw_config = obu::filtered_metadata_bodies(frame).into_iter().find_map(|payload| {
    av1_dovi_config_record_from_metadata_body(
      payload,
      width,
      height,
      duration,
      color_primaries,
      transfer_characteristics,
      matrix_coefficients,
    )
  })?;
  Some(DoviConfig { raw_config })
}

fn read_leb128(bytes: &[u8]) -> Option<(usize, usize)> {
  let mut value = 0u64;
  for i in 0..8 {
    let b = *bytes.get(i)?;
    value |= ((b & 0x7f) as u64) << (i * 7);
    if b & 0x80 == 0 {
      return Some((value as usize, i + 1));
    }
  }
  None
}

pub(crate) fn av1_color_triplet(seq: &obu::SequenceHeader) -> (u8, u8, u8) {
  let desc = seq.color_description;
  (
    desc.map(|c| c.color_primaries).unwrap_or(2),
    desc.map(|c| c.transfer_characteristics).unwrap_or(2),
    desc.map(|c| c.matrix_coefficients).unwrap_or(2),
  )
}

pub(crate) fn av1_dovi_config_record_from_metadata_body(
  body: &[u8],
  width: u32,
  height: u32,
  duration_ns: u64,
  color_primaries: u8,
  transfer_characteristics: u8,
  matrix_coefficients: u8,
) -> Option<Vec<u8>> {
  let rpu_payload = av1_dovi_rpu_payload(body)?;
  let header = parse_av1_t35_dovi_rpu_header(rpu_payload)?;
  let dv_profile = guess_dovi_rpu_profile(&header);
  let compatibility_id = dovi_bl_signal_compatibility_id(
    dv_profile,
    color_primaries,
    matrix_coefficients,
    transfer_characteristics,
  );
  let level = calculate_dovi_level(width, height, duration_ns);
  Some(build_av1_dovi_config_record(level, compatibility_id))
}

fn av1_dovi_rpu_payload(body: &[u8]) -> Option<&[u8]> {
  let (metadata_type, consumed) = read_leb128(body)?;
  if metadata_type != METADATA_TYPE_ITUT_T35 {
    return None;
  }
  let mut pos = consumed;
  let country_code = *body.get(pos)?;
  pos += 1;
  if country_code == 0xff {
    pos += 1;
  }
  let rest = body.get(pos..)?;
  if rest.len() <= DOVI_T35_HEADER.len() || !rest.starts_with(&DOVI_T35_HEADER) {
    return None;
  }
  rest.get(DOVI_T35_HEADER.len()..)
}

#[derive(Debug, Default, Clone, Copy)]
struct DoviRpuHeader {
  rpu_nal_prefix: u32,
  rpu_type: u32,
  rpu_format: u32,
  vdr_rpu_profile: u32,
  bl_video_full_range_flag: bool,
  vdr_bit_depth_minus_8: u64,
  el_spatial_resampling_filter_flag: bool,
  disable_residual_flag: bool,
}

fn parse_av1_t35_dovi_rpu_header(payload: &[u8]) -> Option<DoviRpuHeader> {
  if payload.len() < 3 {
    return None;
  }
  let mut buffer = payload.to_vec();
  let rpu_size = if buffer[1] & 0x10 != 0 {
    if buffer.get(2)? & 0x08 != 0 {
      return None;
    }
    let size = 0x100usize | (((buffer[1] & 0x0f) as usize) << 4) | (((buffer[2] >> 4) & 0x0f) as usize);
    if size + 2 >= buffer.len() {
      return None;
    }
    for i in 0..size {
      buffer[i + 1] = ((buffer[i + 2] & 0x07) << 5) | ((buffer[i + 3] >> 3) & 0x1f);
    }
    size
  } else {
    let size = (((buffer[0] & 0x1f) as usize) << 3) | (((buffer[1] >> 5) & 0x07) as usize);
    if size + 1 >= buffer.len() {
      return None;
    }
    for i in 0..size {
      buffer[i + 1] = ((buffer[i + 1] & 0x0f) << 4) | ((buffer[i + 2] >> 4) & 0x0f);
    }
    size
  };
  buffer[0] = 0x19;
  parse_dovi_rpu_header(&buffer[..rpu_size + 1])
}

fn parse_dovi_rpu_header(bytes: &[u8]) -> Option<DoviRpuHeader> {
  let mut reader = BitReader::new(bytes);
  let mut header = DoviRpuHeader {
    rpu_nal_prefix: reader.read_bits(8).ok()? as u32,
    ..Default::default()
  };
  if header.rpu_nal_prefix != 25 {
    return None;
  }
  header.rpu_type = reader.read_bits(6).ok()? as u32;
  header.rpu_format = reader.read_bits(11).ok()? as u32;

  if header.rpu_type == 2 {
    header.vdr_rpu_profile = reader.read_bits(4).ok()? as u32;
    let _vdr_rpu_level = reader.read_bits(4).ok()?;
    let vdr_seq_info_present = reader.read_bit().ok()?;
    if vdr_seq_info_present {
      let _chroma_resampling_explicit_filter_flag = reader.read_bit().ok()?;
      let coefficient_data_type = reader.read_bits(2).ok()?;
      if coefficient_data_type == 0 {
        let _coefficient_log2_denom = reader.read_ue().ok()?;
      }
      let _vdr_rpu_normalized_idc = reader.read_bits(2).ok()?;
      header.bl_video_full_range_flag = reader.read_bit().ok()?;
      if (header.rpu_format & 0x700) == 0 {
        let _bl_bit_depth_minus8 = reader.read_ue().ok()?;
        let _el_bit_depth_minus8 = reader.read_ue().ok()?;
        header.vdr_bit_depth_minus_8 = reader.read_ue().ok()? as u64;
        let _spatial_resampling_filter_flag = reader.read_bit().ok()?;
        let _reserved_zero_3bits = reader.read_bits(3).ok()?;
        header.el_spatial_resampling_filter_flag = reader.read_bit().ok()?;
        header.disable_residual_flag = reader.read_bit().ok()?;
      }
    }

    let _vdr_dm_metadata_present_flag = reader.read_bit().ok()?;
    let use_prev_vdr_rpu_flag = reader.read_bit().ok()?;
    if use_prev_vdr_rpu_flag {
      let _prev_vdr_rpu_id = reader.read_ue().ok()?;
    } else {
      let _vdr_rpu_id = reader.read_ue().ok()?;
      let _mapping_color_space = reader.read_ue().ok()?;
      let _mapping_chroma_format_idc = reader.read_ue().ok()?;
      for _ in 0..3 {
        let num_pivots_minus2 = reader.read_ue().ok()? as u64;
        for _ in 0..(num_pivots_minus2 + 2) {
          reader.skip_bits(header.vdr_bit_depth_minus_8 + 8).ok()?;
        }
      }
      if (header.rpu_format & 0x700) != 0 && !header.disable_residual_flag {
        reader.skip_bits(3).ok()?;
      }
      let _num_x_partitions_minus1 = reader.read_ue().ok()?;
      let _num_y_partitions_minus1 = reader.read_ue().ok()?;
    }
  }

  Some(header)
}

fn guess_dovi_rpu_profile(header: &DoviRpuHeader) -> u8 {
  let has_el = header.el_spatial_resampling_filter_flag && !header.disable_residual_flag;
  if header.rpu_nal_prefix != 25 {
    return 0;
  }
  if header.vdr_rpu_profile == 0 && header.bl_video_full_range_flag {
    return 5;
  }
  if has_el {
    if header.vdr_bit_depth_minus_8 == 4 { 7 } else { 4 }
  } else {
    8
  }
}

fn dovi_bl_signal_compatibility_id(
  dv_profile: u8,
  color_primaries: u8,
  matrix_coefficients: u8,
  transfer_characteristics: u8,
) -> u8 {
  match dv_profile {
    4 => 2,
    5 => 0,
    7 => 6,
    8 => {
      if color_primaries == 9 && matrix_coefficients == 9 {
        match transfer_characteristics {
          16 => 1,
          14 | 18 => 4,
          _ => 0,
        }
      } else {
        2
      }
    }
    9 => 2,
    _ => 0,
  }
}

/// AV1 Dolby Vision level from the picture rate (`common/dovi_meta.cpp`).
/// Shared with the raw AV1 OBU reader (PARSER-246).
pub(crate) fn calculate_dovi_level(width: u32, height: u32, duration_ns: u64) -> u8 {
  let frame_rate = 1_000_000_000u64 / duration_ns.max(1);
  let pps = frame_rate.saturating_mul(width as u64 * height as u64);
  match pps {
    0..=22_118_400 => 1,
    22_118_401..=27_648_000 => 2,
    27_648_001..=49_766_400 => 3,
    49_766_401..=62_208_000 => 4,
    62_208_001..=124_416_000 => 5,
    124_416_001..=199_065_600 => 6,
    199_065_601..=248_832_000 => 7,
    248_832_001..=398_131_200 => 8,
    398_131_201..=497_664_000 => 9,
    497_664_001..=995_328_000 if width <= 3840 => 10,
    497_664_001..=995_328_000 => 11,
    995_328_001..=1_990_656_000 => 12,
    1_990_656_001..=3_981_312_000 => 13,
    _ => 0,
  }
}

/// 24-byte AV1 Dolby Vision configuration record (DV profile 10) keyed by the
/// `dvvC` block-addition FOURCC.  Shared with the raw AV1 OBU reader
/// (PARSER-246).
pub(crate) fn build_av1_dovi_config_record(level: u8, compatibility_id: u8) -> Vec<u8> {
  let mut p = vec![0u8; 24];
  p[0] = 1;
  p[1] = 0;
  p[2] = (10 << 1) | ((level >> 5) & 0x01);
  p[3] = ((level & 0x1f) << 3) | 0b101;
  p[4] = (compatibility_id & 0x0f) << 4;
  p
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
  bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
pub(crate) fn build_header(fourcc: &[u8; 4], width: u16, height: u16) -> Vec<u8> {
  let mut buf = Vec::with_capacity(HEADER_LEN);
  buf.extend_from_slice(&MAGIC);
  buf.extend_from_slice(&0u16.to_le_bytes()); // version
  buf.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes()); // header_size
  buf.extend_from_slice(fourcc);
  buf.extend_from_slice(&width.to_le_bytes());
  buf.extend_from_slice(&height.to_le_bytes());
  buf.extend_from_slice(&30_000u32.to_le_bytes()); // frame_rate_num
  buf.extend_from_slice(&1000u32.to_le_bytes()); // frame_rate_den
  buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
  buf.extend_from_slice(&0u32.to_le_bytes()); // unused
  buf
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn parses_av1_header() {
    let h = FileHeader::parse(&build_header(b"AV01", 1920, 1080)).unwrap();
    assert_eq!(h.fourcc, *b"AV01");
    assert_eq!(h.width, 1920);
    assert_eq!(h.height, 1080);
    assert_eq!(h.frame_rate_num, 30_000);
    assert_eq!(h.frame_rate_den, 1000);
  }

  #[test]
  fn parse_rejects_wrong_magic() {
    let mut bytes = build_header(b"AV01", 1920, 1080);
    bytes[0] = b'X';
    assert!(FileHeader::parse(&bytes).is_none());
  }

  #[test]
  fn parse_rejects_short_input() {
    assert!(FileHeader::parse(&[0u8; 10]).is_none());
  }

  #[test]
  fn codec_from_fourcc_recognises_supported_codecs() {
    assert_eq!(IvfCodec::from_fourcc(b"VP80"), Some(IvfCodec::Vp8));
    assert_eq!(IvfCodec::from_fourcc(b"VP90"), Some(IvfCodec::Vp9));
    assert_eq!(IvfCodec::from_fourcc(b"AV01"), Some(IvfCodec::Av1));
    assert_eq!(IvfCodec::from_fourcc(b"XYZW"), None);
  }

  #[test]
  fn codec_ids_match_matroska_convention() {
    assert_eq!(IvfCodec::Vp8.codec_id(), "V_VP8");
    assert_eq!(IvfCodec::Vp9.codec_id(), "V_VP9");
    assert_eq!(IvfCodec::Av1.codec_id(), "V_AV1");
  }

  #[test]
  fn probe_accepts_av1_blob() {
    let blob = build_header(b"AV01", 1280, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(IvfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_unsupported_fourcc() {
    let blob = build_header(b"ZZZZ", 1280, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!IvfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_claims_zero_dimensions_or_rate_on_magic_and_fourcc() {
    // PARSER-247: mkvtoolnix's probe_file only checks DKIF magic + supported
    // codec FourCC, so malformed-but-claimed IVF headers (zero dimensions or
    // frame rate) are still claimed by the probe.
    let blob = build_header(b"AV01", 0, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(IvfReader.probe(&mut s).unwrap());

    let mut blob = build_header(b"AV01", 1280, 720);
    blob[20..24].copy_from_slice(&0u32.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(IvfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"DKIF".to_vec()));
    assert!(!IvfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_vp9_track() {
    let blob = build_header(b"VP90", 1920, 1080);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Ivf);
    assert_eq!(out.tracks[0].codec.id, "V_VP9");
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    // 30000/1000 fps → 1/30 second = 33_333_333 ns
    assert_eq!(v.default_duration_ns, Some(33_333_333));
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

    fn write_bit(&mut self, bit: bool) {
      if self.bit_index == 0 {
        self.buf.push(0);
      }
      if bit {
        let last = self.buf.len() - 1;
        self.buf[last] |= 1 << (7 - self.bit_index);
      }
      self.bit_index = (self.bit_index + 1) % 8;
    }

    fn write_bits(&mut self, value: u64, bits: u32) {
      for i in 0..bits {
        self.write_bit((value >> (bits - 1 - i)) & 1 != 0);
      }
    }

    fn into_bytes(mut self) -> Vec<u8> {
      while self.bit_index != 0 {
        self.write_bit(false);
      }
      self.buf
    }
  }

  fn build_reduced_sequence_header(max_w: u32, max_h: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 3); // seq_profile
    w.write_bit(false); // still_picture
    w.write_bit(true); // reduced_still_picture_header
    w.write_bits(0, 5); // seq_level_idx
    w.write_bits(11, 4); // frame_width_bits_minus_1
    w.write_bits(11, 4); // frame_height_bits_minus_1
    w.write_bits((max_w - 1) as u64, 12);
    w.write_bits((max_h - 1) as u64, 12);
    w.write_bit(false); // use_128x128_superblock
    w.write_bit(false); // enable_filter_intra
    w.write_bit(false); // enable_intra_edge_filter
    w.write_bit(false); // enable_superres
    w.write_bit(false); // enable_cdef
    w.write_bit(false); // enable_restoration
    w.write_bit(false); // high_bitdepth
    w.write_bit(false); // monochrome
    w.write_bit(false); // color_description_present
    w.into_bytes()
  }

  fn build_sequence_header_with_operating_point(max_w: u32, max_h: u32, operating_point_idc: u16) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(0, 3); // seq_profile
    w.write_bit(false); // still_picture
    w.write_bit(false); // reduced_still_picture_header
    w.write_bit(false); // timing_info_present
    w.write_bit(false); // initial_display_delay_present_flag
    w.write_bits(0, 5); // operating_points_cnt_minus_1
    w.write_bits(operating_point_idc as u64, 12);
    w.write_bits(8, 5); // seq_level_idx[0]
    w.write_bit(false); // seq_tier[0]
    w.write_bits(11, 4); // frame_width_bits_minus_1
    w.write_bits(11, 4); // frame_height_bits_minus_1
    w.write_bits((max_w - 1) as u64, 12);
    w.write_bits((max_h - 1) as u64, 12);
    w.write_bit(false); // frame_id_numbers_present
    w.write_bit(false); // use_128x128_superblock
    w.write_bit(false); // enable_filter_intra
    w.write_bit(false); // enable_intra_edge_filter
    w.write_bit(false); // enable_interintra_compound
    w.write_bit(false); // enable_masked_compound
    w.write_bit(false); // enable_warped_motion
    w.write_bit(false); // enable_dual_filter
    w.write_bit(false); // enable_order_hint
    w.write_bit(true); // seq_choose_screen_content_tools
    w.write_bit(true); // seq_choose_integer_mv
    w.write_bit(false); // enable_superres
    w.write_bit(false); // enable_cdef
    w.write_bit(false); // enable_restoration
    w.write_bit(false); // high_bitdepth
    w.write_bit(false); // monochrome
    w.write_bit(false); // color_description_present
    w.write_bit(false); // video_full_range_flag
    w.write_bits(0, 2); // chroma_sample_position
    w.write_bit(false); // separate_uv_delta_q
    w.into_bytes()
  }

  fn obu_packet(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 128);
    let mut out = vec![(obu_type << 3) | 0x02, payload.len() as u8];
    out.extend_from_slice(payload);
    out
  }

  fn obu_packet_with_extension(obu_type: u8, temporal_id: u8, spatial_id: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 128);
    let mut out = vec![
      (obu_type << 3) | 0x04 | 0x02,
      ((temporal_id & 0x07) << 5) | ((spatial_id & 0x03) << 3),
      payload.len() as u8,
    ];
    out.extend_from_slice(payload);
    out
  }

  fn build_av1_frame_with_dovi() -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend(obu_packet(1, &build_reduced_sequence_header(1920, 1080)));
    let mut metadata = vec![METADATA_TYPE_ITUT_T35 as u8, 0xb5];
    metadata.extend_from_slice(&DOVI_T35_HEADER);
    metadata.extend_from_slice(&valid_av1_dovi_rpu_payload());
    frame.extend(obu_packet(5, &metadata));
    frame.extend(obu_packet(6, &[0]));
    frame
  }

  fn valid_av1_dovi_rpu_payload() -> Vec<u8> {
    // Short AV1 T.35 RPU coding. After conversion this yields a regular RPU
    // header starting with 0x19 and rpu_type 0, enough for mkvtoolnix to infer
    // DV profile 8 and derive the base-layer compatibility id from AV1 color.
    vec![0x00, 0x60, 0x00, 0x00, 0x00]
  }

  fn build_ivf_with_first_frame(frame: &[u8]) -> Vec<u8> {
    let mut bytes = build_header(b"AV01", 1920, 1080);
    bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(frame);
    bytes
  }

  #[test]
  fn av1_dovi_first_frame_sets_block_addition_surface() {
    let frame = build_av1_frame_with_dovi();
    let mut s = FileSource::from_reader_for_test(Cursor::new(build_ivf_with_first_frame(&frame)));
    let mut out = MediaMetadata::new("dv.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let track = &out.tracks[0];
    assert_eq!(track.properties.common.max_block_addition_id, Some(4));
    // PARSER-187: DV record is exposed as a block-addition mapping, not as the
    // primary codec configuration record.
    let video = track.properties.video.as_ref().unwrap();
    assert!(video.codec_config.is_none());
    assert_eq!(video.block_addition_mappings.len(), 1);
    let mapping = &video.block_addition_mappings[0];
    assert_eq!(mapping.id_type, "dvvC");
    // 24-byte AV1 DV config record → 48 hex chars.
    assert_eq!(mapping.data_hex.len(), 48);
    // The encoded config record carries DV profile 10 in its third byte.
    assert!(mapping.data_hex.starts_with("0100"));
    // Unspecified AV1 color with an RPU-inferred profile 8 maps to BL
    // compatibility id 2, encoded in the high nibble of byte 4.
    assert_eq!(&mapping.data_hex[8..10], "20");
  }

  #[test]
  fn av1_dovi_first_frame_ignores_out_of_operating_point_metadata() {
    let mut frame = Vec::new();
    frame.extend(obu_packet(
      1,
      &build_sequence_header_with_operating_point(1920, 1080, 0x101),
    ));
    let mut metadata = vec![METADATA_TYPE_ITUT_T35 as u8, 0xb5];
    metadata.extend_from_slice(&DOVI_T35_HEADER);
    metadata.extend_from_slice(&valid_av1_dovi_rpu_payload());
    frame.extend(obu_packet_with_extension(5, 1, 0, &metadata));
    frame.extend(obu_packet(6, &[0]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(build_ivf_with_first_frame(&frame)));
    let mut out = MediaMetadata::new("dv-out-of-op.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let track = &out.tracks[0];
    assert_eq!(track.properties.common.max_block_addition_id, None);
    let video = track.properties.video.as_ref().unwrap();
    assert!(video.block_addition_mappings.is_empty());
  }

  #[test]
  fn av1_dovi_first_frame_over_one_mib_is_still_read() {
    let mut frame = build_av1_frame_with_dovi();
    frame.resize(1024 * 1024 + 1, 0);
    let mut s = FileSource::from_reader_for_test(Cursor::new(build_ivf_with_first_frame(&frame)));
    let mut out = MediaMetadata::new("dv-large.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(video.block_addition_mappings.len(), 1);
  }

  #[test]
  fn read_headers_rejects_unsupported_fourcc() {
    let blob = build_header(b"ZZZZ", 1280, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ivf", 0);
    let err = IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn read_headers_recognizes_container_without_track_on_invalid_rate() {
    // PARSER-247: a claimed-but-not-`m_ok` IVF still identifies the container;
    // mkvtoolnix adds no track because `add_available_track_ids` gates on
    // `m_ok`.
    let mut blob = build_header(b"AV01", 1280, 720);
    blob[16..20].copy_from_slice(&0u32.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Ivf);
    assert!(out.container.recognized);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_recognizes_container_without_track_on_zero_dimensions() {
    let blob = build_header(b"AV01", 0, 0);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ivf", 0);
    IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Ivf);
    assert!(out.container.recognized);
    assert!(out.tracks.is_empty());
  }
}
