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

//! Generic EBML element walker.
//!
//! The walker is iterator-based — callers maintain their own container stack
//! rather than letting recursion grow with user-controlled depth (mitigates
//! malicious nesting). Each call decodes one element header — id + size —
//! and the caller chooses whether to descend, skip, or accumulate the payload.
//!
//! Source-of-truth file: `mkvtoolnix/src/common/ebml.cpp` (the libebml glue)
//! plus the EBML RFC at <https://datatracker.ietf.org/doc/html/rfc8794>.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::io::varint::{self, Vint, VintKind};

use super::ids;

/// One EBML element header.  Captures everything a parser needs to decide
/// whether to descend into the payload, skip it, or accumulate it.
#[derive(Debug, Clone, Copy)]
pub struct ElementHeader {
  /// Absolute file offset of the first byte of the id VINT.
  pub start: u64,
  /// Decoded element id (preserves marker bit).
  pub id: u32,
  /// Decoded payload size in bytes; `None` for the EBML "unknown size"
  /// sentinel (used by live-streamed clusters / segments).
  pub size: Option<u64>,
  /// Bytes the id+size header occupied — `payload_start = start + header_len`.
  pub header_len: u8,
}

impl ElementHeader {
  /// Absolute file offset of the first payload byte.
  pub fn payload_start(&self) -> u64 {
    self.start + self.header_len as u64
  }

  /// Absolute file offset one byte past the last payload byte.  `None` when
  /// payload size is unknown (only meaningful for live cluster streams).
  pub fn end(&self) -> Option<u64> {
    self.size.map(|s| self.payload_start() + s)
  }
}

/// Read one EBML element header at the current cursor.  Advances the cursor
/// past the header so subsequent reads land on the first payload byte.
/// Returns `Err(ParseError::UnexpectedEof)` if either VINT cannot fit.
pub fn read_element_header(src: &mut FileSource) -> Result<ElementHeader, ParseError> {
  let start = src.position();
  let id_vint = varint::read(src, VintKind::IdMarker)?;
  let id = vint_to_u32_id(&id_vint, start)?;
  let size_vint = varint::read(src, VintKind::Stripped)?;
  let size = if size_vint.is_unknown_size() {
    None
  } else {
    Some(size_vint.value)
  };
  let header_len = id_vint.width + size_vint.width;
  Ok(ElementHeader {
    start,
    id,
    size,
    header_len,
  })
}

/// Try to read one EBML element header without consuming it. Useful when the
/// caller wants to peek before deciding whether to claim the element. The
/// cursor is restored to its starting position after the read.
pub fn peek_element_header(src: &mut FileSource) -> Result<ElementHeader, ParseError> {
  let pos = src.position();
  let header = read_element_header(src)?;
  src.seek_to(pos)?;
  Ok(header)
}

/// Skip past the element's payload — bringing the cursor to the position
/// after the last payload byte. No-op for unknown-size elements (caller
/// must use a smarter strategy, e.g. scan for the next L1 id).
pub fn skip_payload(src: &mut FileSource, header: &ElementHeader) -> Result<(), ParseError> {
  if let Some(end) = header.end() {
    src.seek_to(end)?;
  }
  Ok(())
}

/// Read an unsigned integer payload (1..=8 bytes, big-endian). Matroska
/// allows zero-byte uints — those evaluate to 0.
pub fn read_uint(src: &mut FileSource, header: &ElementHeader) -> Result<u64, ParseError> {
  let size = header.size.ok_or_else(|| ParseError::Malformed {
    format: "matroska",
    offset: header.start,
    reason: format!("uint element {:#x} has unknown size", header.id),
  })?;
  if size > 8 {
    return Err(ParseError::Malformed {
      format: "matroska",
      offset: header.start,
      reason: format!("uint element {:#x} too large ({} bytes)", header.id, size),
    });
  }
  let mut value: u64 = 0;
  for _ in 0..size {
    let b = src.read_u8()?;
    value = (value << 8) | b as u64;
  }
  Ok(value)
}

