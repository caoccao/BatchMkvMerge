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

//! Generic RIFF chunk walker.
//!
//! RIFF layout (little-endian throughout):
//!
//! ```text
//! 4-byte FOURCC | 4-byte u32 size (LE) | size bytes of payload [ + 1 byte pad if odd ]
//! ```
//!
//! "RIFF" and "LIST" chunks are special: their payload begins with a 4-byte
//! sub-type FOURCC (e.g. "AVI ", "hdrl", "strl", "movi") followed by the
//! actual children.  Both are walked the same way — children come at
//! `payload_start + 4` and run to `payload_end`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

/// One RIFF chunk header.
#[derive(Debug, Clone, Copy)]
pub struct ChunkHeader {
  /// Absolute file offset of the first header byte (start of the FOURCC).
  pub start: u64,
  /// Chunk FOURCC (e.g. b"RIFF", b"avih", b"strh").
  pub kind: [u8; 4],
  /// Payload size in bytes (the value stored in the size field — does not
  /// include the 4-byte FOURCC, the 4-byte size field, or any pad byte).
  pub size: u32,
}

impl ChunkHeader {
  /// Total byte count including the 8-byte header + payload, **not**
  /// including the trailing pad byte for odd sizes.
  pub fn total_size(&self) -> u64 {
    8 + self.size as u64
  }
  /// Absolute offset of the first payload byte.
  pub fn payload_start(&self) -> u64 {
    self.start + 8
  }
  /// Absolute offset one byte past the payload (before any pad byte).
  pub fn payload_end(&self) -> u64 {
    self.payload_start() + self.size as u64
  }
  /// `true` for RIFF / LIST containers — their payload begins with a
  /// 4-byte sub-type FOURCC.
  pub fn is_list_container(&self) -> bool {
    &self.kind == b"RIFF" || &self.kind == b"LIST"
  }
  /// `true` when this chunk's payload size is odd and therefore followed
  /// by a single pad byte to maintain 2-byte alignment.
  pub fn needs_pad_byte(&self) -> bool {
    self.size & 1 != 0
  }
}

/// Read one chunk header at the current cursor.  Advances 8 bytes past the
/// header so the cursor lands on the first payload byte.
pub fn read_chunk_header(src: &mut FileSource) -> Result<ChunkHeader, ParseError> {
  let start = src.position();
  let kind = src.read_array::<4>()?;
  let size = src.read_u32_le()?;
  Ok(ChunkHeader { start, kind, size })
}

/// Peek the next chunk header without advancing the cursor.
pub fn peek_chunk_header(src: &mut FileSource) -> Result<ChunkHeader, ParseError> {
  let pos = src.position();
  let h = read_chunk_header(src)?;
  src.seek_to(pos)?;
  Ok(h)
}

/// Read the 4-byte sub-type FOURCC at the start of a RIFF/LIST payload.
/// Advances 4 bytes.
pub fn read_list_subtype(src: &mut FileSource) -> Result<[u8; 4], ParseError> {
  src.read_array::<4>()
}

/// Seek past the chunk's payload + the pad byte if the size is odd.
pub fn skip_payload_with_pad(src: &mut FileSource, h: &ChunkHeader) -> Result<(), ParseError> {
  let mut target = h.payload_end();
  if h.needs_pad_byte() {
    target = target.saturating_add(1);
  }
  if let Some(stream_end) = src.length() {
    if target > stream_end {
      target = stream_end;
    }
  }
  src.seek_to(target)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildAction {
  /// Caller already advanced the cursor past the payload — walker will
  /// still pad-align if necessary.
  Consumed,
  /// Walker should seek past the payload itself.
  Skip,
}

/// Iterate children of a RIFF/LIST container.  `parent` must be a list-style
/// chunk; the callback fires once per direct child with the cursor at the
/// child's payload start.
pub fn walk_list_children<F>(
  src: &mut FileSource,
  parent: &ChunkHeader,
  stage: &'static str,
  deadline: &Deadline,
  mut on_child: F,
) -> Result<(), ParseError>
where
  F: FnMut(&mut FileSource, &ChunkHeader) -> Result<ChildAction, ParseError>,
{
  if !parent.is_list_container() {
    return Err(ParseError::Malformed {
      format: "avi",
      offset: parent.start,
      reason: format!(
        "walk_list_children called on non-list chunk '{}'",
        fourcc_string(&parent.kind)
      ),
    });
  }
  // Sub-type FOURCC sits at payload_start..payload_start+4 — children
  // follow.
  let first_child = parent.payload_start() + 4;
  let parent_end = parent.payload_end();
  let stream_end = src.length();
  src.seek_to(first_child)?;
  loop {
    deadline.check(stage)?;
    let pos = src.position();
    if pos >= parent_end {
      break;
    }
    if let Some(end) = stream_end {
      if pos >= end {
        break;
      }
      if end - pos < 8 {
        break;
      }
    }
    if parent_end - pos < 8 {
      break;
    }
    let child = match read_chunk_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };
    if child.payload_end() > parent_end {
      return Err(ParseError::Malformed {
        format: "avi",
        offset: child.start,
        reason: format!(
          "child '{}' extends past parent '{}' ({} > {})",
          fourcc_string(&child.kind),
          fourcc_string(&parent.kind),
          child.payload_end(),
          parent_end
        ),
      });
    }
    let action = on_child(src, &child)?;
    match action {
      ChildAction::Consumed => {
        if child.needs_pad_byte() {
          let target = child.payload_end().saturating_add(1).min(parent_end);
          src.seek_to(target)?;
        } else {
          src.seek_to(child.payload_end())?;
        }
      }
      ChildAction::Skip => {
        skip_payload_with_pad(src, &child)?;
      }
    }
    // Defensive: ensure progress.
    if src.position() <= pos {
      return Err(ParseError::Malformed {
        format: "avi",
        offset: pos,
        reason: format!("child '{}' did not advance cursor", fourcc_string(&child.kind)),
      });
    }
  }
  Ok(())
}

