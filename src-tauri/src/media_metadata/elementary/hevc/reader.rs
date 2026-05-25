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

use super::nal::{self, NAL_UNIT_TYPE_PPS, NAL_UNIT_TYPE_SPS, NAL_UNIT_TYPE_VPS};
use super::sps::{self, HevcTier};
use super::vps;

const PROBE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct HevcReader;

impl Reader for HevcReader {
  fn name(&self) -> &'static str {
    "hevc"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    if read < 7 {
      return Ok(false);
    }
    let units = nal::split_nal_units(&buf[..read]);
    Ok(extract_headers(&units).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    let units = nal::split_nal_units(&buf[..read]);
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
    let mut out = vec![0u8; 23];
    out[0] = 1;
    out[1] = if sps.tier == HevcTier::High {
      0x20 | (sps.profile_idc & 0x1f)
    } else {
      sps.profile_idc & 0x1f
    };
    out[12] = sps.level_idc;
    out[18] = 0xfc | (sps.chroma_format_idc & 0x03);
    out[19] = 0xf8 | sps.bit_depth_luma.saturating_sub(8);
    out[20] = 0xf8 | sps.bit_depth_chroma.saturating_sub(8);
    out[21] = 0x03;
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
  for unit in units {
    if unit.nal_unit_type == NAL_UNIT_TYPE_VPS && headers.vps.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if vps::parse(&rbsp).is_ok() {
        headers.vps = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_SPS && headers.sps.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if sps::parse(&rbsp).is_ok() {
        headers.sps = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_PPS && headers.pps.is_none() {
      headers.pps = Some(*unit);
    }
  }
  if headers.vps.is_some() && headers.sps.is_some() && headers.pps.is_some() {
    Some(headers)
  } else {
    None
  }
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
    bytes
  }

  fn build_main10_1080p_sps_rbsp() -> Vec<u8> {
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
    w.write_bit(false); // conformance_window_flag
    w.write_ue(2); // bit_depth_luma_minus8
    w.write_ue(2); // bit_depth_chroma_minus8
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
    write_simple_tail_with_timing(&mut w);
    w.into_bytes()
  }

  fn write_simple_tail_with_timing(w: &mut BitWriter) {
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
    w.write_bit(true); // timing info
    w.write_bits(1, 32);
    w.write_bits(30, 32);
    w.write_bit(false); // poc proportional timing
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

  #[test]
  fn probe_rejects_incomplete_header_sets() {
    let mut bytes = build_main10_stream();
    let pps_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x44]).unwrap();
    bytes.truncate(pps_pos);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!HevcReader.probe(&mut s).unwrap());
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
