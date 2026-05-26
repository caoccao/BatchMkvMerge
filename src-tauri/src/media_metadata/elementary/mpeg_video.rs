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

//! MPEG-1 / MPEG-2 video elementary stream reader.
//!
//! Sequence header (ISO/IEC 11172-2 §2.4.2.3 + 13818-2 §6.2.2.1):
//!
//! ```text
//! 0x00 0x00 0x01 0xB3                 (sequence_header_code)
//! 12 bits horizontal_size
//! 12 bits vertical_size
//! 4  bits aspect_ratio
//! 4  bits frame_rate_code
//! 18 bits bit_rate
//! 1  bit  marker
//! 10 bits vbv_buffer_size
//! 1  bit  constrained
//! 1  bit  load_intra_quantiser_matrix
//! [64 bytes intra matrix if flag set]
//! 1  bit  load_non_intra_quantiser_matrix
//! [64 bytes non-intra matrix if flag set]
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
  Dimensions2D, InterlaceFlag, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 1024 * 1024;
const SEQUENCE_HEADER_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0xB3];
const EXTENSION_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0xB5];
const PICTURE_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0x00];
#[cfg(test)]
const GOP_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0xB8];

const FRAME_RATE_TABLE: [(u32, u32); 16] = [
  (0, 1),
  (24_000, 1001),
  (24, 1),
  (25, 1),
  (30_000, 1001),
  (30, 1),
  (50, 1),
  (60_000, 1001),
  (60, 1),
  (0, 1),
  (0, 1),
  (0, 1),
  (0, 1),
  (0, 1),
  (0, 1),
  (0, 1),
];

#[derive(Debug, Clone, Copy)]
pub struct SequenceHeader {
  pub horizontal_size: u32,
  pub vertical_size: u32,
  pub aspect_ratio_code: u8,
  pub frame_rate_num: u32,
  pub frame_rate_den: u32,
  pub version: u8,
  pub progressive: Option<bool>,
}

pub fn find_sequence_header(bytes: &[u8]) -> Option<usize> {
  bytes.windows(4).position(|w| w == SEQUENCE_HEADER_CODE)
}

pub fn decode_sequence_header(bytes: &[u8]) -> Option<SequenceHeader> {
  let pos = find_sequence_header(bytes)?;
  let body = bytes.get(pos + 4..pos + 4 + 8)?;
  // 12 + 12 + 4 + 4 = 32 bits = first 4 bytes
  let horizontal_size = ((body[0] as u32) << 4) | ((body[1] as u32) >> 4);
  let vertical_size = (((body[1] as u32) & 0x0F) << 8) | body[2] as u32;
  let aspect_ratio_code = (body[3] >> 4) & 0x0F;
  let frame_rate_code = (body[3] & 0x0F) as usize;
  let (num, den) = FRAME_RATE_TABLE[frame_rate_code];
  let mut header = SequenceHeader {
    horizontal_size,
    vertical_size,
    aspect_ratio_code,
    frame_rate_num: num,
    frame_rate_den: den,
    version: 1,
    progressive: None,
  };
  apply_sequence_extension(bytes, pos, &mut header);
  Some(header)
}

