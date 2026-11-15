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

//! `tkhd` (track header) box.  Per ISO/IEC 14496-12 §8.3.2.
//!
//! We extract:
//! - `track_id` — feeds `CommonTrackProperties.number`.
//! - `width` / `height` — 16.16 fixed-point, only meaningful for video tracks
//!   (display dimensions; the encoded raster lives on the sample description).

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone, Copy)]
pub struct TrackHeader {
  pub version: u8,
  pub track_id: u32,
  /// 16.16 fixed-point display width.  Always 0 for non-video tracks.
  pub width_fixed: u32,
  /// 16.16 fixed-point display height.
  pub height_fixed: u32,
  pub enabled: bool,
  /// Decoded display rotation derived from the 3×3 affine matrix (in
  /// degrees, modulo 360).  `None` when the matrix is the identity or
  /// otherwise unrecognised.  PARSER-069.
  pub rotation_degrees: Option<u32>,
  /// `true` when the matrix flips one axis (horizontal or vertical
  /// reflection).  Derived from the determinant of the 2×2 block.
  pub flipped: bool,
  /// The raw 3×3 track display matrix (16.16 / 2.30 fixed-point).  Combined
  /// with the movie matrix to compute yaw/roll (PARSER-147).
  pub matrix: [[i32; 3]; 3],
}

const FLAG_TRACK_ENABLED: u32 = 0x000001;

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<TrackHeader, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 84 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("tkhd payload {payload} bytes is too small"),
    });
  }
  let version = src.read_u8()?;
  let flags_bytes = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
  let flags = ((flags_bytes[0] as u32) << 16) | ((flags_bytes[1] as u32) << 8) | flags_bytes[2] as u32;
  let track_id = match version {
    0 => {
      src.skip(4 + 4)?; // creation + modification (4+4)
      let id = src.read_u32_be()?;
      src.skip(4)?; // reserved
      src.skip(4)?; // duration
      id
    }
    _ => {
      src.skip(8 + 8)?; // creation + modification (8+8)
      let id = src.read_u32_be()?;
      src.skip(4)?; // reserved
      src.skip(8)?; // duration
      id
    }
  };
  // 2x4 reserved + 2 layer + 2 alt_group + 2 volume + 2 reserved
  src.skip(8 + 2 + 2 + 2 + 2)?;
  // PARSER-069: read the 36-byte display matrix instead of skipping it.
  let mut matrix = [0u8; 36];
  src.read_exact(&mut matrix)?;
  let width_fixed = src.read_u32_be()?;
  let height_fixed = src.read_u32_be()?;
  let (rotation_degrees, flipped) = decode_matrix(&matrix);
  let cells = matrix_cells(&matrix);
  Ok(TrackHeader {
    version,
    track_id,
    width_fixed,
    height_fixed,
    enabled: flags & FLAG_TRACK_ENABLED != 0,
    rotation_degrees,
    flipped,
    matrix: cells,
  })
}

/// Convert a 16.16 fixed-point value to a `u32` pixel count by rounding to
/// the nearest integer.  PARSER-070: mkvtoolnix reads tkhd width/height as
/// floating-point and reports the nearest integer, so we round here instead
/// of dropping the fractional bits.
pub fn fixed_to_pixels(fixed: u32) -> u32 {
  // The high 16 bits are the integer part; the low 16 bits the fraction.
  // Add 0x8000 (= 0.5) before truncating to round-half-up.
  fixed.saturating_add(0x8000) >> 16
}

/// Unpack the 36-byte display matrix into nine big-endian `i32` cells.
fn matrix_cells(matrix: &[u8; 36]) -> [[i32; 3]; 3] {
  let mut m = [[0i32; 3]; 3];
  for (idx, cell) in m.iter_mut().flatten().enumerate() {
    let off = idx * 4;
    *cell = i32::from_be_bytes([matrix[off], matrix[off + 1], matrix[off + 2], matrix[off + 3]]);
  }
  m
}

/// Decode the 3×3 ISO BMFF display matrix into a rotation/flip pair.  The
/// matrix is a 9-tuple of 16.16 fixed-point values; only the 2×2 sub-block
/// (a, b, c, d) matters for rotation/reflection.  See
/// `mkvtoolnix/src/input/r_qtmp4.cpp:1628-1663`.  PARSER-069.
fn decode_matrix(matrix: &[u8; 36]) -> (Option<u32>, bool) {
  fn read(matrix: &[u8; 36], idx: usize) -> i32 {
    let off = idx * 4;
    i32::from_be_bytes([matrix[off], matrix[off + 1], matrix[off + 2], matrix[off + 3]])
  }
  fn to_f64(fixed: i32) -> f64 {
    fixed as f64 / 65536.0
  }
  let a = to_f64(read(matrix, 0));
  let b = to_f64(read(matrix, 1));
  let c = to_f64(read(matrix, 3));
  let d = to_f64(read(matrix, 4));
  let zero_check = |v: f64| v.abs() < 0.01;
  // Identity (no rotation, no flip).
  if zero_check(a - 1.0) && zero_check(b) && zero_check(c) && zero_check(d - 1.0) {
    return (None, false);
  }
  // Pure 90° rotation:  (0,1,-1,0)
  if zero_check(a) && zero_check(b - 1.0) && zero_check(c + 1.0) && zero_check(d) {
    return (Some(90), false);
  }
  // 180°: (-1,0,0,-1)
  if zero_check(a + 1.0) && zero_check(b) && zero_check(c) && zero_check(d + 1.0) {
    return (Some(180), false);
  }
  // 270°: (0,-1,1,0)
  if zero_check(a) && zero_check(b + 1.0) && zero_check(c - 1.0) && zero_check(d) {
    return (Some(270), false);
  }
  // Heuristic flip detection — negative determinant means a reflection.
  let det = a * d - b * c;
  (None, det < 0.0)
}

