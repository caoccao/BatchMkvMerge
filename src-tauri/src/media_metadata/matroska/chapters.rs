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

//! Chapter summarizer.  Mirrors `r_matroska.cpp::handle_chapters`
//! (lines 942-977) — but mkvmerge identification mode only surfaces
//! `num_entries` / `num_editions` (matching `id_info.h` schema v20).
//! We count edition entries and chapter atoms; nothing else is read.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  let mut editions: u32 = 0;
  let mut atoms: u32 = 0;
  ebml::walk_children(src, parent, "matroska::chapters", deadline, |src, child| {
    if child.id == ids::EDITION_ENTRY {
      editions += 1;
      count_atoms_in_edition(src, child, deadline, &mut atoms)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })?;
  out.chapters.num_editions = out.chapters.num_editions.saturating_add(editions);
  out.chapters.num_entries = out.chapters.num_entries.saturating_add(atoms);
  Ok(())
}

fn count_atoms_in_edition(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  atoms: &mut u32,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::edition_entry", deadline, |src, child| {
    if child.id == ids::CHAPTER_ATOM {
      *atoms += 1;
      count_sub_atoms(src, child, deadline, atoms)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

fn count_sub_atoms(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  atoms: &mut u32,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::chapter_atom", deadline, |src, child| {
    if child.id == ids::CHAPTER_ATOM {
      *atoms += 1;
      count_sub_atoms(src, child, deadline, atoms)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint};
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn parse_chapters(payload: Vec<u8>) -> MediaMetadata {
    let bytes = encode_element(ids::CHAPTERS, 4, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    out
  }

  fn build_atom(uid: u64) -> Vec<u8> {
    let payload = encode_element_uint(ids::CHAPTER_UID, 2, uid);
    encode_element(ids::CHAPTER_ATOM, 1, &payload)
  }

  fn build_edition(atoms: usize) -> Vec<u8> {
    let mut payload = Vec::new();
    for i in 0..atoms {
      payload.extend(build_atom(i as u64 + 1));
    }
    encode_element(ids::EDITION_ENTRY, 2, &payload)
  }

  #[test]
  fn counts_editions_and_atoms() {
    let mut payload = Vec::new();
    payload.extend(build_edition(3));
    payload.extend(build_edition(2));
    let m = parse_chapters(payload);
    assert_eq!(m.chapters.num_editions, 2);
    assert_eq!(m.chapters.num_entries, 5);
  }

  #[test]
  fn empty_chapters_stays_at_zero() {
    let m = parse_chapters(Vec::new());
    assert_eq!(m.chapters.num_editions, 0);
    assert_eq!(m.chapters.num_entries, 0);
  }

  #[test]
  fn nested_atoms_counted() {
    // Build an atom that contains a nested ChapterAtom child
    let inner = build_atom(99);
    let mut outer_payload = encode_element_uint(ids::CHAPTER_UID, 2, 1);
    outer_payload.extend(inner);
    let outer = encode_element(ids::CHAPTER_ATOM, 1, &outer_payload);
    let edition = encode_element(ids::EDITION_ENTRY, 2, &outer);
    let m = parse_chapters(edition);
    assert_eq!(m.chapters.num_editions, 1);
    assert_eq!(m.chapters.num_entries, 2); // outer + inner
  }

  #[test]
  fn non_edition_children_ignored() {
    let mut payload = Vec::new();
    payload.extend(encode_element(0x80, 1, &[1, 2, 3]));
    payload.extend(build_edition(1));
    let m = parse_chapters(payload);
    assert_eq!(m.chapters.num_editions, 1);
    assert_eq!(m.chapters.num_entries, 1);
  }
}