/// Read a signed integer payload (1..=8 bytes, two's-complement big-endian).
pub fn read_int(src: &mut FileSource, header: &ElementHeader) -> Result<i64, ParseError> {
  let size = header.size.ok_or_else(|| ParseError::Malformed {
    format: "matroska",
    offset: header.start,
    reason: format!("int element {:#x} has unknown size", header.id),
  })?;
  if size > 8 {
    return Err(ParseError::Malformed {
      format: "matroska",
      offset: header.start,
      reason: format!("int element {:#x} too large ({} bytes)", header.id, size),
    });
  }
  if size == 0 {
    return Ok(0);
  }
  let first = src.read_u8()?;
  // Sign-extend the first byte.
  let mut value: i64 = (first as i8) as i64;
  for _ in 1..size {
    let b = src.read_u8()?;
    value = (value << 8) | b as i64;
  }
  Ok(value)
}

/// Read an IEEE 754 float payload (0/4/8 bytes). 0 bytes evaluates to 0.0
/// per the spec.
pub fn read_float(src: &mut FileSource, header: &ElementHeader) -> Result<f64, ParseError> {
  let size = header.size.ok_or_else(|| ParseError::Malformed {
    format: "matroska",
    offset: header.start,
    reason: format!("float element {:#x} has unknown size", header.id),
  })?;
  match size {
    0 => Ok(0.0),
    4 => {
      let raw = src.read_u32_be()?;
      Ok(f32::from_bits(raw) as f64)
    }
    8 => {
      let raw = src.read_u64_be()?;
      Ok(f64::from_bits(raw))
    }
    _ => Err(ParseError::Malformed {
      format: "matroska",
      offset: header.start,
      reason: format!(
        "float element {:#x} has invalid size {} (expected 0, 4 or 8)",
        header.id, size
      ),
    }),
  }
}

/// Read a UTF-8 string payload. Strips trailing NUL bytes (Matroska allows
/// padding string payloads with NUL).
pub fn read_string(src: &mut FileSource, header: &ElementHeader, cap: u64) -> Result<String, ParseError> {
  let bytes = read_binary(src, header, cap)?;
  let trimmed_end = bytes.iter().rposition(|&b| b != 0).map(|p| p + 1).unwrap_or(0);
  let slice = &bytes[..trimmed_end];
  String::from_utf8(slice.to_vec()).map_err(|e| ParseError::Malformed {
    format: "matroska",
    offset: header.start,
    reason: format!("string element {:#x} is not valid UTF-8: {}", header.id, e),
  })
}

/// Read a binary payload up to `cap` bytes. Rejects oversized elements via
/// `ParseError::OversizedElement` so a corrupt size VINT cannot drive a
/// runaway allocation.
pub fn read_binary(src: &mut FileSource, header: &ElementHeader, cap: u64) -> Result<Vec<u8>, ParseError> {
  let size = header.size.ok_or_else(|| ParseError::Malformed {
    format: "matroska",
    offset: header.start,
    reason: format!("binary element {:#x} has unknown size", header.id),
  })?;
  if size > cap {
    return Err(ParseError::OversizedElement {
      format: "matroska",
      id: header.id as u64,
      size,
      cap,
      offset: header.start,
    });
  }
  let mut buf = vec![0u8; size as usize];
  src.read_exact(&mut buf)?;
  Ok(buf)
}

