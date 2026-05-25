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
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoCodecConfig, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

pub const MAGIC: [u8; 4] = *b"DKIF";
const HEADER_LEN: usize = 32;
const FRAME_HEADER_LEN: usize = 12;
const FIRST_FRAME_CAP: usize = 1024 * 1024;
const OBU_TYPE_METADATA: u8 = 5;
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

  pub fn is_valid(&self) -> bool {
    self.width != 0
      && self.height != 0
      && self.frame_rate_num != 0
      && self.frame_rate_den != 0
      && IvfCodec::from_fourcc(&self.fourcc).is_some()
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
      Some(h) => h.is_valid(),
      None => false,
    })
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
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
    if !header.is_valid() {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::Ivf;
    out.container.recognized = true;
    out.container.supported = true;

    let default_duration_ns = if header.frame_rate_num > 0 && header.frame_rate_den > 0 {
      Some((1_000_000_000u64 * header.frame_rate_den as u64) / header.frame_rate_num as u64)
    } else {
      None
    };
    let dovi_config = if codec == IvfCodec::Av1 {
      read_first_frame(src).ok().flatten().and_then(|frame| {
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
      video.codec_config = Some(VideoCodecConfig {
        profile_name: Some(format!("Dolby Vision v1.0 profile 10 level {} BL+RPU", dovi.level)),
        profile_idc: Some(10),
        level_name: Some(dovi.level.to_string()),
        level_idc: Some(dovi.level as u32),
        raw_hex: Some(hex_encode(&dovi.raw_config)),
        ..VideoCodecConfig::default()
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
  level: u8,
  raw_config: Vec<u8>,
}

fn read_first_frame(src: &mut FileSource) -> Result<Option<Vec<u8>>, ParseError> {
  src.seek_to(HEADER_LEN as u64)?;
  let mut frame_header = [0u8; FRAME_HEADER_LEN];
  if src.read_at_most(&mut frame_header)? < FRAME_HEADER_LEN {
    return Ok(None);
  }
  let frame_size = u32::from_le_bytes([frame_header[0], frame_header[1], frame_header[2], frame_header[3]]) as usize;
  if frame_size == 0 || frame_size > FIRST_FRAME_CAP {
    return Ok(None);
  }
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
  if obu::find_sequence_header(frame).is_none() || !frame_has_dovi_rpu(frame) {
    return None;
  }
  let duration = default_duration_ns.filter(|d| *d >= 1_000_000).unwrap_or(40_000_000);
  let level = calculate_dovi_level(width, height, duration);
  Some(DoviConfig {
    level,
    raw_config: build_av1_dovi_config_record(level, 0),
  })
}

fn frame_has_dovi_rpu(frame: &[u8]) -> bool {
  walk_av1_obus(frame, |obu_type, payload| {
    if obu_type != OBU_TYPE_METADATA {
      return None;
    }
    let (metadata_type, consumed) = read_leb128(payload)?;
    if metadata_type != METADATA_TYPE_ITUT_T35 {
      return None;
    }
    let mut pos = consumed;
    let country_code = *payload.get(pos)?;
    pos += 1;
    if country_code == 0xff {
      pos += 1;
    }
    let rest = payload.get(pos..)?;
    (rest.len() > DOVI_T35_HEADER.len() && rest.starts_with(&DOVI_T35_HEADER)).then_some(())
  })
  .is_some()
}

fn walk_av1_obus<'a, T, F>(bytes: &'a [u8], mut visit: F) -> Option<T>
where
  F: FnMut(u8, &'a [u8]) -> Option<T>,
{
  let mut pos = 0usize;
  while pos < bytes.len() {
    let header = obu::decode_header(bytes[pos]);
    pos += 1;
    if header.has_extension {
      pos += 1;
      if pos > bytes.len() {
        return None;
      }
    }
    let payload_len = if header.has_size_field {
      let (size, consumed) = read_leb128(&bytes[pos..])?;
      pos += consumed;
      size
    } else {
      bytes.len().saturating_sub(pos)
    };
    let payload_end = pos.checked_add(payload_len)?;
    if payload_end > bytes.len() {
      return None;
    }
    if let Some(value) = visit(header.obu_type, &bytes[pos..payload_end]) {
      return Some(value);
    }
    pos = payload_end;
  }
  None
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

fn calculate_dovi_level(width: u32, height: u32, duration_ns: u64) -> u8 {
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

fn build_av1_dovi_config_record(level: u8, compatibility_id: u8) -> Vec<u8> {
  let mut p = vec![0u8; 24];
  p[0] = 1;
  p[1] = 0;
  p[2] = (10 << 1) | ((level >> 5) & 0x01);
  p[3] = ((level & 0x1f) << 3) | 0b101;
  p[4] = (compatibility_id & 0x0f) << 4;
  p
}

fn hex_encode(bytes: &[u8]) -> String {
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
  fn probe_rejects_zero_dimensions_or_rate() {
    let blob = build_header(b"AV01", 0, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!IvfReader.probe(&mut s).unwrap());

    let mut blob = build_header(b"AV01", 1280, 720);
    blob[20..24].copy_from_slice(&0u32.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!IvfReader.probe(&mut s).unwrap());
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

  fn obu_packet(obu_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() < 128);
    let mut out = vec![(obu_type << 3) | 0x02, payload.len() as u8];
    out.extend_from_slice(payload);
    out
  }

  fn build_av1_frame_with_dovi() -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend(obu_packet(1, &build_reduced_sequence_header(1920, 1080)));
    let mut metadata = vec![METADATA_TYPE_ITUT_T35 as u8, 0xb5];
    metadata.extend_from_slice(&DOVI_T35_HEADER);
    metadata.extend_from_slice(&[0x11, 0x22]);
    frame.extend(obu_packet(OBU_TYPE_METADATA, &metadata));
    frame.extend(obu_packet(6, &[0]));
    frame
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
    let cfg = track.properties.video.as_ref().unwrap().codec_config.as_ref().unwrap();
    assert_eq!(cfg.profile_idc, Some(10));
    assert!(cfg.profile_name.as_deref().unwrap().contains("Dolby Vision"));
    assert_eq!(cfg.raw_hex.as_ref().unwrap().len(), 48);
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
  fn read_headers_rejects_invalid_zero_rate() {
    let mut blob = build_header(b"AV01", 1280, 720);
    blob[16..20].copy_from_slice(&0u32.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ivf", 0);
    let err = IvfReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
