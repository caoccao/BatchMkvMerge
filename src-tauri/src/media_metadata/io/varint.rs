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

//! EBML variable-length integer (VINT) decoder. The encoding is defined by
//! the Matroska/EBML spec — the leading byte has a marker indicating how
//! many bytes total, and the value is the remaining bits taken big-endian.
//!
//! References:
//!   - Matroska EBML spec §7.2 (VINT)
//!   - mkvtoolnix `src/common/ebml.h:14-60`

use super::super::error::ParseError;
use super::file_source::FileSource;

/// Maximum encoded width in bytes per the EBML spec.
pub const MAX_VINT_WIDTH: usize = 8;

/// A decoded VINT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vint {
  /// Number of bytes consumed from the stream (1..=8).
  pub width: u8,
  /// Decoded integer value (without the leading marker bit).
  pub value: u64,
  /// `IdMarker` when the marker bit is kept (as in EBML *Element IDs*);
  /// `Stripped` when the marker bit is removed (as in EBML *element sizes*).
  pub kind: VintKind,
}

/// Distinguishes the two flavours of VINT decoding the EBML spec uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VintKind {
  /// Element ID — the marker bit *stays* in the value, so e.g. EBML head
  /// `0x1A45DFA3` decodes to `0x1A45DFA3`, not `0x0A45DFA3`.
  IdMarker,
  /// Element size — the marker bit is stripped so a 1-byte size of
  /// `0x82` decodes to `0x02`. The all-ones "unknown size" form is
  /// reported by `is_unknown_size`.
  Stripped,
}

impl Vint {
  /// True for sizes encoded as all 1s after the marker. Matroska treats
  /// these as "the cluster runs to the next L1 element" / EOF.
  pub fn is_unknown_size(&self) -> bool {
    let payload_bits = (7 * self.width) as u32;
    if payload_bits >= 64 {
      self.value == u64::MAX
    } else {
      self.value == (1u64 << payload_bits) - 1
    }
  }
}

/// Decode a VINT from a `FileSource`. Advances the cursor by `width` bytes.
pub fn read(src: &mut FileSource, kind: VintKind) -> Result<Vint, ParseError> {
  let start = src.position();
  let first = src.read_u8()?;
  decode_with_first_byte(first, kind, src, start)
}

/// Pure-byte-slice decoder. Returns the VINT plus how many bytes were
/// consumed; useful inside synthetic-byte unit tests and when peeking from a
/// buffered slice without bumping a `FileSource`.
pub fn decode(bytes: &[u8], kind: VintKind) -> Result<(Vint, usize), ParseError> {
  if bytes.is_empty() {
    return Err(ParseError::UnexpectedEof { offset: 0, wanted: 1 });
  }
  let first = bytes[0];
  let width = vint_width_from_first_byte(first).ok_or_else(|| ParseError::Malformed {
    format: "ebml",
    offset: 0,
    reason: "VINT leading byte has no marker bit set".to_string(),
  })?;
  if bytes.len() < width as usize {
    return Err(ParseError::UnexpectedEof {
      offset: 0,
      wanted: (width as u64) - (bytes.len() as u64),
    });
  }

  let value = compose_value(bytes, width, kind);
  Ok((Vint { width, value, kind }, width as usize))
}

fn decode_with_first_byte(first: u8, kind: VintKind, src: &mut FileSource, start: u64) -> Result<Vint, ParseError> {
  let width = vint_width_from_first_byte(first).ok_or_else(|| ParseError::Malformed {
    format: "ebml",
    offset: start,
    reason: "VINT leading byte has no marker bit set".to_string(),
  })?;
  let mut bytes = [0u8; MAX_VINT_WIDTH];
  bytes[0] = first;
  if width > 1 {
    src.read_exact(&mut bytes[1..width as usize])?;
  }
  let value = compose_value(&bytes[..width as usize], width, kind);
  Ok(Vint { width, value, kind })
}

fn compose_value(bytes: &[u8], width: u8, kind: VintKind) -> u64 {
  let mut v: u64 = match kind {
    VintKind::IdMarker => bytes[0] as u64,
    VintKind::Stripped => (bytes[0] as u64) & marker_mask_for_width(width),
  };
  for &b in &bytes[1..(width as usize)] {
    v = (v << 8) | (b as u64);
  }
  v
}

fn marker_mask_for_width(width: u8) -> u64 {
  // The marker bit sits at position (8 - width) in the first byte;
  // every lower bit is data.
  debug_assert!((1..=MAX_VINT_WIDTH as u8).contains(&width));
  (1u64 << (8 - width)) - 1
}