/// Cap-protected sibling walker. Iterates immediate children of a master
/// element by repeatedly calling `read_element_header` until either:
///
/// - the parent's payload boundary is reached, or
/// - an error surfaces.
///
/// The closure decides per-element whether to descend (in which case it
/// must consume the payload) or skip (in which case the walker advances
/// the cursor to the next sibling itself).
///
/// `parent` may carry an unknown size — in that case the walker reads until
/// it hits an L1 id (any id with width ≤ 4 and a known position in the
/// EBML class tree). The fallback is conservative: we stop on EOF only.
pub fn walk_children<F>(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline_stage: &'static str,
  deadline: &crate::media_metadata::deadline::Deadline,
  mut on_child: F,
) -> Result<(), ParseError>
where
  F: FnMut(&mut FileSource, &ElementHeader) -> Result<ChildAction, ParseError>,
{
  let payload_start = parent.payload_start();
  let payload_end = parent.end();

  if let Some(end) = payload_end {
    if payload_start > end {
      return Err(ParseError::Malformed {
        format: "matroska",
        offset: parent.start,
        reason: format!("element {:#x} payload_start > end", parent.id),
      });
    }
  }

  let stream_end = src.length();
  src.seek_to(payload_start)?;

  while let Some(remaining) = remaining_in_parent(src, payload_end, stream_end) {
    if remaining == 0 {
      break;
    }
    deadline.check(deadline_stage)?;
    // Peek to capture start position before reading; if the read fails
    // mid-header (truncated container) we want to surface that as
    // UnexpectedEof anchored at the right offset.
    let child = read_element_header(src)?;

    // Sanity-check the child does not extend past the parent.
    if let (Some(end), Some(child_end)) = (payload_end, child.end()) {
      if child_end > end {
        return Err(ParseError::Malformed {
          format: "matroska",
          offset: child.start,
          reason: format!(
            "child {:#x} extends past parent {:#x} ({} > {})",
            child.id, parent.id, child_end, end
          ),
        });
      }
    }

    let action = on_child(src, &child)?;
    match action {
      ChildAction::Consumed => {
        // Caller already advanced past the payload — trust them but
        // re-align in case they over- or under-read.
        if let Some(end) = child.end() {
          src.seek_to(end)?;
        }
      }
      ChildAction::Skip => {
        skip_payload(src, &child)?;
      }
    }
  }
  Ok(())
}

/// Tell the walker whether the closure already consumed the element's
/// payload or whether the walker should skip it on the caller's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildAction {
  /// The closure already advanced the cursor (typically because it called
  /// `walk_children` recursively on this element or read its payload).
  Consumed,
  /// The walker should skip past the payload.
  Skip,
}

fn remaining_in_parent(src: &FileSource, payload_end: Option<u64>, stream_end: Option<u64>) -> Option<u64> {
  let pos = src.position();
  let parent_remaining = payload_end.map(|end| end.saturating_sub(pos));
  let stream_remaining = stream_end.map(|end| end.saturating_sub(pos));
  match (parent_remaining, stream_remaining) {
    (Some(a), Some(b)) => Some(a.min(b)),
    (Some(a), None) => Some(a),
    (None, Some(b)) => Some(b),
    (None, None) => None,
  }
}

fn vint_to_u32_id(v: &Vint, offset: u64) -> Result<u32, ParseError> {
  if v.value > u32::MAX as u64 {
    return Err(ParseError::Malformed {
      format: "matroska",
      offset,
      reason: format!("element id {:#x} does not fit in 32 bits", v.value),
    });
  }
  Ok(v.value as u32)
}

/// Encode a `u32` element id as the canonical VINT byte sequence — useful
/// for synthetic test fixtures.  `width` is the VINT byte count (1..=4).
#[cfg(test)]
pub(crate) fn encode_id(id: u32, width: u8) -> Vec<u8> {
  assert!((1..=4).contains(&width));
  let bytes_needed = ((32 - id.leading_zeros()) as usize).div_ceil(8);
  assert!(bytes_needed <= width as usize, "id 0x{id:x} too big for width {width}");
  let mut out = vec![0u8; width as usize];
  for i in 0..width as usize {
    out[width as usize - 1 - i] = ((id >> (8 * i)) & 0xFF) as u8;
  }
  // Verify the marker bit position matches the requested width.
  let leading_byte_marker = 1u8 << (8 - width);
  assert!(
    out[0] & leading_byte_marker != 0,
    "encoded id 0x{id:x} does not have width-{width} marker bit"
  );
  out
}

/// Encode a payload size as a VINT.  Picks the smallest width that fits.
#[cfg(test)]
pub(crate) fn encode_size(size: u64) -> Vec<u8> {
  // 1-byte holds up to 2^7 - 1, 2-byte up to 2^14 - 1, … up to 8-byte
  for width in 1u8..=8 {
    let bits = 7 * width as u32;
    let max = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    if size < max {
      let marker_bit = 1u64 << (8 * width as u64 - width as u64);
      let value = marker_bit | size;
      let mut out = vec![0u8; width as usize];
      for i in 0..width as usize {
        out[width as usize - 1 - i] = ((value >> (8 * i)) & 0xFF) as u8;
      }
      return out;
    }
  }
  panic!("size {size} does not fit in 8-byte VINT");
}

