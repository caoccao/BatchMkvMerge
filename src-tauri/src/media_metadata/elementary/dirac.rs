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
  top_field_first: bool,
  frame_rate_numerator: u32,
  frame_rate_denominator: u32,
  aspect_ratio_numerator: u32,
  aspect_ratio_denominator: u32,
}

impl SequenceHeader {
  /// Display dimensions after applying the sample aspect ratio, mirroring
  /// `dirac_video_packetizer_c::set_headers` (`p_dirac.cpp`).
  fn display_dimensions(&self) -> (u32, u32) {
    let mut w = self.pixel_width;
    let mut h = self.pixel_height;
    let (num, den) = (self.aspect_ratio_numerator as u64, self.aspect_ratio_denominator as u64);
    if num != 0 && den != 0 {
      if num > den {
        w = (((w as u64) * num + den / 2) / den) as u32;
      } else {
        h = (((h as u64) * den + num / 2) / num) as u32;
      }
    }
    (w, h)
  }

  /// Default frame duration in ns from the frame-rate syntax
  /// (`dirac.cpp:366-367`: `1e9 * frame_rate_denominator / frame_rate_numerator`).
  fn default_duration_ns(&self) -> Option<u64> {
    if self.frame_rate_numerator != 0 && self.frame_rate_denominator != 0 {
      Some(1_000_000_000u64 * self.frame_rate_denominator as u64 / self.frame_rate_numerator as u64)
    } else {
      None
    }
  }
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

    let (display_width, display_height) = sequence.display_dimensions();

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
            width: display_width,
            height: display_height,
          }),
          default_duration_ns: sequence.default_duration_ns(),
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
  let mut search_from = 0usize;
  while search_from + 5 <= bytes.len() {
    let rel = bytes[search_from..]
      .windows(5)
      .position(|w| w[..4] == PARSE_INFO_MAGIC && w[4] == SEQUENCE_HEADER_CODE)?;
    let pos = search_from + rel;
    let Some(payload) = complete_parse_unit(bytes, pos) else {
      search_from = pos + 1;
      continue;
    };
    if let Some(sequence) = parse_sequence_header_unit(payload) {
      return Some(sequence);
    }
    search_from = pos + 1;
  }
  None
}

fn complete_parse_unit(bytes: &[u8], pos: usize) -> Option<&[u8]> {
  if pos + PARSE_INFO_HEADER_LEN > bytes.len() || bytes.get(pos..pos + 4)? != PARSE_INFO_MAGIC {
    return None;
  }
  let next_parse_offset = u32::from_be_bytes([bytes[pos + 5], bytes[pos + 6], bytes[pos + 7], bytes[pos + 8]]) as usize;
  let mut search_from = pos + 4;
  while search_from + 4 <= bytes.len() {
    let rel = bytes[search_from..].windows(4).position(|w| w == PARSE_INFO_MAGIC)?;
    let next_sync = search_from + rel;
    if next_parse_offset == 0 || pos.checked_add(next_parse_offset).is_some_and(|end| end <= next_sync) {
      return bytes.get(pos..next_sync);
    }
    search_from = next_sync + 1;
  }
  None
}

