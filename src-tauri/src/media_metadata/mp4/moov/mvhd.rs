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

//! `mvhd` (movie header) box.  Per ISO/IEC 14496-12 §8.2.2:
//!
//! v0 layout: 1B version + 3B flags + 4B creation_time + 4B modification_time
//!            + 4B timescale + 4B duration + 4B rate + 2B volume + 10B reserved
//!            + 36B matrix + 24B predefined + 4B next_track_id  =  100 bytes
//! v1 layout: same but with 8-byte creation/modification/duration  =  112 bytes
//!
//! We only care about `timescale`, `duration` (units), and `next_track_id`.
//! Everything else is skipped.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone, Copy)]
pub struct MovieHeader {
  pub version: u8,
  pub timescale: u32,
  pub duration: u64,
  pub next_track_id: u32,
  /// 3×3 display matrix (16.16 fixed-point columns 0/1, 2.30 column 2).
  /// PARSER-147 — combined with each track's matrix to derive orientation.
  pub matrix: [[i32; 3]; 3],
}

/// The ISO BMFF identity display matrix (`{1,0,0, 0,1,0, 0,0,1}` in the mixed
/// 16.16 / 2.30 fixed-point form).  Used as the neutral element when a movie
/// or track omits its matrix.
pub const IDENTITY_MATRIX: [[i32; 3]; 3] = [
  [0x0001_0000, 0, 0],
  [0, 0x0001_0000, 0],
  [0, 0, 0x4000_0000],
];

/// Read a 3×3 display matrix (9 × big-endian i32) from the current cursor.
pub fn read_matrix(src: &mut FileSource) -> Result<[[i32; 3]; 3], ParseError> {
  let mut m = [[0i32; 3]; 3];
  for row in m.iter_mut() {
    for cell in row.iter_mut() {
      *cell = src.read_u32_be()? as i32;
    }
  }
  Ok(m)
}

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<MovieHeader, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 100 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("mvhd payload {payload} bytes is too small"),
    });
  }
  let version = src.read_u8()?;
  let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];

  let (timescale, duration) = match version {
    0 => {
      // 4B creation + 4B modification = 8 bytes
      src.skip(8)?;
      let ts = src.read_u32_be()?;
      let dur = src.read_u32_be()? as u64;
      (ts, dur)
    }
    _ => {
      // 8B creation + 8B modification = 16 bytes
      src.skip(16)?;
      let ts = src.read_u32_be()?;
      let dur = src.read_u64_be()?;
      (ts, dur)
    }
  };
  // Skip rate (4) + volume (2) + reserved (10) = 16 bytes, then read the
  // 36-byte display matrix (PARSER-147), skip pre_defined (24), read
  // next_track_id (4).
  src.skip(4 + 2 + 10)?;
  let matrix = read_matrix(src)?;
  src.skip(24)?;
  let next_track_id = src.read_u32_be()?;
  Ok(MovieHeader {
    version,
    timescale,
    duration,
    next_track_id,
    matrix,
  })
}

#[cfg(test)]
pub(crate) fn build_mvhd_payload_v0(timescale: u32, duration: u32, next_track_id: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(100);
  p.push(0); // version
  p.extend_from_slice(&[0u8; 3]); // flags
  p.extend_from_slice(&0u32.to_be_bytes()); // creation
  p.extend_from_slice(&0u32.to_be_bytes()); // modification
  p.extend_from_slice(&timescale.to_be_bytes());
  p.extend_from_slice(&duration.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // rate
  p.extend_from_slice(&[0u8; 2]); // volume
  p.extend_from_slice(&[0u8; 10]); // reserved
  p.extend_from_slice(&[0u8; 36]); // matrix
  p.extend_from_slice(&[0u8; 24]); // pre_defined
  p.extend_from_slice(&next_track_id.to_be_bytes());
  p
}

#[cfg(test)]
pub(crate) fn build_mvhd_payload_v1(timescale: u32, duration: u64, next_track_id: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(112);
  p.push(1); // version
  p.extend_from_slice(&[0u8; 3]); // flags
  p.extend_from_slice(&0u64.to_be_bytes()); // creation
  p.extend_from_slice(&0u64.to_be_bytes()); // modification
  p.extend_from_slice(&timescale.to_be_bytes());
  p.extend_from_slice(&duration.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // rate
  p.extend_from_slice(&[0u8; 2]); // volume
  p.extend_from_slice(&[0u8; 10]); // reserved
  p.extend_from_slice(&[0u8; 36]); // matrix
  p.extend_from_slice(&[0u8; 24]); // pre_defined
  p.extend_from_slice(&next_track_id.to_be_bytes());
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::{self, encode_box};
  use std::io::Cursor;

  fn read(bytes: Vec<u8>) -> (BoxHeader, FileSource) {
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    (h, s)
  }

  #[test]
  fn parses_v0_payload() {
    let payload = build_mvhd_payload_v0(1000, 60_000, 5);
    let bytes = encode_box(b"mvhd", &payload);
    let (h, mut s) = read(bytes);
    let m = parse(&mut s, &h).unwrap();
    assert_eq!(m.version, 0);
    assert_eq!(m.timescale, 1000);
    assert_eq!(m.duration, 60_000);
    assert_eq!(m.next_track_id, 5);
  }

  #[test]
  fn parses_v1_payload_with_64_bit_duration() {
    let payload = build_mvhd_payload_v1(48000, 1u64 << 40, 9);
    let bytes = encode_box(b"mvhd", &payload);
    let (h, mut s) = read(bytes);
    let m = parse(&mut s, &h).unwrap();
    assert_eq!(m.version, 1);
    assert_eq!(m.timescale, 48000);
    assert_eq!(m.duration, 1u64 << 40);
    assert_eq!(m.next_track_id, 9);
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"mvhd", &[0u8; 16]);
    let (h, mut s) = read(bytes);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
