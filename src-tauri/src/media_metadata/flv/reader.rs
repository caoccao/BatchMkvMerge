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

//! FlvReader — walks tags until each declared stream has identification
//! state filled in (or until the 1 MiB detection window is exhausted).

use crate::media_metadata::audio::aac;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::avc::{nal as avc_nal, sps as avc_sps};
use crate::media_metadata::elementary::hevc::{nal as hevc_nal, sps as hevc_sps};
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
  ChromaFormat, Dimensions2D, HevcTier as ModelHevcTier, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::mp4::codec_specific::hex_encode;
use crate::media_metadata::reader::Reader;

use super::header::{FlvHeader, HEADER_LEN};
use super::script_data;
use super::tag::{AudioTagFlags, FlvTagHeader, VideoCodecId};

const DETECT_WINDOW: u64 = 1024 * 1024;

#[derive(Debug, Default, Clone)]
struct VideoState {
  codec: Option<VideoCodecId>,
  width: Option<u32>,
  height: Option<u32>,
  frame_rate: Option<f64>,
  headers_read: bool,
  codec_private: Option<Vec<u8>>,
  codec_config: Option<VideoCodecConfig>,
}

#[derive(Debug, Default, Clone)]
struct AudioState {
  codec_id: Option<&'static str>,
  codec_name: Option<&'static str>,
  sample_rate: Option<u32>,
  channels: Option<u32>,
  bit_depth: Option<u32>,
  headers_read: bool,
  codec_private: Option<Vec<u8>>,
  codec_config: Option<AudioCodecConfig>,
}

impl VideoState {
  fn is_valid(&self) -> bool {
    match self.codec {
      Some(VideoCodecId::H264 | VideoCodecId::H265) => self.headers_read,
      Some(VideoCodecId::SorensonH263) => self.width.is_some() && self.height.is_some(),
      Some(_) => true,
      None => false,
    }
  }
}

