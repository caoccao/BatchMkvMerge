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

//! Late level-1 element discovery via a bounded tail scan.  Port of
//! `r_matroska.cpp::find_level1_elements_via_analyzer` (lines 1607-1631), which
//! spins up a `kax_analyzer_c` over the last 5 MiB to recover `Info`,
//! `Tracks`, `Attachments`, `Chapters`, and `Tags` that appear after the
//! clusters in files written without a usable `SeekHead`.
//!
//! mkvtoolnix's analyzer (`kax_analyzer_c` in `parse_mode_full`) re-syncs to a
//! level-1 element boundary with `libebml::EbmlStream::FindNextElement` and
//! then walks the level-1 chain **contiguously**, skipping each element by its
//! declared size — it never inspects the bytes *inside* a Cluster payload
//! (`kax_analyzer.cpp:360-410`).  PARSER-167: the previous implementation
//! recorded *every* byte offset whose 4 bytes matched a canonical level-1 id,
//! so arbitrary media bytes inside a Cluster could be mistaken for a late
//! `Tracks` / `Tags` / `Chapters` / `Attachments` / `Info` element.
//!
//! We mirror the analyzer instead: read the last 5 MiB into memory once, then
//! resync to the earliest offset whose level-1 element chain tiles
//! *contiguously and exactly* to the segment end.  Only that chain's elements
//! are recorded; cluster payloads are jumped over by size and never scanned.
//! A chain that diverges before reaching the segment end is discarded and the
//! scan resumes from the divergence point, so the walk stays linear in the
//! window size and inside the configured parse timeout.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::io::varint::{self, VintKind};

use super::ebml::{self, ElementHeader};
use super::ids;
use super::reader::{DeferredL1, DeferredL1Positions};

/// Last-5-MiB window mkvtoolnix's analyzer inspects.
const TAIL_WINDOW: u64 = 5 * 1024 * 1024;

/// Hard cap on the number of contiguous level-1 elements walked in a single
/// chain attempt — a corrupt size VINT cannot drive an unbounded loop.
const MAX_CHAIN_ELEMENTS: usize = 4_000_000;

/// Scan the file tail for late level-1 elements and push their absolute
/// positions into `deferred`.  Best-effort: any I/O hiccup leaves `deferred`
/// untouched rather than failing the whole parse (mirrors mkvtoolnix's
/// `try { ... } catch (...) {}`).
pub(crate) fn find_level1_elements(
  src: &mut FileSource,
  segment: &ElementHeader,
  _first_cluster_pos: u64,
  deadline: &Deadline,
  deferred: &mut DeferredL1Positions,
) {
  let _ = run(src, segment, deadline, deferred);
}

fn run(
  src: &mut FileSource,
  segment: &ElementHeader,
  deadline: &Deadline,
  deferred: &mut DeferredL1Positions,
) -> Result<(), crate::media_metadata::error::ParseError> {
  let Some(file_len) = src.length() else {
    return Ok(());
  };
  let payload_start = segment.payload_start();
  // Segment end clamps the candidate validation; an unknown-size segment runs
  // to EOF.
  let segment_end = segment.end().unwrap_or(file_len).min(file_len);
  if payload_start >= segment_end {
    return Ok(());
  }

  let start = payload_start.max(file_len.saturating_sub(TAIL_WINDOW));
  let region_end = segment_end;
  if start >= region_end {
    return Ok(());
  }
  let region_len = (region_end - start) as usize;

  src.seek_to(start)?;
  let mut buf = vec![0u8; region_len];
  let read = src.read_at_most(&mut buf)?;
  buf.truncate(read);
  if buf.len() < 4 {
    return Ok(());
  }

  // The contiguous chain must tile exactly to the segment end; trailing bytes
  // beyond what we buffered cannot be validated, so clamp the target there.
  let buf_end_abs = start + buf.len() as u64;
  let limit_abs = segment_end.min(buf_end_abs);

  // Resync to the earliest offset whose level-1 chain tiles to `limit_abs`,
  // then record its elements.  At most one such chain can exist (two chains
  // tiling to the same end would have to overlap), so the first hit wins.
  let mut i = 0usize;
  let mut steps = 0usize;
  while i + 1 < buf.len() {
    if (steps & 0xFFFF) == 0 {
      deadline.check("matroska::tail_analyzer")?;
    }
    steps += 1;
    let walk = walk_chain(&buf, i, start, limit_abs);
    if walk.reached_end {
      for (kind, abs) in walk.records {
        deferred.push(kind, abs);
      }
      break;
    }
    // Resume past the elements we just walked (a contiguous but
    // non-terminating chain), or advance one byte when nothing decoded.
    i = walk.next_index.max(i + 1);
  }
  Ok(())
}