fn parse_sequence_header_unit(payload: &[u8]) -> Option<SequenceHeader> {
  if payload.len() < PARSE_INFO_HEADER_LEN {
    return None;
  }
  let mut br = BitReader::new(&payload[PARSE_INFO_HEADER_LEN..]);
  let _major = read_uint(&mut br).ok()?;
  let _minor = read_uint(&mut br).ok()?;
  let _profile = read_uint(&mut br).ok()?;
  let _level = read_uint(&mut br).ok()?;

  // base_video_format indexes the standard-format table directly; an
  // out-of-range value falls back to format 0 (`dirac.cpp:142-145`).
  let mut base_video_format = read_uint(&mut br).ok()? as usize;
  if base_video_format >= STANDARD_VIDEO_FORMATS.len() {
    base_video_format = 0;
  }
  let mut sequence = standard_video_format(base_video_format);

  // Custom source dimensions.
  if br.read_bit().ok()? {
    sequence.pixel_width = read_uint(&mut br).ok()?;
    sequence.pixel_height = read_uint(&mut br).ok()?;
  }
  // Custom chroma format (parsed for bit alignment only).
  if br.read_bit().ok()? {
    let _chroma_format = read_uint(&mut br).ok()?;
  }
  // Custom scan format.
  if br.read_bit().ok()? {
    sequence.interlaced = br.read_bit().ok()?;
    if sequence.interlaced {
      sequence.top_field_first = br.read_bit().ok()?;
    }
  }
  // Custom frame rate.
  if br.read_bit().ok()? {
    let index = read_uint(&mut br).ok()? as usize;
    if index == 0 {
      sequence.frame_rate_numerator = read_uint(&mut br).ok()?;
      sequence.frame_rate_denominator = read_uint(&mut br).ok()?;
    } else if index < STANDARD_FRAME_RATES.len() {
      let (num, den) = STANDARD_FRAME_RATES[index];
      sequence.frame_rate_numerator = num;
      sequence.frame_rate_denominator = den;
    }
  }
  // Custom pixel aspect ratio.
  if br.read_bit().ok()? {
    let index = read_uint(&mut br).ok()? as usize;
    if index == 0 {
      sequence.aspect_ratio_numerator = read_uint(&mut br).ok()?;
      sequence.aspect_ratio_denominator = read_uint(&mut br).ok()?;
    } else if index < STANDARD_ASPECT_RATIOS.len() {
      let (num, den) = STANDARD_ASPECT_RATIOS[index];
      sequence.aspect_ratio_numerator = num;
      sequence.aspect_ratio_denominator = den;
    }
  }
  // Custom clean area (parsed for bit alignment; not surfaced).
  if br.read_bit().ok()? {
    let _clean_width = read_uint(&mut br).ok()?;
    let _clean_height = read_uint(&mut br).ok()?;
    let _left_offset = read_uint(&mut br).ok()?;
    let _top_offset = read_uint(&mut br).ok()?;
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

/// `standard_video_formats[23]` from `common/dirac.cpp:75-99`, indexed directly
/// by `base_video_format`.  Tuple = `(pixel_width, pixel_height, interlaced,
/// top_field_first, frame_rate_num, frame_rate_den, aspect_ratio_num,
/// aspect_ratio_den)`.
#[rustfmt::skip]
const STANDARD_VIDEO_FORMATS: [(u32, u32, bool, bool, u32, u32, u32, u32); 23] = [
  ( 640,  480, false, false, 24000, 1001,  1,  1),
  ( 176,  120, false, false, 15000, 1001, 10, 11),
  ( 176,  144, false,  true,    25,    2, 12, 11),
  ( 352,  240, false, false, 15000, 1001, 10, 11),
  ( 352,  288, false,  true,    25,    2, 12, 11),
  ( 704,  480, false, false, 15000, 1001, 10, 11),
  ( 704,  576, false,  true,    25,    2, 12, 11),
  ( 720,  480,  true, false, 30000, 1001, 10, 11),
  ( 720,  576,  true,  true,    25,    1, 12, 11),
  (1280,  720, false,  true, 60000, 1001,  1,  1),
  (1280,  720, false,  true,    50,    1,  1,  1),
  (1920, 1080,  true,  true, 30000, 1001,  1,  1),
  (1920, 1080,  true,  true,    25,    1,  1,  1),
  (1920, 1080, false,  true, 60000, 1001,  1,  1),
  (1920, 1080, false,  true,    50,    1,  1,  1),
  (2048, 1080, false,  true,    24,    1,  1,  1),
  (4096, 2160, false,  true,    24,    1,  1,  1),
  (3840, 2160, false,  true, 60000, 1001,  1,  1),
  (3840, 2160, false,  true,    50,    1,  1,  1),
  (7680, 4320, false,  true, 60000, 1001,  1,  1),
  (7680, 4320, false,  true,    50,    1,  1,  1),
  (1920, 1080, false,  true, 24000, 1001,  1,  1),
  ( 720,  486,  true, false, 30000, 1001, 10, 11),
];

/// `standard_frame_rates[11]` from `common/dirac.cpp:101-113`, indexed 1..=10.
#[rustfmt::skip]
const STANDARD_FRAME_RATES: [(u32, u32); 11] = [
  (    0,    0),
  (24000, 1001),
  (   24,    1),
  (   25,    1),
  (30000, 1001),
  (   30,    1),
  (   50,    1),
  (60000, 1001),
  (   60,    1),
  (15000, 1001),
  (   25,    2),
];

/// `standard_aspect_ratios[7]` from `common/dirac.cpp:115-123`, indexed 1..=6.
#[rustfmt::skip]
const STANDARD_ASPECT_RATIOS: [(u32, u32); 7] = [
  ( 0,  0),
  ( 1,  1),
  (10, 11),
  (12, 11),
  (40, 33),
  (16, 11),
  ( 4,  3),
];

fn standard_video_format(index: usize) -> SequenceHeader {
  let (w, h, interlaced, tff, frn, frd, arn, ard) = STANDARD_VIDEO_FORMATS[index];
  SequenceHeader {
    pixel_width: w,
    pixel_height: h,
    interlaced,
    top_field_first: tff,
    frame_rate_numerator: frn,
    frame_rate_denominator: frd,
    aspect_ratio_numerator: arn,
    aspect_ratio_denominator: ard,
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
  bytes.extend_from_slice(&PARSE_INFO_MAGIC);
  bytes
}

/// Build a Dirac sequence header that selects `base_video_format` with no
/// per-field overrides, so the standard-format table values are used verbatim.
#[cfg(test)]
pub(crate) fn build_dirac_base_format(base: u32) -> Vec<u8> {
  let mut bytes = PARSE_INFO_MAGIC.to_vec();
  bytes.push(0x00);
  bytes.extend_from_slice(&[0u8; 8]);
  let mut writer = DiracUintWriter::new();
  writer.write_uint(2); // major
  writer.write_uint(2); // minor
  writer.write_uint(0); // profile
  writer.write_uint(0); // level
  writer.write_uint(base); // base video format
  writer.write_bit(false); // custom dimensions
  writer.write_bit(false); // chroma
  writer.write_bit(false); // scan
  writer.write_bit(false); // frame rate
  writer.write_bit(false); // aspect ratio
  writer.write_bit(false); // clean area
  bytes.extend(writer.into_bytes());
  bytes.extend_from_slice(&PARSE_INFO_MAGIC);
  bytes
}

/// Build a Dirac sequence header with custom source dimensions plus a custom
/// frame-rate index and aspect-ratio index.
#[cfg(test)]
pub(crate) fn build_dirac_custom(width: u32, height: u32, frame_rate_index: u32, aspect_ratio_index: u32) -> Vec<u8> {
  let mut bytes = PARSE_INFO_MAGIC.to_vec();
  bytes.push(0x00);
  bytes.extend_from_slice(&[0u8; 8]);
  let mut writer = DiracUintWriter::new();
  writer.write_uint(2); // major
  writer.write_uint(2); // minor
  writer.write_uint(0); // profile
  writer.write_uint(0); // level
  writer.write_uint(0); // base video format
  writer.write_bit(true); // custom dimensions
  writer.write_uint(width);
  writer.write_uint(height);
  writer.write_bit(false); // chroma
  writer.write_bit(false); // scan
  writer.write_bit(true); // frame rate flag
  writer.write_uint(frame_rate_index);
  writer.write_bit(true); // aspect ratio flag
  writer.write_uint(aspect_ratio_index);
  writer.write_bit(false); // clean area
  bytes.extend(writer.into_bytes());
  bytes.extend_from_slice(&PARSE_INFO_MAGIC);
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

  #[test]
  fn probe_rejects_unflushed_sequence_header_at_eof() {
    let mut bytes = build_dirac_stream();
    bytes.truncate(bytes.len() - PARSE_INFO_MAGIC.len());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!DiracReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-242: full standard-format table + overrides ---------------

  #[test]
  fn standard_format_table_resolves_all_documented_indices() {
    // Indices that previously fell back to 640x480.
    let cases = [
      (3u32, 352u32, 240u32),
      (4, 352, 288),
      (5, 704, 480),
      (6, 704, 576),
      (15, 2048, 1080),
      (16, 4096, 2160),
      (17, 3840, 2160),
      (19, 7680, 4320),
      (22, 720, 486),
    ];
    for (index, w, h) in cases {
      let seq = parse_sequence_header(&build_dirac_base_format(index)).unwrap();
      assert_eq!((seq.pixel_width, seq.pixel_height), (w, h), "base_video_format {index}");
    }
  }

  #[test]
  fn out_of_range_base_format_falls_back_to_zero() {
    let seq = parse_sequence_header(&build_dirac_base_format(99)).unwrap();
    assert_eq!((seq.pixel_width, seq.pixel_height), (640, 480));
  }

  #[test]
  fn base_format_carries_frame_rate_and_aspect_ratio() {
    // Format 7 (720x480 interlaced) → 30000/1001 fps, 10/11 PAR.
    let seq = parse_sequence_header(&build_dirac_base_format(7)).unwrap();
    assert_eq!(seq.frame_rate_numerator, 30000);
    assert_eq!(seq.frame_rate_denominator, 1001);
    assert_eq!((seq.aspect_ratio_numerator, seq.aspect_ratio_denominator), (10, 11));
    // 720x480 with 10:11 PAR: num(10) < den(11) → height scaled up.
    let (dw, dh) = seq.display_dimensions();
    assert_eq!(dw, 720);
    assert_eq!(dh, ((480u64 * 11 + 5) / 10) as u32); // 528
  }

  #[test]
  fn custom_frame_rate_and_aspect_drive_display_and_duration() {
    // Custom 720x576, frame-rate index 4 (30000/1001), aspect index 3 (12/11).
    let bytes = build_dirac_custom(720, 576, 4, 3);
    let seq = parse_sequence_header(&bytes).unwrap();
    assert_eq!((seq.pixel_width, seq.pixel_height), (720, 576));
    assert_eq!((seq.frame_rate_numerator, seq.frame_rate_denominator), (30000, 1001));
    assert_eq!((seq.aspect_ratio_numerator, seq.aspect_ratio_denominator), (12, 11));
    // PAR 12:11, num > den → width scaled up: round(720 * 12 / 11) = 785.
    let (dw, dh) = seq.display_dimensions();
    assert_eq!((dw, dh), (785, 576));
    // default duration = 1e9 * 1001 / 30000 = 33_366_666 ns.
    assert_eq!(seq.default_duration_ns(), Some(33_366_666));

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.drc", 0);
    DiracReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 720,
        height: 576
      })
    );
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 785,
        height: 576
      })
    );
    assert_eq!(v.default_duration_ns, Some(33_366_666));
  }
}
