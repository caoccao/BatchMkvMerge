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

//! SeekHead walker.  Mirrors `r_matroska.cpp::handle_seek_head` (lines
//! 1509-1581) — extracts `(SeekID, SeekPosition)` pairs and converts them
//! into deferred L1 positions.
//!
//! Note: matroska SeekPosition values are *Segment-relative* — they're the
//! offset from the start of the Segment's payload, **not** the start of the
//! file.  We translate to absolute file offsets before publishing.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;
use super::reader::{DeferredL1, DeferredL1Positions};

/// Walk the SeekHead at `(src.position .. parent.end)` and route each entry
/// into `deferred`.  Unknown / cluster / cues entries are ignored — those are
/// either dispatched elsewhere or not relevant to identification.
///
/// `segment_payload_start` is the absolute file offset of the enclosing
/// Segment's first payload byte.  SeekPosition values are Segment-relative
/// per the Matroska spec, so we translate them via the same arithmetic
/// `libmatroska::KaxSegment::GetGlobalPosition` performs
/// (`global = relative + segment_payload_start`).
pub(crate) fn collect_deferred(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  deferred: &mut DeferredL1Positions,
  segment_payload_start: u64,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::seek_head", deadline, |src, child| {
    if child.id != ids::SEEK {
      return Ok(ChildAction::Skip);
    }
    let entry = match read_seek_entry(src, child, deadline) {
      Ok(e) => e,
      Err(ParseError::Malformed { .. }) => {
        // Bad entry — mkvtoolnix's loop also tolerates these
        // and moves on.
        return Ok(ChildAction::Consumed);
      }
      Err(e) => return Err(e),
    };
    let Some(SeekEntry { id, position }) = entry else {
      return Ok(ChildAction::Consumed);
    };
    let absolute = segment_payload_start.saturating_add(position);
    if let Some(kind) = classify_seek_id(id) {
      deferred.push(kind, absolute);
    }
    Ok(ChildAction::Consumed)
  })
}

#[derive(Debug, Clone, Copy)]
struct SeekEntry {
  id: u32,
  position: u64,
}