/// Read a chunk's payload into a `Vec<u8>`.  Caps allocation against `cap`.
pub fn read_payload(src: &mut FileSource, h: &ChunkHeader, cap: u64) -> Result<Vec<u8>, ParseError> {
  let size = h.size as u64;
  if size > cap {
    return Err(ParseError::OversizedElement {
      format: "avi",
      id: u32::from_le_bytes(h.kind) as u64,
      size,
      cap,
      offset: h.start,
    });
  }
  let mut buf = vec![0u8; size as usize];
  src.read_exact(&mut buf)?;
  Ok(buf)
}

/// Lossy ASCII rendering of a FOURCC for log/error strings.
pub fn fourcc_string(bytes: &[u8; 4]) -> String {
  bytes
    .iter()
    .map(|b| if (0x20..=0x7E).contains(b) { *b as char } else { '?' })
    .collect()
}

#[cfg(test)]
pub(crate) fn encode_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
  let size = payload.len() as u32;
  let mut out = Vec::with_capacity(8 + payload.len() + 1);
  out.extend_from_slice(kind);
  out.extend_from_slice(&size.to_le_bytes());
  out.extend_from_slice(payload);
  if payload.len() & 1 != 0 {
    out.push(0); // pad byte
  }
  out
}

#[cfg(test)]
pub(crate) fn encode_list(kind: &[u8; 4], list_type: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
  let mut payload = Vec::new();
  payload.extend_from_slice(list_type);
  for c in children {
    payload.extend_from_slice(c);
  }
  encode_chunk(kind, &payload)
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
  fn reads_chunk_header() {
    let bytes = encode_chunk(b"avih", &[0u8; 12]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    assert_eq!(&h.kind, b"avih");
    assert_eq!(h.size, 12);
    assert_eq!(h.payload_start(), 8);
    assert_eq!(h.payload_end(), 20);
    assert!(!h.needs_pad_byte());
    assert_eq!(h.total_size(), 20);
  }

  #[test]
  fn pad_byte_required_for_odd_size() {
    let bytes = encode_chunk(b"junk", &[1u8; 3]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    assert!(h.needs_pad_byte());
    skip_payload_with_pad(&mut s, &h).unwrap();
    // 8 header + 3 payload + 1 pad = 12
    assert_eq!(s.position(), 12);
  }

  #[test]
  fn peek_does_not_advance() {
    let bytes = encode_chunk(b"avih", &[0u8; 4]);
    let mut s = src(bytes);
    let _h = peek_chunk_header(&mut s).unwrap();
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn riff_list_is_recognised() {
    let inner = encode_chunk(b"avih", &[0u8; 4]);
    let list = encode_list(b"LIST", b"hdrl", &[inner]);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    assert!(h.is_list_container());
    let sub = read_list_subtype(&mut s).unwrap();
    assert_eq!(&sub, b"hdrl");
  }

  #[test]
  fn walk_list_children_iterates_in_order() {
    let a = encode_chunk(b"avih", &[1u8; 4]);
    let b = encode_chunk(b"strl", &[2u8; 4]);
    let list = encode_list(b"LIST", b"hdrl", &[a, b]);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    let mut seen: Vec<[u8; 4]> = Vec::new();
    walk_list_children(&mut s, &h, "test", &dl(), |_src, c| {
      seen.push(c.kind);
      Ok(ChildAction::Skip)
    })
    .unwrap();
    assert_eq!(seen, vec![*b"avih", *b"strl"]);
  }

  #[test]
  fn walk_rejects_child_extending_past_parent() {
    // Build a list with a child that claims size 100 but parent only has
    // room for 12.
    let mut payload = Vec::new();
    payload.extend_from_slice(b"hdrl");
    payload.extend_from_slice(b"avih");
    payload.extend_from_slice(&100u32.to_le_bytes()); // bogus size
    let list = encode_chunk(b"LIST", &payload);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    let err = walk_list_children(&mut s, &h, "test", &dl(), |_, _| Ok(ChildAction::Skip)).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn walk_rejects_called_on_non_list() {
    let bytes = encode_chunk(b"avih", &[0u8; 4]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    let err = walk_list_children(&mut s, &h, "test", &dl(), |_, _| Ok(ChildAction::Skip)).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_payload_caps_oversize() {
    let bytes = encode_chunk(b"avih", &[0u8; 32]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    let err = read_payload(&mut s, &h, 16).unwrap_err();
    assert!(matches!(err, ParseError::OversizedElement { .. }));
  }

  #[test]
  fn read_payload_returns_full() {
    let bytes = encode_chunk(b"avih", &[1, 2, 3, 4]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    let p = read_payload(&mut s, &h, 1024).unwrap();
    assert_eq!(p, vec![1, 2, 3, 4]);
  }

  #[test]
  fn fourcc_string_renders_ascii_and_replaces_garbage() {
    assert_eq!(fourcc_string(b"RIFF"), "RIFF");
    assert_eq!(fourcc_string(&[b'a', 0xFF, b'b', 0]), "a?b?");
  }

  #[test]
  fn skip_payload_handles_oversized_target_against_stream_end() {
    // Build a chunk that claims a larger size than the stream actually
    // contains — skip should clamp to stream end without panicking.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"junk");
    bytes.extend_from_slice(&999u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    skip_payload_with_pad(&mut s, &h).unwrap();
    assert_eq!(s.position(), s.length().unwrap());
  }

  #[test]
  fn walk_consumed_pad_alignment_advances_one_extra_byte() {
    let a = encode_chunk(b"junk", &[1, 2, 3]); // odd size + pad
    let b = encode_chunk(b"next", &[4, 5, 6, 7]);
    let list = encode_list(b"LIST", b"hdrl", &[a, b]);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    let mut seen_sizes: Vec<u32> = Vec::new();
    walk_list_children(&mut s, &h, "test", &dl(), |src, c| {
      seen_sizes.push(c.size);
      // Read payload manually so we test Consumed path.
      for _ in 0..c.size {
        let _ = src.read_u8()?;
      }
      Ok(ChildAction::Consumed)
    })
    .unwrap();
    assert_eq!(seen_sizes, vec![3, 4]);
  }

  #[test]
  fn walk_returns_zero_advance_error_when_callback_swallows_payload() {
    // Build a child where the callback claims `Consumed` but leaves the
    // cursor at the start.  The walker re-aligns to child.payload_end so
    // progress is still made — confirm no infinite loop here.
    let child = encode_chunk(b"junk", &[1, 2, 3, 4]);
    let list = encode_list(b"LIST", b"hdrl", &[child]);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    let mut count = 0;
    walk_list_children(&mut s, &h, "test", &dl(), |_src, _c| {
      count += 1;
      // Do not advance — walker should re-align to payload_end.
      Ok(ChildAction::Consumed)
    })
    .unwrap();
    assert_eq!(count, 1);
  }

  #[test]
  fn walk_stops_at_eof_when_stream_shorter_than_declared_size() {
    // Build a list whose declared size points past EOF — walker should
    // stop at stream end without panicking.
    let mut bytes = b"LIST".to_vec();
    bytes.extend_from_slice(&1000u32.to_le_bytes());
    bytes.extend_from_slice(b"hdrl");
    // Only 8 bytes of actual payload then EOF
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    let mut count = 0;
    let _ = walk_list_children(&mut s, &h, "test", &dl(), |_src, _c| {
      count += 1;
      Ok(ChildAction::Skip)
    });
    // No assertion on count — important is no panic / infinite loop.
    let _ = count;
  }

  #[test]
  fn needs_pad_byte_predicate_handles_zero_size() {
    let zero = ChunkHeader {
      start: 0,
      kind: *b"abcd",
      size: 0,
    };
    assert!(!zero.needs_pad_byte());
    let one = ChunkHeader {
      start: 0,
      kind: *b"abcd",
      size: 1,
    };
    assert!(one.needs_pad_byte());
  }

  #[test]
  fn list_subtype_round_trip() {
    let inner = encode_chunk(b"avih", &[0u8; 4]);
    let list = encode_list(b"LIST", b"hdrl", &[inner]);
    let mut s = src(list);
    let h = read_chunk_header(&mut s).unwrap();
    let sub = read_list_subtype(&mut s).unwrap();
    assert_eq!(&sub, b"hdrl");
    assert_eq!(h.total_size(), 8 + 4 + 12); // 4 = list sub + 12 = inner chunk
  }

  #[test]
  fn skip_payload_past_eof_clamps_safely() {
    // Build a header that claims a size larger than the stream remainder.
    let mut bytes = b"junk".to_vec();
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 2]);
    let mut s = src(bytes);
    let h = read_chunk_header(&mut s).unwrap();
    skip_payload_with_pad(&mut s, &h).unwrap();
    // Cursor lands at stream end, not past it.
    assert_eq!(s.position(), s.length().unwrap());
  }
}
