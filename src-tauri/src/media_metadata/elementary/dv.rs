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

const PROBE_BYTES: usize = 256;
const HEADER_BLOCK_PREFIX: [u8; 3] = [0x1F, 0x07, 0x00];

#[derive(Debug, Default, Clone, Copy)]
pub struct DvReader;

impl Reader for DvReader {
  fn name(&self) -> &'static str {
    "dv"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 4 {
      return Ok(false);
    }
    Ok(head[..3] == HEADER_BLOCK_PREFIX)
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
    if read < 4 || head[..3] != HEADER_BLOCK_PREFIX {
      return Err(ParseError::Unrecognised);
    }
    out.container.format = ContainerFormat::Dv;
    out.container.recognized = true;
    out.container.supported = false;
    Ok(())
  }
}

#[cfg(test)]
pub(crate) fn build_dv_frame_ntsc() -> Vec<u8> {
  let mut bytes = HEADER_BLOCK_PREFIX.to_vec();
  bytes.push(0x00); // dsf = 0 → NTSC
  bytes.extend_from_slice(&[0u8; 76]); // remainder of header DIF block
  bytes
}

#[cfg(test)]
pub(crate) fn build_dv_frame_pal() -> Vec<u8> {
  let mut bytes = HEADER_BLOCK_PREFIX.to_vec();
  bytes.push(0x80); // dsf = 1 → PAL
  bytes.extend_from_slice(&[0u8; 76]);
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