/// Convenience for tests: assemble id + size + payload.
#[cfg(test)]
pub(crate) fn encode_element(id: u32, id_width: u8, payload: &[u8]) -> Vec<u8> {
  let mut out = encode_id(id, id_width);
  out.extend(encode_size(payload.len() as u64));
  out.extend_from_slice(payload);
  out
}

#[cfg(test)]
pub(crate) fn encode_element_uint(id: u32, id_width: u8, value: u64) -> Vec<u8> {
  // Encode value as the shortest big-endian sequence that round-trips.
  if value == 0 {
    return encode_element(id, id_width, &[0u8]);
  }
  let mut bytes_needed = 0usize;
  for byte in 0..8 {
    if (value >> (8 * (7 - byte))) & 0xFF != 0 {
      bytes_needed = 8 - byte;
      break;
    }
  }
  let bytes_needed = bytes_needed.max(1);
  let mut payload = Vec::with_capacity(bytes_needed);
  for i in 0..bytes_needed {
    payload.push(((value >> (8 * (bytes_needed - 1 - i))) & 0xFF) as u8);
  }
  encode_element(id, id_width, &payload)
}

#[cfg(test)]
pub(crate) fn encode_element_string(id: u32, id_width: u8, value: &str) -> Vec<u8> {
  encode_element(id, id_width, value.as_bytes())
}

#[cfg(test)]
pub(crate) fn encode_element_float(id: u32, id_width: u8, value: f64) -> Vec<u8> {
  let bytes = value.to_bits().to_be_bytes();
  encode_element(id, id_width, &bytes)
}