fn read_seek_entry(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
) -> Result<Option<SeekEntry>, ParseError> {
  let mut id: Option<u32> = None;
  let mut position: Option<u64> = None;
  ebml::walk_children(
    src,
    parent,
    "matroska::seek_entry",
    deadline,
    |src, child| match child.id {
      ids::SEEK_ID => {
        let bytes = ebml::read_binary(src, child, 8)?;
        if bytes.is_empty() || bytes.len() > 4 {
          // Anything but 1-4 byte IDs is rejected by mkvtoolnix
          // (it discards via `k_id->GetSize() > 4` check).
          Ok(ChildAction::Consumed)
        } else {
          let mut v = 0u32;
          for b in &bytes {
            v = (v << 8) | *b as u32;
          }
          id = Some(v);
          Ok(ChildAction::Consumed)
        }
      }
      ids::SEEK_POSITION => {
        position = Some(ebml::read_uint(src, child)?);
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    },
  )?;
  match (id, position) {
    (Some(id), Some(position)) => Ok(Some(SeekEntry { id, position })),
    _ => Ok(None),
  }
}

fn classify_seek_id(id: u32) -> Option<DeferredL1> {
  match id {
    ids::INFO => Some(DeferredL1::Info),
    ids::TRACKS => Some(DeferredL1::Tracks),
    ids::ATTACHMENTS => Some(DeferredL1::Attachments),
    ids::CHAPTERS => Some(DeferredL1::Chapters),
    ids::TAGS => Some(DeferredL1::Tags),
    // A SeekHead may point at another SeekHead (PARSER-038).
    ids::SEEK_HEAD => Some(DeferredL1::SeekHead),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint};
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_seek_entry(target_id: u32, position: u64) -> Vec<u8> {
    let id_bytes: Vec<u8> = match target_id {
      id if id <= 0xFF => vec![id as u8],
      id if id <= 0xFFFF => vec![(id >> 8) as u8, (id & 0xFF) as u8],
      id if id <= 0xFFFFFF => vec![(id >> 16) as u8, (id >> 8) as u8, (id & 0xFF) as u8],
      id => id.to_be_bytes().to_vec(),
    };
    let seek_id = encode_element(ids::SEEK_ID, 2, &id_bytes);
    let seek_pos = encode_element_uint(ids::SEEK_POSITION, 2, position);
    let mut payload = Vec::new();
    payload.extend(seek_id);
    payload.extend(seek_pos);
    encode_element(ids::SEEK, 2, &payload)
  }

  #[test]
  fn collect_classifies_info_tracks_attachments_chapters_tags() {
    let mut payload = Vec::new();
    payload.extend(build_seek_entry(ids::INFO, 100));
    payload.extend(build_seek_entry(ids::TRACKS, 200));
    payload.extend(build_seek_entry(ids::ATTACHMENTS, 300));
    payload.extend(build_seek_entry(ids::CHAPTERS, 400));
    payload.extend(build_seek_entry(ids::TAGS, 500));
    let head = encode_element(ids::SEEK_HEAD, 4, &payload);

    let mut s = src(head);
    let parent = ebml::read_element_header(&mut s).unwrap();
    let mut deferred = DeferredL1Positions::default();
    collect_deferred(&mut s, &parent, &no_deadline(), &mut deferred, 0).unwrap();

    assert_eq!(deferred.take(DeferredL1::Info), vec![100]);
    assert_eq!(deferred.take(DeferredL1::Tracks), vec![200]);
    assert_eq!(deferred.take(DeferredL1::Attachments), vec![300]);
    assert_eq!(deferred.take(DeferredL1::Chapters), vec![400]);
    assert_eq!(deferred.take(DeferredL1::Tags), vec![500]);
  }

  #[test]
  fn collect_ignores_unknown_seek_ids() {
    let mut payload = Vec::new();
    payload.extend(build_seek_entry(0x12345678, 42));
    payload.extend(build_seek_entry(ids::CUES, 99));
    let head = encode_element(ids::SEEK_HEAD, 4, &payload);

    let mut s = src(head);
    let parent = ebml::read_element_header(&mut s).unwrap();
    let mut deferred = DeferredL1Positions::default();
    collect_deferred(&mut s, &parent, &no_deadline(), &mut deferred, 0).unwrap();

    assert!(deferred.take(DeferredL1::Info).is_empty());
    assert!(deferred.take(DeferredL1::Tracks).is_empty());
  }

  #[test]
  fn translate_resolves_to_absolute_offsets() {
    let payload = build_seek_entry(ids::INFO, 10);
    let head = encode_element(ids::SEEK_HEAD, 4, &payload);

    let mut s = src(head);
    let parent = ebml::read_element_header(&mut s).unwrap();
    let mut deferred = DeferredL1Positions::default();
    collect_deferred(
      &mut s,
      &parent,
      &no_deadline(),
      &mut deferred,
      /*segment_payload_start=*/ 1000,
    )
    .unwrap();
    assert_eq!(deferred.take(DeferredL1::Info), vec![1010]);
  }

  #[test]
  fn malformed_entry_does_not_abort_walk() {
    // Build an entry with missing SeekPosition followed by a valid one
    let bad_payload = encode_element(ids::SEEK_ID, 2, &[0x15, 0x49, 0xA9, 0x66]);
    let bad_entry = encode_element(ids::SEEK, 2, &bad_payload);

    let mut payload = bad_entry;
    payload.extend(build_seek_entry(ids::TRACKS, 250));
    let head = encode_element(ids::SEEK_HEAD, 4, &payload);

    let mut s = src(head);
    let parent = ebml::read_element_header(&mut s).unwrap();
    let mut deferred = DeferredL1Positions::default();
    collect_deferred(&mut s, &parent, &no_deadline(), &mut deferred, 0).unwrap();
    assert_eq!(deferred.take(DeferredL1::Tracks), vec![250]);
  }

  #[test]
  fn empty_seek_head_yields_no_deferred() {
    let head = encode_element(ids::SEEK_HEAD, 4, &[]);
    let mut s = src(head);
    let parent = ebml::read_element_header(&mut s).unwrap();
    let mut deferred = DeferredL1Positions::default();
    collect_deferred(&mut s, &parent, &no_deadline(), &mut deferred, 0).unwrap();
    for kind in [
      DeferredL1::Info,
      DeferredL1::Tracks,
      DeferredL1::Attachments,
      DeferredL1::Chapters,
      DeferredL1::Tags,
    ] {
      assert!(deferred.take(kind).is_empty());
    }
  }

  #[test]
  fn classify_seek_id_known_routes() {
    assert_eq!(classify_seek_id(ids::INFO), Some(DeferredL1::Info));
    assert_eq!(classify_seek_id(ids::TRACKS), Some(DeferredL1::Tracks));
    assert_eq!(classify_seek_id(ids::ATTACHMENTS), Some(DeferredL1::Attachments));
    assert_eq!(classify_seek_id(ids::CHAPTERS), Some(DeferredL1::Chapters));
    assert_eq!(classify_seek_id(ids::TAGS), Some(DeferredL1::Tags));
    assert_eq!(classify_seek_id(ids::CLUSTER), None);
    assert_eq!(classify_seek_id(ids::CUES), None);
  }
}
