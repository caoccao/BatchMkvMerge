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

//! ISO BMFF / QuickTime box header walker.
//!
//! Each box starts with a 32-bit big-endian size + 4-byte ASCII type. Sizes:
//! - `size > 8`            → header is 8 bytes, payload follows.
//! - `size == 1`           → "large size" — next 8 bytes hold a 64-bit size,
//!                           header total = 16 bytes.
//! - `size == 0`           → box runs to end of file (only legal at top level).
//!
//! For each header we also recognise the optional `uuid` box (where the
//! type is `uuid` and the next 16 bytes carry an extended type), but for
//! identification we never need to interpret the uuid payload — we just
//! skip past it.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

/// Wraps a 4-byte ASCII box type. The raw bytes are preserved so non-ASCII
/// types (the `©` prefix on iTunes metadata atoms is 0xA9) round-trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoxType(pub [u8; 4]);

impl BoxType {
  /// Constructor from a 4-byte ASCII literal — handy for tests.
  pub const fn new(b: &[u8; 4]) -> Self {
    Self(*b)
  }

  /// `true` when every byte is printable ASCII.  Mirrors mkvtoolnix's
  /// `fourcc_c::human_readable`.
  pub fn is_human_readable(&self) -> bool {
    self.0.iter().all(|b| (0x20..=0x7E).contains(b))
  }

  /// Lossy display.
  pub fn as_str_lossy(&self) -> String {
    self
      .0
      .iter()
      .map(|b| if (0x20..=0x7E).contains(b) { *b as char } else { '?' })
      .collect()
  }

  /// Compare against an ASCII 4-byte literal.
  pub fn eq_ascii(&self, ascii: &[u8; 4]) -> bool {
    self.0 == *ascii
  }
}

/// One MP4 box header — size and offsets decoded.
#[derive(Debug, Clone, Copy)]
pub struct BoxHeader {
  /// Absolute file offset of the first header byte.
  pub start: u64,
  /// Box type (4-byte ASCII tag).
  pub kind: BoxType,
  /// Number of bytes the header occupied (8 for normal, 16 for large size).
  pub header_len: u8,
  /// Total box size in bytes (header + payload).  `None` for the size=0
  /// "to-EOF" form.
  pub total_size: Option<u64>,
}

impl BoxHeader {
  /// Absolute offset of the first payload byte.
  pub fn payload_start(&self) -> u64 {
    self.start + self.header_len as u64
  }

  /// Absolute offset one byte past the payload.  `None` for size=0 boxes.
  pub fn end(&self) -> Option<u64> {
    self.total_size.map(|s| self.start + s)
  }

  /// Payload size in bytes.  `None` when size=0 ⇒ to-EOF.
  pub fn payload_size(&self) -> Option<u64> {
    self.total_size.map(|s| s.saturating_sub(self.header_len as u64))
  }
}

const SOFT_BOX_CAP: u64 = 64 * 1024 * 1024;

/// Read one box header at the current cursor.  Advances past the header.
pub fn read_box_header(src: &mut FileSource) -> Result<BoxHeader, ParseError> {
  let start = src.position();
  let size32 = src.read_u32_be()? as u64;
  let kind = BoxType(src.read_array::<4>()?);
  match size32 {
    0 => Ok(BoxHeader {
      start,
      kind,
      header_len: 8,
      total_size: None, // to-EOF
    }),
    1 => {
      let large = src.read_u64_be()?;
      if large < 16 {
        return Err(ParseError::Malformed {
          format: "mp4",
          offset: start,
          reason: format!("large-size {large} too small for 16-byte header"),
        });
      }
      Ok(BoxHeader {
        start,
        kind,
        header_len: 16,
        total_size: Some(large),
      })
    }
    n if n < 8 => Err(ParseError::Malformed {
      format: "mp4",
      offset: start,
      reason: format!("box size {n} smaller than 8-byte header"),
    }),
    n => Ok(BoxHeader {
      start,
      kind,
      header_len: 8,
      total_size: Some(n),
    }),
  }
}

/// Peek the next header without advancing the cursor.
pub fn peek_box_header(src: &mut FileSource) -> Result<BoxHeader, ParseError> {
  let pos = src.position();
  let h = read_box_header(src)?;
  src.seek_to(pos)?;
  Ok(h)
}

/// Seek past the payload of a box.
pub fn skip_payload(src: &mut FileSource, h: &BoxHeader) -> Result<(), ParseError> {
  if let Some(end) = h.end() {
    src.seek_to(end)?;
  } else if let Some(stream_end) = src.length() {
    src.seek_to(stream_end)?;
  }
  Ok(())
}

