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

//! Top-level `AvcReader` — recognises raw AVC/H.264 elementary streams.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
  ChromaFormat, Dimensions2D, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::mp4::codec_specific::hex_encode;
use crate::media_metadata::reader::Reader;

use super::nal::{self, NAL_UNIT_TYPE_PPS, NAL_UNIT_TYPE_SPS};
use super::sps;

const PROBE_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_PROBE_CHUNKS: usize = 50;

#[derive(Debug, Default, Clone, Copy)]
pub struct AvcReader;

impl Reader for AvcReader {
  fn name(&self) -> &'static str {
    "avc"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let buf = read_probe_prefix(src, None)?;
    if buf.len() < 5 {
      return Ok(false);
    }
    if starts_with_mpeg_ts_sync(&buf) {
      return Ok(false);
    }
    let units = nal::split_nal_units(&buf);
    Ok(extract_headers(&units).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
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

    out.container.format = ContainerFormat::Avc;
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
        id: "V_MPEG4/ISO/AVC".to_string(),
        name: Some("AVC/H.264".to_string()),
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
      d.check("avc-probe")?;
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

fn starts_with_mpeg_ts_sync(buf: &[u8]) -> bool {
  buf.first() == Some(&0x47)
}

#[derive(Debug, Clone, Copy)]
struct AvcHeaders<'a> {
  sps: Option<nal::NalUnit<'a>>,
  pps: Option<nal::NalUnit<'a>>,
}

impl<'a> AvcHeaders<'a> {
  fn codec_private(&self, sps: &sps::AvcSps) -> Vec<u8> {
    let sps_unit = self.sps.expect("validated SPS");
    let pps_unit = self.pps.expect("validated PPS");
    let sps_bytes = nal_bytes(sps_unit);
    let pps_bytes = nal_bytes(pps_unit);
    let mut out = Vec::new();
    out.push(1);
    out.push(sps.profile_idc);
    // PARSER-257: byte 2 carries the SPS constraint-set / profile-compatibility
    // flags (`buffer[2] = sps.profile_compat`,
    // `../mkvtoolnix/src/common/avc/avcc.cpp:134`), not a hard-coded zero.
    out.push(sps.profile_compat);
    out.push(sps.level_idc);
    out.push(0xff);
    out.push(0xe1);
    out.extend_from_slice(&(sps_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(&sps_bytes);
    out.push(1);
    out.extend_from_slice(&(pps_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(&pps_bytes);
    out
  }
}

fn extract_headers<'a>(units: &'a [nal::NalUnit<'a>]) -> Option<AvcHeaders<'a>> {
  let mut headers = AvcHeaders { sps: None, pps: None };
  for unit in units {
    if unit.nal_unit_type == NAL_UNIT_TYPE_SPS && headers.sps.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if sps::parse(&rbsp).is_ok_and(|sps| sps.display_width > 0 && sps.display_height > 0) {
        headers.sps = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_PPS && headers.pps.is_none() {
      headers.pps = Some(*unit);
    }
  }
  if headers.sps.is_some() && headers.pps.is_some() {
    Some(headers)
  } else {
    None
  }
}

fn nal_bytes(unit: nal::NalUnit<'_>) -> Vec<u8> {
  let mut bytes = Vec::with_capacity(unit.payload.len() + 1);
  bytes.push((unit.nal_ref_idc << 5) | unit.nal_unit_type);
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

  /// Build a tiny Annex B byte stream with a baseline 1920x1080 SPS so the
  /// reader produces a complete track.
  fn build_avc_with_baseline_1080p_sps() -> Vec<u8> {
    build_avc_with_sps_tail(build_baseline_1080p_tail())
  }

  fn build_avc_with_sps_tail(tail: Vec<u8>) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67]; // SPS NAL
    bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
    bytes.extend(tail);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE]); // PPS NAL
    // Append an AUD NAL so the SPS NAL has a definite end.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]);
    bytes
  }

  fn build_baseline_1080p_tail() -> Vec<u8> {
    build_baseline_1080p_tail_with_vui(false)
  }

  fn build_baseline_1080p_tail_with_vui(include_vui: bool) -> Vec<u8> {
    build_baseline_1080p_tail_with_crop_and_vui(4, include_vui)
  }

  fn build_baseline_1080p_tail_with_crop_and_vui(crop_bottom: u32, include_vui: bool) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(119); // pic_width_in_mbs_minus1 = 1920/16-1
    w.write_ue(67); // pic_height_in_map_units_minus1 = 1080/16-1 → coded 1088
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(crop_bottom); // crop_bottom → 1088 - 4*2 = 1080
    if include_vui {
      w.write_bit(true); // vui_parameters_present_flag
      w.write_bit(false); // aspect_ratio_info_present_flag
      w.write_bit(false); // overscan_info_present_flag
      w.write_bit(false); // video_signal_type_present_flag
      w.write_bit(false); // chroma_loc_info_present_flag
      w.write_bit(true); // timing_info_present_flag
      w.write_bits(1, 32); // num_units_in_tick
      w.write_bits(60, 32); // time_scale
      w.write_bit(true); // fixed_frame_rate_flag
    } else {
      w.write_bit(false); // vui_parameters_present_flag
    }
    w.into_bytes()
  }

  fn build_avc_with_vui_timing() -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67];
    bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
    bytes.extend(build_baseline_1080p_tail_with_vui(true));
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE]);
    bytes
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
  fn probe_accepts_annex_b_with_sps() {
    let bytes = build_avc_with_baseline_1080p_sps();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(AvcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
    assert!(!AvcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_sps_without_pps() {
    let mut bytes = build_avc_with_baseline_1080p_sps();
    let pps_pos = bytes.windows(5).position(|w| w == [0, 0, 0, 1, 0x68]).unwrap();
    bytes.truncate(pps_pos);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!AvcReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_mpeg_ts_sync_prefixed_stream() {
    let mut bytes = vec![0x47];
    bytes.extend(build_avc_with_baseline_1080p_sps());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(!AvcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("sync-prefixed.h264", 0);
    let err = AvcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn probe_rejects_sps_with_zero_cropped_height() {
    let tail = build_baseline_1080p_tail_with_crop_and_vui(544, false);
    let bytes = build_avc_with_sps_tail(tail);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(!AvcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("overcrop.h264", 0);
    let err = AvcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn read_headers_extracts_dimensions_and_codec_config() {
    let bytes = build_avc_with_baseline_1080p_sps();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.h264", 0);
    AvcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Avc);
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    let cfg = v.codec_config.as_ref().unwrap();
    assert_eq!(cfg.profile_idc, Some(66));
    assert_eq!(cfg.level_name.as_deref(), Some("4.0"));
    assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
    assert_eq!(cfg.bit_depth_luma, Some(8));
    assert_eq!(cfg.is_elementary_stream, Some(true));
    assert!(cfg.raw_hex.as_ref().unwrap().starts_with("01420028"));
    assert!(out.tracks[0].codec.codec_private.is_some());
  }

  #[test]
  fn probe_and_read_headers_find_headers_after_64k_prefix() {
    let mut bytes = vec![0x00u8; 70 * 1024];
    bytes.extend(build_avc_with_baseline_1080p_sps());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(AvcReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("late.h264", 0);
    AvcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Avc);
    assert_eq!(out.tracks.len(), 1);
  }

  #[test]
  fn read_headers_extracts_vui_timing_duration() {
    let bytes = build_avc_with_vui_timing();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("timed.h264", 0);
    AvcReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    // PARSER-238: num_units_in_tick=1, time_scale=60 → 1e9 / 60 (no ×2).
    assert_eq!(v.default_duration_ns, Some(16_666_666));
  }

  #[test]
  fn read_headers_returns_unrecognised_without_sps() {
    let bytes = vec![0x00, 0x00, 0x00, 0x01, 0x06]; // SEI NAL only
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.h264", 0);
    let err = AvcReader
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