/// Outcome of walking a contiguous level-1 chain from one buffer offset.
struct ChainWalk {
  /// Of-interest elements (`Info` / `Tracks` / …) found along the chain.
  records: Vec<(DeferredL1, u64)>,
  /// Buffer index where the walk stopped (the divergence point, or the index
  /// of `limit_abs` when the chain tiled exactly to the segment end).
  next_index: usize,
  /// True when the chain tiled contiguously and exactly to `limit_abs`.
  reached_end: bool,
}

/// Walk level-1 elements contiguously from buffer index `i0`, decoding each
/// header and jumping over its payload by the declared size.  The chain is
/// only accepted (`reached_end == true`) when its elements tile exactly to
/// `limit_abs`; this is the structured-walk invariant that keeps Cluster-
/// internal bytes from being mistaken for late headers.
fn walk_chain(buf: &[u8], i0: usize, start: u64, limit_abs: u64) -> ChainWalk {
  let mut i = i0;
  let mut records = Vec::new();
  let mut walked = 0usize;
  loop {
    let abs = start + i as u64;
    if abs == limit_abs {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: true,
      };
    }
    if abs > limit_abs {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    }
    let Some((id, size, header_len)) = decode_header_at(buf, i) else {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    };
    // Only canonical level-1 ids form the chain (Cluster / Cues / SeekHead /
    // Void / CRC-32 are valid links we step over but never record).
    if !ebml::is_segment_level_1(id) {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    }
    // An unknown-size element cannot be skipped deterministically, so it ends
    // the contiguous walk.
    let Some(size) = size else {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    };
    let end_abs = abs + header_len as u64 + size;
    if end_abs > limit_abs {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    }
    if let Some(kind) = classify(id) {
      records.push((kind, abs));
    }
    // `end_abs <= limit_abs <= buf_end_abs`, so this fits in the buffer.
    i = (end_abs - start) as usize;
    walked += 1;
    if walked > MAX_CHAIN_ELEMENTS {
      return ChainWalk {
        records,
        next_index: i,
        reached_end: false,
      };
    }
  }
}

/// Decode an EBML element header (id + size) straight from the in-memory tail
/// buffer at `i`.  Returns `None` when the bytes do not form a level-1-width
/// id (≤ 4 bytes) followed by a size VINT.
fn decode_header_at(buf: &[u8], i: usize) -> Option<(u32, Option<u64>, usize)> {
  let slice = buf.get(i..)?;
  let (id_vint, id_len) = varint::decode(slice, VintKind::IdMarker).ok()?;
  if id_vint.width > 4 || id_vint.value > u32::MAX as u64 {
    return None;
  }
  let rest = slice.get(id_len..)?;
  let (size_vint, size_len) = varint::decode(rest, VintKind::Stripped).ok()?;
  let size = if size_vint.is_unknown_size() {
    None
  } else {
    Some(size_vint.value)
  };
  Some((id_vint.value as u32, size, id_len + size_len))
}

