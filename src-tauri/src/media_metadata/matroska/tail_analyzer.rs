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
//! mkvtoolnix's analyzer re-syncs to known level-1 IDs with
//! `libebml::EbmlStream::FindNextElement`. We mirror that by reading the tail
//! into memory once and scanning for the canonical 4-byte element IDs, then
//! validating each candidate against an `ElementHeader` read at that offset
//! (the size VINT must be finite and the element must fit inside the segment).
//! The bounded 5 MiB window keeps this inside the configured parse timeout.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::io::file_source::FileSource;

use super::ebml::{self, ElementHeader};
use super::ids;
use super::reader::{DeferredL1, DeferredL1Positions};

/// Last-5-MiB window mkvtoolnix's analyzer inspects.
const TAIL_WINDOW: u64 = 5 * 1024 * 1024;

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

  let scan_end = buf.len() - 4;
  for i in 0..=scan_end {
    if (i & 0xFFFF) == 0 {
      deadline.check("matroska::tail_analyzer")?;
    }
    let sig = u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
    let Some(kind) = classify(sig) else {
      continue;
    };
    let abs = start + i as u64;
    if validate(src, sig, abs, segment_end).unwrap_or(false) {
      deferred.push(kind, abs);
    }
  }
  Ok(())
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

/// Confirm a signature match is a genuine element: the header must decode to
/// the expected id with a finite size whose payload stays inside the segment.
fn validate(
  src: &mut FileSource,
  expected_id: u32,
  abs: u64,
  segment_end: u64,
) -> Result<bool, crate::media_metadata::error::ParseError> {
  src.seek_to(abs)?;
  let header = match ebml::read_element_header(src) {
    Ok(h) => h,
    Err(_) => return Ok(false),
  };
  if header.id != expected_id {
    return Ok(false);
  }
  match header.end() {
    Some(end) if end <= segment_end => Ok(true),
    _ => Ok(false),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint};
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