/// Tell the walker whether a child closure already consumed the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildAction {
  /// The closure already advanced the cursor.
  Consumed,
  /// The walker should seek past the payload itself.
  Skip,
}

/// Iterate immediate children of a parent box.
/// - The closure is invoked once per child with the cursor at the child's
///   payload start.  It may return `Consumed` (cursor will be re-aligned to
///   the child's end if known) or `Skip` (walker advances).
/// - The walker stops at the parent's end, EOF, or when the cursor stops
///   advancing (defensive — prevents infinite loops on size-0 children).
pub fn walk_children<F>(
  src: &mut FileSource,
  parent: &BoxHeader,
  stage: &'static str,
  deadline: &Deadline,
  mut on_child: F,
) -> Result<(), ParseError>
where
  F: FnMut(&mut FileSource, &BoxHeader) -> Result<ChildAction, ParseError>,
{
  let parent_end = parent.end();
  let stream_end = src.length();
  src.seek_to(parent.payload_start())?;

  loop {
    deadline.check(stage)?;
    let pos = src.position();
    if let Some(end) = parent_end {
      if pos >= end {
        break;
      }
      if end - pos < 8 {
        // Trailing padding shorter than a box header; skip past.
        src.seek_to(end)?;
        break;
      }
    }
    if let Some(end) = stream_end {
      if pos >= end {
        break;
      }
    }
    let header = match read_box_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };
    // Defensive: a child claiming to extend past the parent is malformed.
    if let (Some(end), Some(child_end)) = (parent_end, header.end()) {
      if child_end > end {
        return Err(ParseError::Malformed {
          format: "mp4",
          offset: header.start,
          reason: format!(
            "child '{}' extends past parent '{}' ({} > {})",
            header.kind.as_str_lossy(),
            parent.kind.as_str_lossy(),
            child_end,
            end
          ),
        });
      }
    }
    let action = on_child(src, &header)?;
    match action {
      ChildAction::Consumed => {
        if let Some(end) = header.end() {
          src.seek_to(end)?;
        }
      }
      ChildAction::Skip => {
        skip_payload(src, &header)?;
      }
    }
    // Defensive: ensure the cursor advanced past pos so a size-0 box
    // can't loop forever.
    if src.position() <= pos {
      return Err(ParseError::Malformed {
        format: "mp4",
        offset: pos,
        reason: format!("child '{}' did not advance cursor", header.kind.as_str_lossy()),
      });
    }
  }
  Ok(())
}

/// Read a fixed-size payload, capped to avoid runaway allocation.
pub fn read_payload(src: &mut FileSource, h: &BoxHeader, cap: u64) -> Result<Vec<u8>, ParseError> {
  let size = h.payload_size().unwrap_or(0);
  let effective_cap = cap.min(SOFT_BOX_CAP);
  if size > effective_cap {
    return Err(ParseError::OversizedElement {
      format: "mp4",
      id: u32::from_be_bytes(h.kind.0) as u64,
      size,
      cap: effective_cap,
      offset: h.start,
    });
  }
  let mut buf = vec![0u8; size as usize];
  src.read_exact(&mut buf)?;
  Ok(buf)
}

#[cfg(test)]
pub(crate) fn encode_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
  let total = (8 + payload.len()) as u32;
  let mut out = Vec::with_capacity(total as usize);
  out.extend_from_slice(&total.to_be_bytes());
  out.extend_from_slice(kind);
  out.extend_from_slice(payload);
  out
}