/// The five element kinds mkvtoolnix's analyzer registers — not SeekHead and
/// not Cues.
fn classify(id: u32) -> Option<DeferredL1> {
  match id {
    ids::INFO => Some(DeferredL1::Info),
    ids::TRACKS => Some(DeferredL1::Tracks),
    ids::ATTACHMENTS => Some(DeferredL1::Attachments),
    ids::CHAPTERS => Some(DeferredL1::Chapters),
    ids::TAGS => Some(DeferredL1::Tags),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint, encode_id, encode_size};
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn segment_header(payload_len: u64) -> ElementHeader {
    // A finite-size Segment header whose payload starts at byte 0 of our
    // synthetic region (we only use payload_start / end during validation).
    ElementHeader {
      start: 0,
      id: ids::SEGMENT,
      size: Some(payload_len),
      header_len: 0,
    }
  }

  #[test]
  fn finds_tracks_and_tags_in_tail() {
    // Region layout: [cluster-ish filler][Tracks][Tags].
    let filler = vec![0u8; 16];
    let tracks = {
      let mut t = Vec::new();
      t.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
      let entry = encode_element(ids::TRACK_ENTRY, 1, &t);
      encode_element(ids::TRACKS, 4, &entry)
    };
    let tags = encode_element(ids::TAGS, 4, &encode_element(ids::TAG, 2, &[]));

    let mut bytes = Vec::new();
    bytes.extend(&filler);
    let tracks_pos = bytes.len() as u64;
    bytes.extend(&tracks);
    let tags_pos = bytes.len() as u64;
    bytes.extend(&tags);

    let seg = segment_header(bytes.len() as u64);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut deferred = DeferredL1Positions::default();
    find_level1_elements(&mut s, &seg, 0, &no_deadline(), &mut deferred);

    assert_eq!(deferred.take(DeferredL1::Tracks), vec![tracks_pos]);
    assert_eq!(deferred.take(DeferredL1::Tags), vec![tags_pos]);
  }

  #[test]
  fn rejects_signature_with_oversized_size() {
    // A TRACKS id followed by a finite size that spills past the segment end
    // must be rejected as a false match.
    let mut bytes = vec![0u8; 8];
    bytes.extend(ids::TRACKS.to_be_bytes()); // id
    bytes.push(0x88); // 1-byte size VINT = 8, but only 0 payload bytes present
    let seg = segment_header(bytes.len() as u64);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut deferred = DeferredL1Positions::default();
    find_level1_elements(&mut s, &seg, 0, &no_deadline(), &mut deferred);
    assert!(deferred.take(DeferredL1::Tracks).is_empty());
  }

  #[test]
  fn finds_late_chapters_attachments_info() {
    let info = encode_element(ids::INFO, 4, &encode_element_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000));
    let chapters = encode_element(ids::CHAPTERS, 4, &[]);
    let attachments = encode_element(ids::ATTACHMENTS, 4, &[]);

    let mut bytes = Vec::new();
    let info_pos = bytes.len() as u64;
    bytes.extend(&info);
    let chap_pos = bytes.len() as u64;
    bytes.extend(&chapters);
    let att_pos = bytes.len() as u64;
    bytes.extend(&attachments);

    let seg = segment_header(bytes.len() as u64);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut deferred = DeferredL1Positions::default();
    find_level1_elements(&mut s, &seg, 0, &no_deadline(), &mut deferred);

    assert_eq!(deferred.take(DeferredL1::Info), vec![info_pos]);
    assert_eq!(deferred.take(DeferredL1::Chapters), vec![chap_pos]);
    assert_eq!(deferred.take(DeferredL1::Attachments), vec![att_pos]);
  }

  #[test]
  fn cluster_internal_signature_is_not_recorded() {
    // PARSER-167: a Cluster payload that happens to contain bytes matching a
    // canonical level-1 id (here a fake TRACKS element with a fitting size)
    // must be skipped over by size, not mistaken for a late Tracks element.
    let fake_inner = {
      let mut v = encode_id(ids::TRACKS, 4);
      v.extend(encode_size(0)); // size 0 — would have validated under the old scan
      v.extend([0xAB, 0xCD, 0xEF]); // arbitrary trailing cluster bytes
      v
    };
    let cluster = encode_element(ids::CLUSTER, 4, &fake_inner);
    let real_tracks = {
      let entry = encode_element_uint(ids::TRACK_NUMBER, 1, 1);
      encode_element(ids::TRACKS, 4, &encode_element(ids::TRACK_ENTRY, 1, &entry))
    };

    let mut bytes = Vec::new();
    bytes.extend(&cluster);
    let real_tracks_pos = bytes.len() as u64;
    bytes.extend(&real_tracks);

    let seg = segment_header(bytes.len() as u64);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut deferred = DeferredL1Positions::default();
    find_level1_elements(&mut s, &seg, 0, &no_deadline(), &mut deferred);

    // Only the genuine Tracks element after the cluster is recorded — the
    // fake id buried in the cluster payload is invisible to the chain walk.
    assert_eq!(deferred.take(DeferredL1::Tracks), vec![real_tracks_pos]);
  }

  #[test]
  fn chain_that_does_not_tile_to_segment_end_is_ignored() {
    // A valid Tracks element followed by trailing junk that is not a level-1
    // element: the chain never tiles exactly to the segment end, so nothing
    // is recorded (mirrors the analyzer refusing a broken chain).
    let tracks = encode_element(ids::TRACKS, 4, &encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    let mut bytes = Vec::new();
    bytes.extend(&tracks);
    bytes.extend([0x11, 0x22, 0x33, 0x44]); // junk past the Tracks element

    let seg = segment_header(bytes.len() as u64);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut deferred = DeferredL1Positions::default();
    find_level1_elements(&mut s, &seg, 0, &no_deadline(), &mut deferred);
    assert!(deferred.take(DeferredL1::Tracks).is_empty());
  }

  #[test]
  fn classify_only_matches_five_kinds() {
    assert_eq!(classify(ids::INFO), Some(DeferredL1::Info));
    assert_eq!(classify(ids::TRACKS), Some(DeferredL1::Tracks));
    assert_eq!(classify(ids::ATTACHMENTS), Some(DeferredL1::Attachments));
    assert_eq!(classify(ids::CHAPTERS), Some(DeferredL1::Chapters));
    assert_eq!(classify(ids::TAGS), Some(DeferredL1::Tags));
    assert_eq!(classify(ids::CUES), None);
    assert_eq!(classify(ids::SEEK_HEAD), None);
    assert_eq!(classify(ids::CLUSTER), None);
  }
}