/// Return the encoded byte width (1..=8) based on the first byte's leading
/// 1-bit. `None` if the byte is all zeros (an illegal VINT).
pub fn vint_width_from_first_byte(first: u8) -> Option<u8> {
  if first == 0 {
    None
  } else {
    Some(1 + (first.leading_zeros() as u8))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::io::file_source::FileSource;
  use std::io::Cursor;

  fn src(bytes: &[u8]) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes.to_vec()))
  }

  #[test]
  fn one_byte_id_marker_keeps_marker() {
    // 0x82 = 10000010 -> width 1, IdMarker value = 0x82
    let (v, n) = decode(&[0x82], VintKind::IdMarker).unwrap();
    assert_eq!(n, 1);
    assert_eq!(v.width, 1);
    assert_eq!(v.value, 0x82);
    assert!(!v.is_unknown_size());
  }

  #[test]
  fn one_byte_stripped_drops_marker() {
    // 0x82 = 10000010 -> width 1, stripped value = 0x02
    let (v, n) = decode(&[0x82], VintKind::Stripped).unwrap();
    assert_eq!(n, 1);
    assert_eq!(v.width, 1);
    assert_eq!(v.value, 0x02);
  }

  #[test]
  fn ebml_head_id_decodes_as_expected() {
    // EBMLHead = 0x1A 45 DF A3, width = 4, IdMarker keeps marker bit.
    let bytes = [0x1A, 0x45, 0xDF, 0xA3];
    let (v, n) = decode(&bytes, VintKind::IdMarker).unwrap();
    assert_eq!(n, 4);
    assert_eq!(v.width, 4);
    assert_eq!(v.value, 0x1A45_DFA3);
  }

  #[test]
  fn stripped_unknown_size_one_byte() {
    // 0xFF stripped = 0x7F = unknown size for width 1.
    let (v, _) = decode(&[0xFF], VintKind::Stripped).unwrap();
    assert!(v.is_unknown_size());
  }

  #[test]
  fn stripped_unknown_size_eight_bytes() {
    // 01 FF FF FF FF FF FF FF -> width 8, all data bits set -> unknown
    let bytes = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    let (v, n) = decode(&bytes, VintKind::Stripped).unwrap();
    assert_eq!(n, 8);
    assert_eq!(v.width, 8);
    assert!(v.is_unknown_size());
  }

  #[test]
  fn stripped_value_not_unknown_when_one_bit_clear() {
    let bytes = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE];
    let (v, _) = decode(&bytes, VintKind::Stripped).unwrap();
    assert!(!v.is_unknown_size());
  }

  #[test]
  fn first_byte_zero_is_malformed() {
    let err = decode(&[0x00], VintKind::Stripped).unwrap_err();
    match err {
      ParseError::Malformed { format, reason, .. } => {
        assert_eq!(format, "ebml");
        assert!(reason.contains("marker"));
      }
      other => panic!("expected Malformed, got {other:?}"),
    }
  }

  #[test]
  fn empty_input_is_unexpected_eof() {
    let err = decode(&[], VintKind::Stripped).unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn short_buffer_for_declared_width_is_eof() {
    // 0x20 -> width = 3 (leading_zeros = 2, +1)
    let err = decode(&[0x20], VintKind::Stripped).unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn width_helper_matches_marker_position() {
    assert_eq!(vint_width_from_first_byte(0x80), Some(1));
    assert_eq!(vint_width_from_first_byte(0x40), Some(2));
    assert_eq!(vint_width_from_first_byte(0x20), Some(3));
    assert_eq!(vint_width_from_first_byte(0x10), Some(4));
    assert_eq!(vint_width_from_first_byte(0x08), Some(5));
    assert_eq!(vint_width_from_first_byte(0x04), Some(6));
    assert_eq!(vint_width_from_first_byte(0x02), Some(7));
    assert_eq!(vint_width_from_first_byte(0x01), Some(8));
    assert_eq!(vint_width_from_first_byte(0x00), None);
  }

  #[test]
  fn file_source_read_advances_cursor() {
    let mut s = src(&[0x82, 0x40, 0xAB]);
    let v = read(&mut s, VintKind::Stripped).unwrap();
    assert_eq!(v.width, 1);
    assert_eq!(v.value, 0x02);
    assert_eq!(s.position(), 1);
    // and the next VINT can be read continuing forward
    let v2 = read(&mut s, VintKind::IdMarker).unwrap();
    assert_eq!(v2.width, 2);
    assert_eq!(v2.value, 0x40AB);
    assert_eq!(s.position(), 3);
  }

  #[test]
  fn file_source_short_read_after_first_byte_is_eof() {
    let mut s = src(&[0x40]); // declares width 2, but only 1 byte available
    let err = read(&mut s, VintKind::Stripped).unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn id_marker_preserves_bytes_in_4_byte_id() {
    // Cluster = 0x1F 43 B6 75
    let (v, n) = decode(&[0x1F, 0x43, 0xB6, 0x75], VintKind::IdMarker).unwrap();
    assert_eq!(n, 4);
    assert_eq!(v.value, 0x1F43_B675);
  }

  #[test]
  fn five_byte_stripped_value_decoded() {
    // 0x08 leads -> width 5
    // marker mask is (1 << (8-5))-1 = 7 -> first byte's lower 3 bits stay,
    // followed by 4 more bytes.
    let bytes = [0x08, 0x01, 0x02, 0x03, 0x04];
    let (v, n) = decode(&bytes, VintKind::Stripped).unwrap();
    assert_eq!(n, 5);
    assert_eq!(v.width, 5);
    assert_eq!(v.value, (0u64 << 32) | 0x01_02_03_04);
  }

  #[test]
  fn eight_byte_id_marker_keeps_leading_bit() {
    // 0x01 leads -> width 8
    let bytes = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let (v, n) = decode(&bytes, VintKind::IdMarker).unwrap();
    assert_eq!(n, 8);
    assert_eq!(v.width, 8);
    // IdMarker preserves the entire byte (0x01) at the top.
    assert_eq!(v.value, 0x0102_0304_0506_0708);
  }
}
