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

//! Dirac elementary stream reader.
//!
//! The stream begins with parse-info blocks whose magic is `BBCD` followed
//! by a one-byte parse-code. Sequence-header parse-code = `0x00`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, InterlaceFlag, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

pub const PARSE_INFO_MAGIC: [u8; 4] = *b"BBCD";
const PROBE_BYTES: usize = 1024 * 1024;
const PARSE_INFO_HEADER_LEN: usize = 13;
const SEQUENCE_HEADER_CODE: u8 = 0x00;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SequenceHeader {
  pixel_width: u32,
  pixel_height: u32,
  interlaced: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DiracReader;

impl Reader for DiracReader {
  fn name(&self) -> &'static str {
    "dirac"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    Ok(starts_with_sync(&head[..read]) && parse_sequence_header(&head[..read]).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut head = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut head)?;
    if !starts_with_sync(&head[..read]) {
      return Err(ParseError::Unrecognised);
    }
    let sequence = parse_sequence_header(&head[..read]).ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Dirac;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_DIRAC".to_string(),
        name: Some("Dirac".to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        video: Some(VideoTrackProperties {
          pixel_dimensions: Some(Dimensions2D {
            width: sequence.pixel_width,
            height: sequence.pixel_height,
          }),
          display_dimensions: Some(Dimensions2D {
            width: sequence.pixel_width,
            height: sequence.pixel_height,
          }),
          interlace: Some(if sequence.interlaced {
            InterlaceFlag::Interlaced
          } else {
            InterlaceFlag::Progressive
          }),
          ..VideoTrackProperties::default()
        }),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

/// PARSER-222: mkvtoolnix's `dirac_es_reader_c::probe_file()` requires the
/// stream to *start* with the Dirac sync word (`get_uint32_be(buffer) ==
/// SYNC_WORD`, `../mkvtoolnix/src/input/r_dirac.cpp:33-35`) before handing
/// the data to the parser.  Searching for a `BBCD` sequence header anywhere
/// in the prefix turned unrelated files that happen to contain a later
/// sequence-header-shaped blob into false positives.
fn starts_with_sync(bytes: &[u8]) -> bool {
  bytes.len() >= 4 && bytes[..4] == PARSE_INFO_MAGIC
}

fn parse_sequence_header(bytes: &[u8]) -> Option<SequenceHeader> {
  let pos = bytes
    .windows(5)
    .position(|w| w[..4] == PARSE_INFO_MAGIC && w[4] == SEQUENCE_HEADER_CODE)?;
  let payload = bytes.get(pos..)?;
  if payload.len() < PARSE_INFO_HEADER_LEN {
    return None;
  }
  let mut br = BitReader::new(&payload[PARSE_INFO_HEADER_LEN..]);
  let _major = read_uint(&mut br).ok()?;
  let _minor = read_uint(&mut br).ok()?;
  let _profile = read_uint(&mut br).ok()?;
  let _level = read_uint(&mut br).ok()?;
  let base_video_format = read_uint(&mut br).ok()?;
  let mut sequence = standard_video_format(base_video_format);
  if br.read_bit().ok()? {
    sequence.pixel_width = read_uint(&mut br).ok()?;
    sequence.pixel_height = read_uint(&mut br).ok()?;
  }
  if br.read_bit().ok()? {
    let _chroma_format = read_uint(&mut br).ok()?;
  }
  if br.read_bit().ok()? {
    sequence.interlaced = br.read_bit().ok()?;
    if sequence.interlaced {
      let _top_field_first = br.read_bit().ok()?;
    }
  }
  Some(sequence)
}

fn read_uint(br: &mut BitReader<'_>) -> Result<u32, ParseError> {
  let mut count = 0u32;
  let mut value = 0u32;
  while !br.read_bit()? {
    count += 1;
    value <<= 1;
    value |= u32::from(br.read_bit()?);
    if count > 31 {
      return Err(ParseError::Malformed {
        format: "dirac",
        offset: br.position_bytes(),
        reason: "Dirac uint exceeds 31 continuation pairs".to_string(),
      });
    }
  }
  Ok((1u32 << count) - 1 + value)
}

fn standard_video_format(index: u32) -> SequenceHeader {
  match index {
    1 => SequenceHeader {
      pixel_width: 176,
      pixel_height: 120,
      interlaced: false,
    },
    2 => SequenceHeader {
      pixel_width: 176,
      pixel_height: 144,
      interlaced: false,
    },
    7 => SequenceHeader {
      pixel_width: 720,
      pixel_height: 480,
      interlaced: true,
    },
    8 => SequenceHeader {
      pixel_width: 720,
      pixel_height: 576,
      interlaced: true,
    },
    9 | 10 => SequenceHeader {
      pixel_width: 1280,
      pixel_height: 720,
      interlaced: false,
    },
    11 | 12 => SequenceHeader {
      pixel_width: 1920,
      pixel_height: 1080,
      interlaced: true,
    },
    13 | 14 | 21 => SequenceHeader {
      pixel_width: 1920,
      pixel_height: 1080,
      interlaced: false,
    },
    _ => SequenceHeader {
      pixel_width: 640,
      pixel_height: 480,
      interlaced: false,
    },
  }
}

#[cfg(test)]
pub(crate) fn build_dirac_stream() -> Vec<u8> {
  let mut bytes = PARSE_INFO_MAGIC.to_vec();
  bytes.push(0x00); // sequence-header parse-code
  bytes.extend_from_slice(&[0u8; 8]); // next / previous parse offsets
  let mut writer = DiracUintWriter::new();
  writer.write_uint(2); // major_version
  writer.write_uint(2); // minor_version
  writer.write_uint(0); // profile
  writer.write_uint(0); // level
  writer.write_uint(0); // base video format
  writer.write_bit(true); // custom dimensions
  writer.write_uint(1920);
  writer.write_uint(1080);
  writer.write_bit(false); // chroma_format_flag
  writer.write_bit(true); // scan_format_flag
  writer.write_bit(false); // progressive
  writer.write_bit(false); // frame_rate_flag
  writer.write_bit(false); // aspect_ratio_flag
  writer.write_bit(false); // clean_area_flag
  bytes.extend(writer.into_bytes());
  bytes
}

#[cfg(test)]
struct DiracUintWriter {
  bytes: Vec<u8>,
  bit_index: u8,
}

#[cfg(test)]
impl DiracUintWriter {
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

  fn write_uint(&mut self, value: u32) {
    let mut count = 0u32;
    loop {
      let base = (1u32 << count) - 1;
      let capacity = 1u32 << count;
      if value >= base && value < base + capacity {
        let remainder = value - base;
        for shift in (0..count).rev() {
          self.write_bit(false);
          self.write_bit(((remainder >> shift) & 1) != 0);
        }
        self.write_bit(true);
        break;
      }
      count += 1;
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
  fn probe_accepts_bbcd_magic_plus_sequence_header() {
    let bytes = build_dirac_stream();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(DiracReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_wrong_parse_code() {
    let mut bytes = build_dirac_stream();
    bytes[4] = 0x10; // not sequence header
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!DiracReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"BBC".to_vec()));
    assert!(!DiracReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_dirac_track() {
    let bytes = build_dirac_stream();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.drc", 0);
    DiracReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Dirac);
    assert_eq!(out.tracks[0].codec.id, "V_DIRAC");
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    assert_eq!(video.interlace, Some(InterlaceFlag::Progressive));
  }

  #[test]
  fn probe_rejects_sequence_header_not_at_stream_start() {
    // PARSER-222: unrelated leading bytes before a later BBCD sequence
    // header must not be claimed — the stream has to start with the sync.
    let mut bytes = vec![0xAAu8; 16];
    bytes.extend(build_dirac_stream());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!DiracReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_rejects_sequence_header_not_at_stream_start() {
    let mut bytes = vec![0xAAu8; 16];
    bytes.extend(build_dirac_stream());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.drc", 0);
    let err = DiracReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn read_headers_rejects_non_dirac_input() {
    let bytes = vec![0xAAu8; 16];
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.drc", 0);
    let err = DiracReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
