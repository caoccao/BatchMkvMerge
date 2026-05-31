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

//! DV (Digital Video) elementary stream reader.
//!
//! Each DV frame begins with a DIF block whose first byte distinguishes the
//! block type.  The header DIF block (`0x1F 0x07 0x00`) carries the DV-50
//! / DV-25 system info at byte 3:
//!
//! - bit 7: `dsf` (NTSC vs PAL).
//! - bits 6..0: reserved + tracks-per-frame.
//!
//! Identification only needs to flag the stream as DV.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::reader::Reader;

#[cfg(test)]
const HEADER_BLOCK_PREFIX: [u8; 3] = [0x1F, 0x07, 0x00];
/// Upstream caps the probe at 20 MiB (`../mkvtoolnix/src/input/r_dv.cpp:27`).
const DV_PROBE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct DvReader;

impl Reader for DvReader {
  fn name(&self) -> &'static str {
    "dv"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let head = read_probe_buffer(src)?;
    Ok(looks_like_dv(&head))
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let head = read_probe_buffer(src)?;
    if !looks_like_dv(&head) {
      return Err(ParseError::Unrecognised);
    }
    out.container.format = ContainerFormat::Dv;
    out.container.recognized = true;
    out.container.supported = false;
    Ok(())
  }
}

fn read_probe_buffer(src: &mut FileSource) -> Result<Vec<u8>, ParseError> {
  let size = src.length().unwrap_or(DV_PROBE_BYTES).min(DV_PROBE_BYTES) as usize;
  let mut head = vec![0u8; size];
  let read = src.read_at_most(&mut head)?;
  head.truncate(read);
  src.seek_to(0)?;
  Ok(head)
}

/// Port of `dv_reader_c::probe_file()` (`../mkvtoolnix/src/input/r_dv.cpp:25-66`).
///
/// A sliding 32-bit big-endian window walks the probe buffer counting primary
/// DIF-section markers (`0x1f07003f` with the channel/sequence bits masked
/// off) and secondary markers, plus the `0xff3f0701` audio-section marker that
/// trails a `0x..3f0700` marker by exactly 80 bytes.  DV is reported only when
/// the marker density crosses the upstream thresholds — a bare three-byte
/// prefix is no longer sufficient, and a valid stream whose first header does
/// not sit at offset 0 is still recognised.
fn looks_like_dv(bytes: &[u8]) -> bool {
  if bytes.len() < 4 {
    return false;
  }
  let probe_size = bytes.len() as u64;
  let mut state = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
  let mut matches: u64 = 0;
  let mut secondary_matches: u64 = 0;
  let mut marker_pos: u64 = 0;
  for (i, &byte) in bytes.iter().enumerate().skip(4) {
    let i = i as u64;
    if (state & 0xffff_ff7f) == 0x1f07_003f {
      matches += 1;
    }
    // Any section header — including non-zero seq/chan numbers — should
    // recur roughly every 12000 bytes, at least 10 per frame.
    if (state & 0xff07_ff7f) == 0x1f07_003f {
      secondary_matches += 1;
    }
    if state == 0x003f_0700 || state == 0xff3f_0700 {
      marker_pos = i;
    }
    if state == 0xff3f_0701 && (i - marker_pos) == 80 {
      matches += 1;
    }
    state = (state << 8) | u32::from(byte);
  }

  matches > 0
    && (probe_size / matches) < (1024 * 1024)
    && (matches > 4 || (secondary_matches >= 10 && (probe_size / secondary_matches) < 24000))
}

/// One 80-byte DIF block whose section header is `0x1f 0x07 0x00 <byte4>`.
/// The primary marker requires `(byte4 & 0x7f) == 0x3f`; bit 7 carries `dsf`
/// (0 → NTSC, 1 → PAL).
#[cfg(test)]
fn build_dv_section(byte4: u8) -> Vec<u8> {
  let mut block = vec![
    HEADER_BLOCK_PREFIX[0],
    HEADER_BLOCK_PREFIX[1],
    HEADER_BLOCK_PREFIX[2],
    byte4,
  ];
  block.extend_from_slice(&[0u8; 76]);
  block
}

#[cfg(test)]
pub(crate) fn build_dv_frame_ntsc() -> Vec<u8> {
  // Eight DIF section headers give the marker density mkvtoolnix's probe
  // requires (matches > 4).  byte4 = 0x3f → dsf = 0 → NTSC.
  let mut bytes = Vec::new();
  for _ in 0..8 {
    bytes.extend(build_dv_section(0x3f));
  }
  bytes
}

#[cfg(test)]
pub(crate) fn build_dv_frame_pal() -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..8 {
    bytes.extend(build_dv_section(0xbf)); // byte4 = 0xbf → dsf = 1 → PAL
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn probe_accepts_header_dif_prefix() {
    let bytes = build_dv_frame_ntsc();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(DvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_wrong_prefix() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 64]));
    assert!(!DvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_prefix_only() {
    // PARSER-221: a short file that merely starts with the 0x1f 0x07 0x00
    // prefix no longer passes — the marker-density threshold is not met.
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0x1F, 0x07, 0x00, 0x3F, 0x00, 0x00]));
    assert!(!DvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_marker_dense_stream() {
    // PARSER-221: density scan reports DV from the recurring section markers.
    let bytes = build_dv_frame_ntsc();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(DvReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_marks_ntsc_dv_unsupported_without_tracks() {
    let bytes = build_dv_frame_ntsc();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.dv", 0);
    DvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Dv);
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_marks_pal_dv_unsupported_without_tracks() {
    let bytes = build_dv_frame_pal();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.dv", 0);
    DvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_rejects_garbage() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 16]));
    let mut out = MediaMetadata::new("clip.dv", 0);
    let err = DvReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