#[cfg(test)]
pub(crate) fn encode_large_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
  let total: u64 = 16 + payload.len() as u64;
  let mut out = Vec::with_capacity(total as usize);
  out.extend_from_slice(&1u32.to_be_bytes()); // size == 1 ⇒ large size follows
  out.extend_from_slice(kind);
  out.extend_from_slice(&total.to_be_bytes());
  out.extend_from_slice(payload);
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }
  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn reads_normal_box_header() {
    let bytes = encode_box(b"ftyp", &[1, 2, 3, 4]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    assert!(h.kind.eq_ascii(b"ftyp"));
    assert_eq!(h.header_len, 8);
    assert_eq!(h.total_size, Some(12));
    assert_eq!(h.payload_start(), 8);
    assert_eq!(h.end(), Some(12));
  }

  #[test]
  fn reads_large_box_header() {
    let bytes = encode_large_box(b"mdat", &[0xFFu8; 32]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    assert!(h.kind.eq_ascii(b"mdat"));
    assert_eq!(h.header_len, 16);
    assert_eq!(h.total_size, Some(48));
    assert_eq!(h.payload_size(), Some(32));
  }

  #[test]
  fn reads_size_zero_as_to_eof() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"mdat");
    bytes.extend_from_slice(&[0u8; 16]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    assert_eq!(h.total_size, None);
    assert_eq!(h.end(), None);
  }

  #[test]
  fn rejects_size_smaller_than_header() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&4u32.to_be_bytes());
    bytes.extend_from_slice(b"abcd");
    let mut s = src(bytes);
    let err = read_box_header(&mut s).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_large_size_smaller_than_16() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(b"mdat");
    bytes.extend_from_slice(&8u64.to_be_bytes()); // illegal: must be ≥ 16
    let mut s = src(bytes);
    let err = read_box_header(&mut s).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn peek_does_not_advance() {
    let bytes = encode_box(b"moov", &[0u8; 4]);
    let mut s = src(bytes);
    let h = peek_box_header(&mut s).unwrap();
    assert!(h.kind.eq_ascii(b"moov"));
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn skip_payload_seeks_to_end_when_known() {
    let bytes = encode_box(b"free", &[0u8; 16]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    skip_payload(&mut s, &h).unwrap();
    assert_eq!(s.position(), 24);
  }

  #[test]
  fn skip_payload_seeks_to_stream_end_when_size_zero() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"free");
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    skip_payload(&mut s, &h).unwrap();
    assert_eq!(s.position(), 16);
  }

  #[test]
  fn walks_children_in_order() {
    let a = encode_box(b"hdlr", &[1u8; 4]);
    let b = encode_box(b"mdhd", &[2u8; 4]);
    let mut payload = Vec::new();
    payload.extend(a);
    payload.extend(b);
    let parent = encode_box(b"mdia", &payload);
    let mut s = src(parent);
    let h = read_box_header(&mut s).unwrap();
    let mut seen: Vec<[u8; 4]> = Vec::new();
    walk_children(&mut s, &h, "test", &dl(), |_src, c| {
      seen.push(c.kind.0);
      Ok(ChildAction::Skip)
    })
    .unwrap();
    assert_eq!(seen, vec![*b"hdlr", *b"mdhd"]);
  }

  #[test]
  fn walk_rejects_child_extending_past_parent() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&20u32.to_be_bytes()); // child claims 20 bytes
    payload.extend_from_slice(b"trak");
    let parent = encode_box(b"moov", &payload); // parent is only 16 bytes total
    let mut s = src(parent);
    let h = read_box_header(&mut s).unwrap();
    let err = walk_children(&mut s, &h, "test", &dl(), |_, _| Ok(ChildAction::Skip)).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn walk_stops_at_trailing_padding_less_than_header() {
    // Parent has a child + 4 bytes of padding (less than 8-byte header).
    let child = encode_box(b"hdlr", &[]);
    let mut payload = child;
    payload.extend_from_slice(&[0u8; 4]);
    let parent = encode_box(b"mdia", &payload);
    let mut s = src(parent);
    let h = read_box_header(&mut s).unwrap();
    let mut count = 0;
    walk_children(&mut s, &h, "test", &dl(), |_, _| {
      count += 1;
      Ok(ChildAction::Skip)
    })
    .unwrap();
    assert_eq!(count, 1);
  }

  #[test]
  fn read_payload_capped_against_explicit_limit() {
    let bytes = encode_box(b"data", &[0u8; 32]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    let err = read_payload(&mut s, &h, 16).unwrap_err();
    assert!(matches!(err, ParseError::OversizedElement { .. }));
  }

  #[test]
  fn read_payload_returns_full_when_within_cap() {
    let bytes = encode_box(b"data", &[1, 2, 3, 4]);
    let mut s = src(bytes);
    let h = read_box_header(&mut s).unwrap();
    let v = read_payload(&mut s, &h, 1024).unwrap();
    assert_eq!(v, vec![1, 2, 3, 4]);
  }

  #[test]
  fn box_type_human_readable_predicate() {
    assert!(BoxType::new(b"ftyp").is_human_readable());
    assert!(!BoxType::new(&[0xFF, 0, 0, 0]).is_human_readable());
  }

  #[test]
  fn box_type_lossy_string_replaces_non_ascii() {
    let s = BoxType::new(&[b'a', b'b', 0xFF, b'd']).as_str_lossy();
    assert_eq!(s, "ab?d");
  }

  #[test]
  fn box_type_eq_ascii() {
    let t = BoxType::new(b"moov");
    assert!(t.eq_ascii(b"moov"));
    assert!(!t.eq_ascii(b"trak"));
  }
}