/// Marker for IDs that are well-known L1 elements (Segment, SeekHead, Info, …).
/// Currently unused — reserved for the "unknown size cluster recovery" path.
pub fn is_segment_level_1(id: u32) -> bool {
  matches!(
    id,
    ids::SEEK_HEAD
      | ids::INFO
      | ids::TRACKS
      | ids::ATTACHMENTS
      | ids::CHAPTERS
      | ids::TAGS
      | ids::CUES
      | ids::CLUSTER
      | ids::VOID
      | ids::CRC32
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn read_header_decodes_id_and_size() {
    // EBML head (1A 45 DF A3) + size 0x83 (= 3 stripped) + 3 bytes payload.
    let bytes = vec![0x1A, 0x45, 0xDF, 0xA3, 0x83, 0x01, 0x02, 0x03];
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    assert_eq!(h.id, ids::EBML);
    assert_eq!(h.size, Some(3));
    assert_eq!(h.header_len, 5);
    assert_eq!(h.payload_start(), 5);
    assert_eq!(h.end(), Some(8));
  }

  #[test]
  fn peek_header_does_not_advance_cursor() {
    let bytes = vec![0xEC, 0x80]; // Void, size 0
    let mut s = src(bytes);
    let h = peek_element_header(&mut s).unwrap();
    assert_eq!(h.id, ids::VOID);
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn unknown_size_sentinel_decodes_as_none() {
    // Segment id then size 0xFF (= unknown-size sentinel for width 1).
    let bytes = vec![0x18, 0x53, 0x80, 0x67, 0xFF];
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    assert_eq!(h.id, ids::SEGMENT);
    assert_eq!(h.size, None);
    assert_eq!(h.end(), None);
  }

  #[test]
  fn truncated_header_returns_eof() {
    let bytes = vec![0x1A]; // EBML id needs 4 bytes
    let mut s = src(bytes);
    let err = read_element_header(&mut s).unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn skip_payload_seeks_to_end_when_known() {
    let bytes = vec![0xEC, 0x84, 1, 2, 3, 4, 0xFF];
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    skip_payload(&mut s, &h).unwrap();
    assert_eq!(s.position(), 6);
    // Next byte after skip is the 0xFF marker.
    assert_eq!(s.read_u8().unwrap(), 0xFF);
  }

  #[test]
  fn skip_payload_is_noop_on_unknown_size() {
    let bytes = vec![0xEC, 0xFF]; // Void with unknown size
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let pos_before = s.position();
    skip_payload(&mut s, &h).unwrap();
    assert_eq!(s.position(), pos_before);
  }

  #[test]
  fn read_uint_decodes_payload() {
    // 0x83 = TrackType (1 byte ID), size 1, value 0x01 (video)
    let bytes = encode_element_uint(ids::TRACK_TYPE, 1, 0x01);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_uint(&mut s, &h).unwrap();
    assert_eq!(v, 1);
  }

  #[test]
  fn read_uint_handles_zero_byte_payload() {
    let bytes = encode_element(ids::TRACK_TYPE, 1, &[]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_uint(&mut s, &h).unwrap();
    assert_eq!(v, 0);
  }

  #[test]
  fn read_uint_rejects_unknown_size() {
    // Element with unknown size cannot be a uint.
    let bytes = vec![0x83, 0xFF];
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_uint(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_uint_rejects_oversize() {
    // Manually craft 9-byte uint
    let mut bytes = vec![0x83, 0x89]; // size = 9
    bytes.extend_from_slice(&[1u8; 9]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_uint(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_int_sign_extends() {
    // Encode -1 as 0xFF (1-byte sint)
    let bytes = encode_element(0x83, 1, &[0xFF]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_int(&mut s, &h).unwrap();
    assert_eq!(v, -1);
  }

  #[test]
  fn read_int_handles_zero_byte_payload() {
    let bytes = encode_element(0x83, 1, &[]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_int(&mut s, &h).unwrap();
    assert_eq!(v, 0);
  }

  #[test]
  fn read_int_rejects_oversize() {
    // 9-byte sint
    let mut bytes = vec![0x83, 0x89];
    bytes.extend_from_slice(&[0u8; 9]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_int(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_float_decodes_4_and_8_bytes() {
    let b4 = encode_element_float(0xB5, 1, std::f64::consts::PI);
    let mut s = src(b4);
    let h = read_element_header(&mut s).unwrap();
    let v = read_float(&mut s, &h).unwrap();
    assert!((v - std::f64::consts::PI).abs() < 1e-12);

    // Float as 4-byte
    let payload = (0.5f32).to_bits().to_be_bytes();
    let b4 = encode_element(0xB5, 1, &payload);
    let mut s = src(b4);
    let h = read_element_header(&mut s).unwrap();
    let v = read_float(&mut s, &h).unwrap();
    assert!((v - 0.5).abs() < 1e-6);
  }

  #[test]
  fn read_float_zero_byte_is_zero() {
    let bytes = encode_element(0xB5, 1, &[]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_float(&mut s, &h).unwrap();
    assert_eq!(v, 0.0);
  }

  #[test]
  fn read_float_rejects_other_sizes() {
    let bytes = encode_element(0xB5, 1, &[1, 2, 3]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_float(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_string_strips_trailing_nul() {
    let bytes = encode_element_string(0x86, 1, "matroska\0\0");
    // strip the encoded element's payload-padding by passing extra NULs in payload
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_string(&mut s, &h, 64).unwrap();
    assert_eq!(v, "matroska");
  }

  #[test]
  fn read_string_handles_all_nuls() {
    let bytes = encode_element(0x86, 1, &[0u8; 4]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let v = read_string(&mut s, &h, 64).unwrap();
    assert_eq!(v, "");
  }

  #[test]
  fn read_string_returns_malformed_on_bad_utf8() {
    // Lone continuation byte
    let bytes = encode_element(0x86, 1, &[0x80]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_string(&mut s, &h, 64).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { format: "matroska", .. }));
  }

  #[test]
  fn read_binary_enforces_cap() {
    let bytes = encode_element(0x86, 1, &[0u8; 32]);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    let err = read_binary(&mut s, &h, 16).unwrap_err();
    assert!(matches!(err, ParseError::OversizedElement { format: "matroska", .. }));
  }

  #[test]
  fn walk_children_iterates_siblings() {
    // Build a master: parent id 0xAE (TrackEntry), 2 children
    let child_a = encode_element_uint(ids::TRACK_NUMBER, 1, 1);
    let child_b = encode_element_uint(ids::TRACK_TYPE, 1, 2);
    let mut payload = Vec::new();
    payload.extend(&child_a);
    payload.extend(&child_b);
    let master = encode_element(ids::TRACK_ENTRY, 1, &payload);

    let mut s = src(master);
    let h = read_element_header(&mut s).unwrap();
    let deadline = no_deadline();
    let mut seen = Vec::new();
    walk_children(&mut s, &h, "test", &deadline, |src, ch| match ch.id {
      ids::TRACK_NUMBER => {
        let v = read_uint(src, ch)?;
        seen.push(("number", v));
        Ok(ChildAction::Consumed)
      }
      ids::TRACK_TYPE => {
        let v = read_uint(src, ch)?;
        seen.push(("type", v));
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    })
    .unwrap();
    assert_eq!(seen, vec![("number", 1), ("type", 2)]);
  }

  #[test]
  fn walk_children_skips_unwanted_payloads() {
    // Two children — closure only descends into the second one
    let a = encode_element(ids::TRACK_NUMBER, 1, &[0u8; 8]);
    let b = encode_element_uint(ids::TRACK_TYPE, 1, 0x42);
    let mut payload = Vec::new();
    payload.extend(&a);
    payload.extend(&b);
    let master = encode_element(ids::TRACK_ENTRY, 1, &payload);

    let mut s = src(master);
    let h = read_element_header(&mut s).unwrap();
    let deadline = no_deadline();
    let mut got = None;
    walk_children(&mut s, &h, "test", &deadline, |src, ch| {
      if ch.id == ids::TRACK_TYPE {
        got = Some(read_uint(src, ch)?);
        Ok(ChildAction::Consumed)
      } else {
        Ok(ChildAction::Skip)
      }
    })
    .unwrap();
    assert_eq!(got, Some(0x42));
  }

  #[test]
  fn walk_children_errors_when_child_extends_past_parent() {
    // Parent declares size 4, child declares size 8
    let mut payload = encode_id(ids::TRACK_NUMBER, 1);
    payload.extend(encode_size(8));
    // No payload bytes — parent claims size 4 but child wants 8
    let mut master = encode_id(ids::TRACK_ENTRY, 1);
    master.extend(encode_size(payload.len() as u64));
    master.extend(payload);

    let mut s = src(master);
    let h = read_element_header(&mut s).unwrap();
    let deadline = no_deadline();
    let err = walk_children(&mut s, &h, "test", &deadline, |_, _| Ok(ChildAction::Skip)).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn walk_children_honours_deadline() {
    let child = encode_element_uint(ids::TRACK_NUMBER, 1, 1);
    let master = encode_element(ids::TRACK_ENTRY, 1, &child);
    let mut s = src(master);
    let h = read_element_header(&mut s).unwrap();
    let deadline = Deadline::new(0);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let err = walk_children(&mut s, &h, "test", &deadline, |_, _| Ok(ChildAction::Skip)).unwrap_err();
    assert!(matches!(err, ParseError::Timeout { .. }));
  }

  #[test]
  fn is_segment_level_1_recognises_canonical_l1_ids() {
    for id in [
      ids::SEEK_HEAD,
      ids::INFO,
      ids::TRACKS,
      ids::ATTACHMENTS,
      ids::CHAPTERS,
      ids::TAGS,
      ids::CUES,
      ids::CLUSTER,
      ids::VOID,
      ids::CRC32,
    ] {
      assert!(is_segment_level_1(id), "{id:#x} should be L1");
    }
    assert!(!is_segment_level_1(ids::TRACK_NUMBER));
  }

  #[test]
  fn encode_helpers_round_trip() {
    let bytes = encode_element_uint(ids::TRACK_TYPE, 1, 0xAB);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    assert_eq!(h.id, ids::TRACK_TYPE);
    assert_eq!(read_uint(&mut s, &h).unwrap(), 0xAB);

    let bytes = encode_element_string(ids::CODEC_ID, 1, "V_AV1");
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    assert_eq!(h.id, ids::CODEC_ID);
    assert_eq!(read_string(&mut s, &h, 32).unwrap(), "V_AV1");

    let bytes = encode_element_float(ids::AUDIO_SAMPLING_FREQ, 1, 48_000.0);
    let mut s = src(bytes);
    let h = read_element_header(&mut s).unwrap();
    assert_eq!(read_float(&mut s, &h).unwrap(), 48_000.0);
  }
}