pub fn frame_duration_ns(num: u32, den: u32) -> Option<u64> {
  if num == 0 {
    return None;
  }
  Some((den as u128 * 1_000_000_000 / num as u128) as u64)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegVideoReader;

impl Reader for MpegVideoReader {
  fn name(&self) -> &'static str {
    "mpeg_video"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read >= 4 && looks_like_mpeg_video_es(&buf[..read]))
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
    let header = decode_sequence_header(&buf[..read]).ok_or(ParseError::Unrecognised)?;
    if !looks_like_mpeg_video_es(&buf[..read]) {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::MpegVideo;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let pixel_dimensions = Dimensions2D {
      width: header.horizontal_size,
      height: header.vertical_size,
    };
    let video = VideoTrackProperties {
      pixel_dimensions: Some(pixel_dimensions),
      display_dimensions: Some(display_dimensions(&header).unwrap_or(pixel_dimensions)),
      interlace: header.progressive.map(|progressive| {
        if progressive {
          InterlaceFlag::Progressive
        } else {
          InterlaceFlag::Interlaced
        }
      }),
      default_duration_ns: frame_duration_ns(header.frame_rate_num, header.frame_rate_den),
      codec_config: Some(VideoCodecConfig {
        profile_name: Some(if header.version == 2 {
          "MPEG-2 Video".to_string()
        } else {
          "MPEG-1 Video".to_string()
        }),
        is_elementary_stream: Some(true),
        ..VideoCodecConfig::default()
      }),
      ..VideoTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: if header.version == 2 {
          "V_MPEG2".to_string()
        } else {
          "V_MPEG1".to_string()
        },
        name: Some("MPEG-1/2 Video".to_string()),
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

fn apply_sequence_extension(bytes: &[u8], sequence_pos: usize, header: &mut SequenceHeader) {
  let Some(ext_rel) = bytes
    .get(sequence_pos + 4..)
    .and_then(|tail| tail.windows(4).position(|w| w == EXTENSION_START_CODE))
  else {
    return;
  };
  let ext_pos = sequence_pos + 4 + ext_rel;
  let Some(body) = bytes.get(ext_pos + 4..) else {
    return;
  };
  let mut br = BitReader::new(body);
  let Ok(ext_id) = br.read_bits(4) else {
    return;
  };
  if ext_id != 1 {
    return;
  }
  header.version = 2;
  let _ = br.read_bits(8);
  let progressive = br.read_bit().ok();
  header.progressive = progressive;
}

fn looks_like_mpeg_video_es(bytes: &[u8]) -> bool {
  if is_transport_stream(bytes) || bytes.starts_with(&[0x00, 0x00, 0x01, 0xba]) {
    return false;
  }

  let start_code_at_beginning = bytes.len() >= 4 && is_start_code(&bytes[..4]);
  let mut sequence_start_code_found = false;
  let mut picture_start_code_found = false;
  let mut gop_start_code_found = false;
  let mut ext_start_code_found = false;
  let mut slice_start_codes = 0u32;

  let mut pos = 0usize;
  while pos + 4 <= bytes.len() {
    if is_start_code(&bytes[pos..pos + 4]) {
      let code = bytes[pos + 3];
      match code {
        0xB3 => sequence_start_code_found = true,
        0x00 => picture_start_code_found = true,
        0xB8 => gop_start_code_found = true,
        0xB5 => ext_start_code_found = true,
        0x01..=0xAF => slice_start_codes += 1,
        _ => {}
      }
      if sequence_start_code_found
        && picture_start_code_found
        && ((slice_start_codes > 0 && start_code_at_beginning)
          || (slice_start_codes > 0 && gop_start_code_found && ext_start_code_found)
          || slice_start_codes >= 25)
        && mpeg_frame_probe_valid(bytes)
      {
        return true;
      }
      pos += 4;
    } else {
      pos += 1;
    }
  }
  false
}

fn mpeg_frame_probe_valid(bytes: &[u8]) -> bool {
  let Some(header) = decode_sequence_header(bytes) else {
    return false;
  };
  if header.horizontal_size == 0 || header.vertical_size == 0 || header.frame_rate_num == 0 {
    return false;
  }
  let Some(sequence_pos) = find_sequence_header(bytes) else {
    return false;
  };
  let mut saw_picture = false;
  let mut saw_slice = false;
  let mut saw_valid_picture_header = false;
  let mut pos = sequence_pos + 4;
  while pos + 4 <= bytes.len() {
    if bytes[pos..pos + 3] == [0x00, 0x00, 0x01] {
      let code = bytes[pos + 3];
      if bytes[pos..pos + 4] == PICTURE_START_CODE {
        saw_picture = true;
        saw_valid_picture_header = valid_picture_header(bytes.get(pos + 4..));
      } else if (0x01..=0xaf).contains(&code) && saw_picture {
        saw_slice = true;
        break;
      }
      pos += 4;
    } else {
      pos += 1;
    }
  }
  saw_picture && saw_valid_picture_header && saw_slice
}

fn valid_picture_header(body: Option<&[u8]>) -> bool {
  let Some(body) = body else {
    return false;
  };
  if body.len() < 2 {
    return false;
  }
  let picture_coding_type = (body[1] >> 3) & 0x07;
  (1..=4).contains(&picture_coding_type)
}

fn is_start_code(bytes: &[u8]) -> bool {
  bytes.len() >= 4 && bytes[0..3] == [0x00, 0x00, 0x01]
}

fn is_transport_stream(bytes: &[u8]) -> bool {
  bytes.first() == Some(&0x47)
    && bytes.get(188) == Some(&0x47)
    && match bytes.get(376) {
      Some(b) => *b == 0x47,
      None => true,
    }
}

fn display_dimensions(header: &SequenceHeader) -> Option<Dimensions2D> {
  let (num, den) = match header.aspect_ratio_code {
    1 => {
      return Some(Dimensions2D {
        width: header.horizontal_size,
        height: header.vertical_size,
      });
    }
    2 => (4u32, 3u32),
    3 => (16, 9),
    4 => (221, 100),
    _ => return None,
  };
  Some(Dimensions2D {
    width: ((header.vertical_size as u64 * num as u64 + den as u64 / 2) / den as u64) as u32,
    height: header.vertical_size,
  })
}

#[cfg(test)]
pub(crate) fn build_sequence_header(width: u32, height: u32, frame_rate_code: u8) -> Vec<u8> {
  let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
  bytes.push(((width >> 4) & 0xFF) as u8);
  bytes.push((((width & 0x0F) << 4) | ((height >> 8) & 0x0F)) as u8);
  bytes.push((height & 0xFF) as u8);
  bytes.push((1u8 << 4) | (frame_rate_code & 0x0F));
  bytes.extend_from_slice(&[0u8; 4]); // bitrate + markers
  bytes
}

#[cfg(test)]
fn build_es(width: u32, height: u32, frame_rate_code: u8) -> Vec<u8> {
  let mut bytes = build_sequence_header(width, height, frame_rate_code);
  bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x08]);
  bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x01, 0x80]);
  bytes
}