impl AudioState {
  fn is_valid(&self) -> bool {
    match self.codec_id {
      Some("A_AAC") => self.headers_read,
      Some(_) => true,
      None => false,
    }
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FlvReader;

impl Reader for FlvReader {
  fn name(&self) -> &'static str {
    "flv"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = [0u8; HEADER_LEN];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    if read < HEADER_LEN {
      return Ok(false);
    }
    Ok(FlvHeader::parse(&buf).is_some())
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let mut buf = [0u8; HEADER_LEN];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    if read < HEADER_LEN {
      return Err(ParseError::Unrecognised);
    }
    let header = FlvHeader::parse(&buf).ok_or(ParseError::Unrecognised)?;
    src.seek_to(header.data_offset as u64)?;

    out.container.format = ContainerFormat::Flv;
    out.container.recognized = true;
    out.container.supported = true;

    let mut video = VideoState::default();
    let mut audio = AudioState::default();
    let mut video_seen = false;
    let mut audio_seen = false;

    loop {
      deadline.check("flv-tag")?;
      let pos = src.position();
      if pos >= DETECT_WINDOW {
        break;
      }
      // Try to read the next tag header.  EOF here is a clean stop.
      let mut header_buf = [0u8; FlvTagHeader::TOTAL_LEN];
      match src.read_at_most(&mut header_buf)? {
        n if n < FlvTagHeader::TOTAL_LEN => break,
        _ => {}
      }
      let tag = match FlvTagHeader::parse(&header_buf) {
        Some(t) => t,
        None => break,
      };
      let payload_pos = src.position();
      if tag.is_audio() {
        audio_seen = true;
        read_audio_payload(src, tag.data_size, &mut audio)?;
      } else if tag.is_video() {
        video_seen = true;
        read_video_payload(src, tag.data_size, &mut video)?;
      } else if tag.is_script() {
        read_script_payload(src, tag.data_size, &mut video)?;
      }
      // Always seek to the byte just past this tag's payload.
      src.seek_to(payload_pos + tag.data_size as u64)?;
      // Stop early once declared streams have enough header data to be
      // muxer-valid. AVC/HEVC/AAC need their sequence headers.
      let video_done = !header.has_video() || video.is_valid();
      let audio_done = !header.has_audio() || audio.is_valid();
      if video_done && audio_done && (video_seen || audio_seen) {
        break;
      }
    }

    let mut track_id: i64 = 0;
    if header.has_video() && video.is_valid() {
      let codec_info = match video.codec {
        Some(c) => CodecInfo {
          id: c.codec_id().to_string(),
          name: Some(c.display_name().to_string()),
          codec_private: video.codec_private.as_deref().map(CodecPrivate::from_bytes),
        },
        None => CodecInfo {
          id: "V_UNKNOWN".to_string(),
          name: Some("Unknown".to_string()),
          codec_private: None,
        },
      };
      let mut common = CommonTrackProperties::default();
      common.number = Some(1);
      let dims = video
        .width
        .zip(video.height)
        .map(|(w, h)| Dimensions2D { width: w, height: h });
      let mut vp = VideoTrackProperties {
        pixel_dimensions: dims,
        display_dimensions: dims,
        codec_config: video.codec_config.clone(),
        ..VideoTrackProperties::default()
      };
      if let Some(fps) = video.frame_rate {
        if fps > 0.0 {
          vp.default_duration_ns = Some((1_000_000_000.0 / fps).round() as u64);
        }
      }
      out.tracks.push(Track {
        id: track_id,
        track_type: TrackType::Video,
        codec: codec_info,
        properties: TrackProperties {
          common,
          video: Some(vp),
          ..TrackProperties::default()
        },
      });
      track_id += 1;
    }

    if header.has_audio() && audio.is_valid() {
      let mut common = CommonTrackProperties::default();
      common.number = Some((track_id as u64) + 1);
      let ap = AudioTrackProperties {
        sampling_frequency: audio.sample_rate.map(|r| r as f64),
        channels: audio.channels,
        bit_depth: audio.bit_depth,
        output_sampling_frequency: audio.codec_config.as_ref().and_then(|cfg| {
          if cfg.aac_sbr_present == Some(true) {
            audio.sample_rate.map(|r| (r * 2) as f64)
          } else {
            audio.sample_rate.map(|r| r as f64)
          }
        }),
        codec_config: audio.codec_config.clone(),
        ..AudioTrackProperties::default()
      };
      out.tracks.push(Track {
        id: track_id,
        track_type: TrackType::Audio,
        codec: CodecInfo {
          id: audio.codec_id.unwrap_or("A_UNKNOWN").to_string(),
          name: audio.codec_name.map(str::to_owned),
          codec_private: audio.codec_private.as_deref().map(CodecPrivate::from_bytes),
        },
        properties: TrackProperties {
          common,
          audio: Some(ap),
          ..TrackProperties::default()
        },
      });
    }

    Ok(())
  }
}

fn read_audio_payload(src: &mut FileSource, data_size: u32, state: &mut AudioState) -> Result<(), ParseError> {
  if data_size == 0 {
    return Ok(());
  }
  let mut payload = vec![0u8; data_size as usize];
  src.read_exact(&mut payload)?;
  let flags = AudioTagFlags::parse(payload[0]);
  match flags.format {
    2 | 14 => {
      state.codec_id.get_or_insert("A_MPEG/L3");
      state.codec_name.get_or_insert("MP3");
      if state.sample_rate.is_none() {
        state.sample_rate = if flags.format == 14 {
          Some(8_000)
        } else {
          flags.sample_rate()
        };
      }
      if state.channels.is_none() {
        state.channels = Some(flags.channels());
      }
      if state.bit_depth.is_none() {
        state.bit_depth = Some(flags.bits_per_sample() as u32);
      }
      state.headers_read = true;
    }
    10 => {
      state.codec_id.get_or_insert("A_AAC");
      state.codec_name.get_or_insert("AAC");
      if state.sample_rate.is_none() {
        if let Some(rate) = flags.sample_rate() {
          state.sample_rate = Some(rate);
        }
      }
      if state.channels.is_none() {
        state.channels = Some(flags.channels());
      }
      if payload.get(1) == Some(&0) && payload.len() > 2 {
        let asc = &payload[2..];
        state.codec_private = Some(asc.to_vec());
        if let Some(header) = aac::parse_audio_specific_config_bytes(asc) {
          if header.sample_rate > 0 {
            state.sample_rate = Some(header.sample_rate);
          }
          if header.channels > 0 {
            state.channels = Some(header.channels);
          }
          state.codec_name = Some("AAC");
          state.codec_config = Some(aac::codec_config_from_header(&header, asc));
          state.headers_read = true;
        }
      }
    }
    _ => {}
  };
  Ok(())
}

fn read_video_payload(src: &mut FileSource, data_size: u32, state: &mut VideoState) -> Result<(), ParseError> {
  if data_size == 0 {
    return Ok(());
  }
  let mut payload = vec![0u8; data_size as usize];
  src.read_exact(&mut payload)?;
  let header_byte = payload[0];
  let codec_id = VideoCodecId::from_byte(header_byte & 0x0F);
  if let Some(cid) = codec_id {
    state.codec.get_or_insert(cid);
    match cid {
      VideoCodecId::H264 => {
        if payload.get(1) == Some(&0) && payload.len() > 5 {
          let private = &payload[5..];
          let parsed = parse_avcc(private);
          state.codec_private = Some(private.to_vec());
          state.codec_config = parsed.config;
          if let Some(dim) = parsed.dimensions {
            state.width = Some(dim.width);
            state.height = Some(dim.height);
          }
          state.headers_read = parsed.complete;
        }
      }
      VideoCodecId::H265 => {
        if payload.get(1) == Some(&0) && payload.len() > 5 {
          let private = &payload[5..];
          let parsed = parse_hvcc(private);
          state.codec_private = Some(private.to_vec());
          state.codec_config = parsed.config;
          if let Some(dim) = parsed.dimensions {
            state.width = Some(dim.width);
            state.height = Some(dim.height);
          }
          state.headers_read = parsed.complete;
        }
      }
      VideoCodecId::SorensonH263 => {
        if state.width.is_none() || state.height.is_none() {
          if let Some(dim) = parse_flv1_dimensions(&payload[1..]) {
            state.width = Some(dim.width);
            state.height = Some(dim.height);
          }
        }
        state.headers_read = state.width.is_some() && state.height.is_some();
      }
      _ => {
        state.headers_read = true;
      }
    }
  }
  Ok(())
}

#[derive(Debug, Default)]
struct ParsedVideoConfig {
  dimensions: Option<Dimensions2D>,
  config: Option<VideoCodecConfig>,
  complete: bool,
}

fn parse_avcc(payload: &[u8]) -> ParsedVideoConfig {
  let raw_hex = Some(hex_encode(payload));
  if payload.len() < 6 {
    return ParsedVideoConfig {
      config: Some(VideoCodecConfig {
        raw_hex,
        is_elementary_stream: Some(false),
        ..VideoCodecConfig::default()
      }),
      ..ParsedVideoConfig::default()
    };
  }

  let profile_idc = payload[1];
  let level_idc = payload[3];
  let num_sps = payload[5] & 0x1f;
  let mut offset = 6usize;
  let mut sps_info: Option<avc_sps::AvcSps> = None;
  let mut saw_sps = false;
  for _ in 0..num_sps {
    if offset + 2 > payload.len() {
      break;
    }
    let len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
    offset += 2;
    if offset + len > payload.len() {
      break;
    }
    let nal = &payload[offset..offset + len];
    offset += len;
    if nal.first().map(|b| b & 0x1f) == Some(avc_nal::NAL_UNIT_TYPE_SPS) && nal.len() > 1 {
      saw_sps = true;
      let rbsp = avc_nal::strip_emulation_prevention(&nal[1..]);
      if let Ok(parsed) = avc_sps::parse(&rbsp) {
        sps_info = Some(parsed);
      }
    }
  }

  let mut saw_pps = false;
  if offset < payload.len() {
    let num_pps = payload[offset];
    offset += 1;
    for _ in 0..num_pps {
      if offset + 2 > payload.len() {
        break;
      }
      let len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
      offset += 2;
      if offset + len > payload.len() {
        break;
      }
      let nal = &payload[offset..offset + len];
      offset += len;
      if nal.first().map(|b| b & 0x1f) == Some(avc_nal::NAL_UNIT_TYPE_PPS) {
        saw_pps = true;
      }
    }
  }

  let mut config = VideoCodecConfig {
    profile_idc: Some(profile_idc as u32),
    profile_name: Some(avc_sps::format_profile(profile_idc).to_string()),
    level_idc: Some(level_idc as u32),
    level_name: Some(avc_sps::format_level(level_idc)),
    raw_hex,
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };
  let mut dimensions = None;
  if let Some(sps) = sps_info {
    dimensions = Some(Dimensions2D {
      width: sps.display_width,
      height: sps.display_height,
    });
    config.coded_dimensions = Some(Dimensions2D {
      width: sps.coded_width,
      height: sps.coded_height,
    });
    config.chroma_format = Some(classify_avc_chroma(sps.chroma_format_idc));
    config.bit_depth_luma = Some(sps.bit_depth_luma as u32);
    config.bit_depth_chroma = Some(sps.bit_depth_chroma as u32);
  }

  ParsedVideoConfig {
    dimensions,
    config: Some(config),
    complete: saw_sps && saw_pps,
  }
}

fn parse_hvcc(payload: &[u8]) -> ParsedVideoConfig {
  let raw_hex = Some(hex_encode(payload));
  if payload.len() < 23 {
    return ParsedVideoConfig {
      config: Some(VideoCodecConfig {
        raw_hex,
        is_elementary_stream: Some(false),
        ..VideoCodecConfig::default()
      }),
      ..ParsedVideoConfig::default()
    };
  }

  let profile_byte = payload[1];
  let tier_high = (profile_byte & 0x20) != 0;
  let profile_idc = profile_byte & 0x1f;
  let level_idc = payload[12];
  let mut config = VideoCodecConfig {
    profile_idc: Some(profile_idc as u32),
    profile_name: Some(hevc_sps::format_profile(profile_idc).to_string()),
    level_idc: Some(level_idc as u32),
    level_name: Some(hevc_sps::format_level(level_idc)),
    tier: Some(if tier_high {
      ModelHevcTier::High
    } else {
      ModelHevcTier::Main
    }),
    chroma_format: Some(classify_hevc_chroma(payload[18] & 0x03)),
    bit_depth_luma: Some(((payload[19] & 0x07) + 8) as u32),
    bit_depth_chroma: Some(((payload[20] & 0x07) + 8) as u32),
    raw_hex,
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };

  let mut dimensions = None;
  let mut saw_vps = false;
  let mut saw_sps = false;
  let mut saw_pps = false;
  let mut offset = 23usize;
  let num_arrays = payload[22] as usize;
  for _ in 0..num_arrays {
    if offset + 3 > payload.len() {
      break;
    }
    let nal_type = payload[offset] & 0x3f;
    let num_nalus = u16::from_be_bytes([payload[offset + 1], payload[offset + 2]]) as usize;
    offset += 3;
    for _ in 0..num_nalus {
      if offset + 2 > payload.len() {
        break;
      }
      let len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
      offset += 2;
      if offset + len > payload.len() {
        break;
      }
      let nal = &payload[offset..offset + len];
      offset += len;
      if nal_type == hevc_nal::NAL_UNIT_TYPE_VPS {
        saw_vps = true;
      } else if nal_type == hevc_nal::NAL_UNIT_TYPE_PPS {
        saw_pps = true;
      } else if nal_type == hevc_nal::NAL_UNIT_TYPE_SPS && nal.len() > 2 {
        saw_sps = true;
        let rbsp = hevc_nal::strip_emulation_prevention(&nal[2..]);
        if let Ok(sps) = hevc_sps::parse(&rbsp) {
          dimensions = Some(Dimensions2D {
            width: sps.display_width,
            height: sps.display_height,
          });
          config.profile_idc = Some(sps.profile_idc as u32);
          config.profile_name = Some(hevc_sps::format_profile(sps.profile_idc).to_string());
          config.level_idc = Some(sps.level_idc as u32);
          config.level_name = Some(hevc_sps::format_level(sps.level_idc));
          config.tier = Some(match sps.tier {
            hevc_sps::HevcTier::Main => ModelHevcTier::Main,
            hevc_sps::HevcTier::High => ModelHevcTier::High,
          });
          config.coded_dimensions = Some(Dimensions2D {
            width: sps.coded_width,
            height: sps.coded_height,
          });
          config.chroma_format = Some(classify_hevc_chroma(sps.chroma_format_idc));
          config.bit_depth_luma = Some(sps.bit_depth_luma as u32);
          config.bit_depth_chroma = Some(sps.bit_depth_chroma as u32);
        }
      }
    }
  }

  ParsedVideoConfig {
    dimensions,
    config: Some(config),
    complete: saw_vps && saw_sps && saw_pps,
  }
}

fn parse_flv1_dimensions(payload: &[u8]) -> Option<Dimensions2D> {
  let mut br = BitReader::new(payload);
  if br.read_bits(17).ok()? != 1 {
    return None;
  }
  let picture_format = br.read_bits(5).ok()?;
  if picture_format != 0 && picture_format != 1 {
    return None;
  }
  br.skip_bits(8).ok()?;
  let size_format = br.read_bits(3).ok()?;
  let (width, height) = match size_format {
    0 => (br.read_bits(8).ok()? as u32, br.read_bits(8).ok()? as u32),
    1 => (br.read_bits(16).ok()? as u32, br.read_bits(16).ok()? as u32),
    2 => (352, 288),
    3 => (176, 144),
    4 => (128, 96),
    5 => (320, 240),
    6 => (160, 120),
    _ => return None,
  };
  if width == 0 || height == 0 {
    return None;
  }
  Some(Dimensions2D { width, height })
}

fn classify_avc_chroma(idc: u8) -> ChromaFormat {
  match idc {
    0 => ChromaFormat::Monochrome,
    1 => ChromaFormat::Yuv420,
    2 => ChromaFormat::Yuv422,
    3 => ChromaFormat::Yuv444,
    _ => ChromaFormat::Other,
  }
}

fn classify_hevc_chroma(idc: u8) -> ChromaFormat {
  match idc {
    0 => ChromaFormat::Monochrome,
    1 => ChromaFormat::Yuv420,
    2 => ChromaFormat::Yuv422,
    3 => ChromaFormat::Yuv444,
    _ => ChromaFormat::Other,
  }
}

fn read_script_payload(src: &mut FileSource, data_size: u32, state: &mut VideoState) -> Result<(), ParseError> {
  if data_size == 0 {
    return Ok(());
  }
  let mut bytes = vec![0u8; data_size as usize];
  src.read_exact(&mut bytes)?;
  let meta = script_data::parse(&bytes);
  if let Some(w) = meta.number("width") {
    state.width.get_or_insert(w as u32);
  }
  if let Some(h) = meta.number("height") {
    state.height.get_or_insert(h as u32);
  }
  if let Some(fps) = meta.number("framerate") {
    if state.frame_rate.is_none() && fps > 0.0 {
      state.frame_rate = Some(fps);
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::flv::header::{TYPE_FLAG_AUDIO, TYPE_FLAG_VIDEO, build_header};
  use crate::media_metadata::flv::script_data::{AmfValue, build_on_meta_data};
  use crate::media_metadata::flv::tag::{TAG_AUDIO, TAG_SCRIPT, TAG_VIDEO};
  use std::io::Cursor;

  fn build_tag(tag_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u32.to_be_bytes()); // previous_tag_size
    buf.push(tag_type);
    let len = payload.len() as u32;
    buf.push(((len >> 16) & 0xFF) as u8);
    buf.push(((len >> 8) & 0xFF) as u8);
    buf.push((len & 0xFF) as u8);
    buf.extend_from_slice(&[0u8; 3]); // timestamp
    buf.push(0u8); // timestamp_ext
    buf.extend_from_slice(&[0u8; 3]); // stream id
    buf.extend_from_slice(payload);
    buf
  }

  fn minimal_avcc() -> Vec<u8> {
    let mut payload = vec![1, 66, 0, 40, 0xff, 0xe1];
    payload.extend_from_slice(&2u16.to_be_bytes());
    payload.extend_from_slice(&[0x67, 0x80]);
    payload.push(1);
    payload.extend_from_slice(&2u16.to_be_bytes());
    payload.extend_from_slice(&[0x68, 0x80]);
    payload
  }

  fn minimal_hvcc() -> Vec<u8> {
    let mut payload = vec![0u8; 23];
    payload[0] = 1;
    payload[1] = 1;
    payload[12] = 120;
    payload[18] = 0xfd;
    payload[19] = 0xf8;
    payload[20] = 0xf8;
    payload[22] = 3;
    for nal_type in [32u8, 33, 34] {
      payload.push(0x80 | nal_type);
      payload.extend_from_slice(&1u16.to_be_bytes());
      let nal_len = if nal_type == 33 { 3u16 } else { 2u16 };
      payload.extend_from_slice(&nal_len.to_be_bytes());
      payload.push((nal_type & 0x3f) << 1);
      payload.push(0x01);
      if nal_type == 33 {
        payload.push(0);
      }
    }
    payload
  }

  fn flv1_dimensions_payload(width: u16, height: u16) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.write_bits(1, 17);
    writer.write_bits(0, 5);
    writer.write_bits(0, 8);
    writer.write_bits(1, 3);
    writer.write_bits(width as u64, 16);
    writer.write_bits(height as u64, 16);
    writer.into_bytes()
  }

  struct BitWriter {
    bytes: Vec<u8>,
    bit_index: u8,
  }

  impl BitWriter {
    fn new() -> Self {
      Self {
        bytes: Vec::new(),
        bit_index: 0,
      }
    }

    fn write_bit(&mut self, bit: bool) {
      if self.bit_index == 0 {
        self.bytes.push(0);
      }
      if bit {
        let last = self.bytes.len() - 1;
        self.bytes[last] |= 1 << (7 - self.bit_index);
      }
      self.bit_index = (self.bit_index + 1) % 8;
    }

    fn write_bits(&mut self, value: u64, bits: u8) {
      for shift in (0..bits).rev() {
        self.write_bit(((value >> shift) & 1) != 0);
      }
    }

    fn into_bytes(self) -> Vec<u8> {
      self.bytes
    }
  }

  #[test]
  fn probe_accepts_minimal_flv_header() {
    let blob = build_header(1, TYPE_FLAG_VIDEO | TYPE_FLAG_AUDIO);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(FlvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_other_magic() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"XYZ\x01\x05\x00\x00\x00\x09".to_vec()));
    assert!(!FlvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_video_and_audio_tracks() {
    // FLV with one video tag (H.264) + one audio tag (AAC, 44.1k stereo)
    // + a script tag declaring dims and framerate.
    let mut blob = build_header(1, TYPE_FLAG_VIDEO | TYPE_FLAG_AUDIO);
    // Script tag with onMetaData
    let script_payload = build_on_meta_data(&[
      ("width", AmfValue::Number(1920.0)),
      ("height", AmfValue::Number(1080.0)),
      ("framerate", AmfValue::Number(30.0)),
    ]);
    blob.extend(build_tag(TAG_SCRIPT, &script_payload));
    // Video tag: byte = (key_frame<<4) | codec_id (7 = H.264), followed
    // by packet type 0 (AVC sequence header) + composition time + avcC.
    let mut video_payload = vec![(1 << 4) | 7, 0, 0, 0, 0];
    video_payload.extend(minimal_avcc());
    blob.extend(build_tag(TAG_VIDEO, &video_payload));
    // Audio tag: byte = (AAC<<4) | (44.1k<<2) | (16b<<1) | stereo
    let audio_byte = (10 << 4) | (3 << 2) | (1 << 1) | 1;
    blob.extend(build_tag(TAG_AUDIO, &[audio_byte, 0, 0x12, 0x10]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Flv);
    assert_eq!(out.tracks.len(), 2);
    let v = out.tracks.iter().find(|t| t.track_type == TrackType::Video).unwrap();
    assert_eq!(v.codec.id, "V_MPEG4/ISO/AVC");
    assert!(v.codec.codec_private.is_some());
    let vp = v.properties.video.as_ref().unwrap();
    assert_eq!(
      vp.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    assert_eq!(vp.default_duration_ns, Some(33_333_333));
    assert!(vp.codec_config.as_ref().unwrap().raw_hex.is_some());

    let a = out.tracks.iter().find(|t| t.track_type == TrackType::Audio).unwrap();
    assert_eq!(a.codec.id, "A_AAC");
    assert_eq!(a.codec.codec_private.as_ref().unwrap().hex, "1210");
    let ap = a.properties.audio.as_ref().unwrap();
    assert_eq!(ap.sampling_frequency, Some(44_100.0));
    assert_eq!(ap.channels, Some(2));
    assert_eq!(ap.codec_config.as_ref().unwrap().aac_object_type, Some(2));
  }

  #[test]
  fn read_headers_handles_audio_only_files() {
    let mut blob = build_header(1, TYPE_FLAG_AUDIO);
    let audio_byte = (2 << 4) | (3 << 2);
    blob.extend(build_tag(TAG_AUDIO, &[audio_byte, 0]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("audio.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_MPEG/L3");
  }

  #[test]
  fn read_headers_returns_no_tracks_when_payload_is_empty() {
    let blob = build_header(1, 0); // neither flag set
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("empty.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Flv);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_rejects_invalid_header() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 32]));
    let mut out = MediaMetadata::new("not-flv", 0);
    let err = FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn read_headers_recognises_h265_video_tag() {
    let mut blob = build_header(1, TYPE_FLAG_VIDEO);
    // Video tag: byte = (key_frame<<4) | codec_id (12 = H.265)
    let mut video_payload = vec![(1 << 4) | 12, 0, 0, 0, 0];
    video_payload.extend(minimal_hvcc());
    blob.extend(build_tag(TAG_VIDEO, &video_payload));
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks.iter().find(|t| t.track_type == TrackType::Video).unwrap();
    assert_eq!(v.codec.id, "V_MPEGH/ISO/HEVC");
    assert!(v.codec.codec_private.is_some());
  }

  #[test]
  fn read_headers_skips_unsupported_audio_formats() {
    let mut blob = build_header(1, TYPE_FLAG_AUDIO);
    let audio_byte = (6 << 4) | (3 << 2) | (1 << 1) | 1;
    blob.extend(build_tag(TAG_AUDIO, &[audio_byte, 0]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("nelly.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_extracts_sorenson_dimensions_from_payload() {
    let mut blob = build_header(1, TYPE_FLAG_VIDEO);
    let mut payload = vec![(1 << 4) | 2];
    payload.extend(flv1_dimensions_payload(640, 360));
    blob.extend(build_tag(TAG_VIDEO, &payload));
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("flv1.flv", 0);
    FlvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 640,
        height: 360
      })
    );
  }
}