#[cfg(test)]
pub(crate) fn build_tkhd_payload_v0(track_id: u32, width_px: u16, height_px: u16) -> Vec<u8> {
  let mut p = Vec::with_capacity(84);
  p.push(0); // version
  p.extend_from_slice(&[0u8, 0u8, 0x01u8]); // flags = track_enabled
  p.extend_from_slice(&0u32.to_be_bytes()); // creation
  p.extend_from_slice(&0u32.to_be_bytes()); // modification
  p.extend_from_slice(&track_id.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // reserved
  p.extend_from_slice(&0u32.to_be_bytes()); // duration
  p.extend_from_slice(&[0u8; 8]); // 2x4 reserved
  p.extend_from_slice(&[0u8; 2]); // layer
  p.extend_from_slice(&[0u8; 2]); // alt_group
  p.extend_from_slice(&[0u8; 2]); // volume
  p.extend_from_slice(&[0u8; 2]); // reserved
  p.extend_from_slice(&[0u8; 36]); // matrix
  let width = (width_px as u32) << 16;
  let height = (height_px as u32) << 16;
  p.extend_from_slice(&width.to_be_bytes());
  p.extend_from_slice(&height.to_be_bytes());
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
  fn parses_v0_tkhd() {
    let payload = build_tkhd_payload_v0(2, 1920, 1080);
    let bytes = encode_box(b"tkhd", &payload);
    let (h, mut s) = read(bytes);
    let t = parse(&mut s, &h).unwrap();
    assert_eq!(t.track_id, 2);
    assert_eq!(fixed_to_pixels(t.width_fixed), 1920);
    assert_eq!(fixed_to_pixels(t.height_fixed), 1080);
    assert!(t.enabled);
  }

  #[test]
  fn parses_v1_tkhd() {
    let mut p = Vec::new();
    p.push(1); // version
    p.extend_from_slice(&[0u8, 0u8, 0x01u8]); // flags
    p.extend_from_slice(&[0u8; 8]); // creation
    p.extend_from_slice(&[0u8; 8]); // modification
    p.extend_from_slice(&7u32.to_be_bytes()); // track_id
    p.extend_from_slice(&[0u8; 4]); // reserved
    p.extend_from_slice(&[0u8; 8]); // duration
    p.extend_from_slice(&[0u8; 8 + 2 + 2 + 2 + 2 + 36]);
    p.extend_from_slice(&((1280u32) << 16).to_be_bytes());
    p.extend_from_slice(&((720u32) << 16).to_be_bytes());
    let bytes = encode_box(b"tkhd", &p);
    let (h, mut s) = read(bytes);
    let t = parse(&mut s, &h).unwrap();
    assert_eq!(t.version, 1);
    assert_eq!(t.track_id, 7);
    assert_eq!(fixed_to_pixels(t.width_fixed), 1280);
    assert_eq!(fixed_to_pixels(t.height_fixed), 720);
  }

  #[test]
  fn flag_enabled_decoded() {
    let mut p = build_tkhd_payload_v0(1, 0, 0);
    // Zero out the flag byte (offset 3)
    p[3] = 0;
    let bytes = encode_box(b"tkhd", &p);
    let (h, mut s) = read(bytes);
    let t = parse(&mut s, &h).unwrap();
    assert!(!t.enabled);
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"tkhd", &[0u8; 16]);
    let (h, mut s) = read(bytes);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  // ---- PARSER-069: tkhd matrix decode ---------------------------------

  fn matrix_with(a: f64, b: f64, c: f64, d: f64) -> [u8; 36] {
    let mut m = [0u8; 36];
    let cells: [f64; 9] = [a, b, 0.0, c, d, 0.0, 0.0, 0.0, 1.0];
    for (i, v) in cells.iter().enumerate() {
      let fixed = (v * 65536.0) as i32;
      m[i * 4..i * 4 + 4].copy_from_slice(&fixed.to_be_bytes());
    }
    m
  }

  #[test]
  fn matrix_decodes_identity() {
    let (rot, flip) = decode_matrix(&matrix_with(1.0, 0.0, 0.0, 1.0));
    assert!(rot.is_none());
    assert!(!flip);
  }

  #[test]
  fn matrix_decodes_90_degree_rotation() {
    let (rot, flip) = decode_matrix(&matrix_with(0.0, 1.0, -1.0, 0.0));
    assert_eq!(rot, Some(90));
    assert!(!flip);
  }

  #[test]
  fn matrix_decodes_180_degree_rotation() {
    let (rot, _) = decode_matrix(&matrix_with(-1.0, 0.0, 0.0, -1.0));
    assert_eq!(rot, Some(180));
  }

  #[test]
  fn matrix_decodes_270_degree_rotation() {
    let (rot, _) = decode_matrix(&matrix_with(0.0, -1.0, 1.0, 0.0));
    assert_eq!(rot, Some(270));
  }

  #[test]
  fn matrix_decodes_horizontal_flip() {
    let (_, flip) = decode_matrix(&matrix_with(-1.0, 0.0, 0.0, 1.0));
    assert!(flip);
  }

  #[test]
  fn fixed_to_pixels_rounds_half_up() {
    // PARSER-070: 16.16 → integer should round-half-up, not truncate.
    assert_eq!(fixed_to_pixels(0x07800000), 1920);
    // 0.5 rounds to 1.
    assert_eq!(fixed_to_pixels(0x00008000), 1);
    // 0.49 truncates to 0.
    assert_eq!(fixed_to_pixels(0x00007FFF), 0);
  }
}