#[cfg(test)]
fn sequence_extension(progressive: bool) -> Vec<u8> {
  sequence_extension_with_ignored_size_and_frame_rate(progressive, 0, 0, 0, 0)
}

#[cfg(test)]
fn sequence_extension_with_ignored_size_and_frame_rate(
  progressive: bool,
  horizontal_ext: u64,
  vertical_ext: u64,
  frame_rate_ext_n: u64,
  frame_rate_ext_d: u64,
) -> Vec<u8> {
  let mut w = BitWriter::new();
  w.write_bits(1, 4);
  w.write_bits(0, 8);
  w.write_bit(progressive);
  w.write_bits(1, 2);
  w.write_bits(horizontal_ext, 2);
  w.write_bits(vertical_ext, 2);
  w.write_bits(0, 12);
  w.write_bit(true);
  w.write_bits(0, 8);
  w.write_bit(false);
  w.write_bits(frame_rate_ext_n, 2);
  w.write_bits(frame_rate_ext_d, 5);
  let mut bytes = EXTENSION_START_CODE.to_vec();
  bytes.extend(w.into_bytes());
  bytes
}

#[cfg(test)]
struct BitWriter {
  bytes: Vec<u8>,
  bit_index: u8,
}

#[cfg(test)]
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

  fn write_bits(&mut self, value: u64, bits: u32) {
    for shift in (0..bits).rev() {
      self.write_bit(((value >> shift) & 1) != 0);
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
  fn decodes_1920x1080_30fps_sequence_header() {
    let bytes = build_sequence_header(1920, 1080, 5);
    let h = decode_sequence_header(&bytes).unwrap();
    assert_eq!(h.horizontal_size, 1920);
    assert_eq!(h.vertical_size, 1080);
    assert_eq!(h.frame_rate_num, 30);
    assert_eq!(h.frame_rate_den, 1);
  }

  #[test]
  fn decodes_23976_fps() {
    let bytes = build_sequence_header(1920, 1080, 1);
    let h = decode_sequence_header(&bytes).unwrap();
    assert_eq!(h.frame_rate_num, 24_000);
    assert_eq!(h.frame_rate_den, 1001);
  }

  #[test]
  fn find_sequence_header_skips_garbage() {
    let mut bytes = vec![0xAAu8; 16];
    bytes.extend(build_sequence_header(640, 480, 3));
    assert_eq!(find_sequence_header(&bytes), Some(16));
  }

  #[test]
  fn find_sequence_header_returns_none() {
    assert!(find_sequence_header(&[0xAAu8; 16]).is_none());
  }

  #[test]
  fn probe_accepts_bounded_es_pattern_at_start() {
    let bytes = build_es(640, 480, 3);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegVideoReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_one_slice_pattern_with_leading_bytes() {
    let mut prefixed = vec![0xAAu8; 4];
    prefixed.extend(build_es(640, 480, 3));
    let mut s = FileSource::from_reader_for_test(Cursor::new(prefixed));
    assert!(!MpegVideoReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_prefixed_gop_extension_slice_pattern() {
    let mut bytes = vec![0xAAu8; 4];
    bytes.extend(build_sequence_header(640, 480, 3));
    bytes.extend_from_slice(&GOP_START_CODE);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    bytes.extend(sequence_extension(true));
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x08]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x01, 0x80]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegVideoReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_prefixed_stream_with_many_slices() {
    let mut bytes = vec![0xAAu8; 4];
    bytes.extend(build_sequence_header(640, 480, 3));
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x08]);
    for code in 1..=25u8 {
      bytes.extend_from_slice(&[0x00, 0x00, 0x01, code, 0x80]);
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegVideoReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_isolated_sequence_header() {
    let bytes = build_sequence_header(640, 480, 3);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!MpegVideoReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_video_track() {
    let bytes = build_es(1280, 720, 5);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpv", 0);
    MpegVideoReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::MpegVideo);
    assert_eq!(out.tracks[0].codec.id, "V_MPEG1");
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1280,
        height: 720
      })
    );
  }

  #[test]
  fn read_headers_uses_sequence_extension_for_mpeg2_interlace() {
    let mut bytes = build_sequence_header(720, 576, 3);
    bytes[7] = (3 << 4) | 3; // 16:9 DAR, 25 fps
    bytes.extend(sequence_extension(false));
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x00, 0x08]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x01, 0x01, 0x80]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.m2v", 0);
    MpegVideoReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "V_MPEG2");
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(video.interlace, Some(InterlaceFlag::Interlaced));
    assert_eq!(
      video.display_dimensions,
      Some(Dimensions2D {
        width: 1024,
        height: 576
      })
    );
  }

  #[test]
  fn sequence_extension_size_and_frame_rate_factors_are_ignored() {
    let mut bytes = build_sequence_header(720, 576, 3);
    bytes.extend(sequence_extension_with_ignored_size_and_frame_rate(true, 3, 2, 2, 4));
    let h = decode_sequence_header(&bytes).unwrap();
    assert_eq!(h.version, 2);
    assert_eq!(h.progressive, Some(true));
    assert_eq!(h.horizontal_size, 720);
    assert_eq!(h.vertical_size, 576);
    assert_eq!(h.frame_rate_num, 25);
    assert_eq!(h.frame_rate_den, 1);
  }

  #[test]
  fn frame_duration_ns_for_known_rates() {
    assert_eq!(frame_duration_ns(30, 1), Some(33_333_333));
    assert_eq!(frame_duration_ns(24_000, 1001), Some(41_708_333));
    assert!(frame_duration_ns(0, 1).is_none());
  }
}
