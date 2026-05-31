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

//! `udta` (user data) walker.  Two common shapes:
//!
//! 1. `udta` → `meta` → `hdlr`("mdir") + `ilst` (iTunes path).
//! 2. `udta` → `meta` → `keys` + `ilst` (QuickTime keyed path; we recognise
//!    the meta box but only the iTunes shape is decoded for now).
//!
//! We walk into either `meta` directly or through `udta`.  The actual tag
//! extraction happens in [`super::ilst`].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::ilst;

/// QuickTime `©nam` title atom FOURCC (`0xA9 'n' 'a' 'm'`).
const COPYRIGHT_NAME: [u8; 4] = [0xA9, b'n', b'a', b'm'];

pub fn parse_udta(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::udta", deadline, |src, child| {
    if child.kind.eq_ascii(b"meta") {
      parse_meta(src, child, deadline, out)?;
      Ok(ChildAction::Consumed)
    } else if &child.kind.0 == b"chpl" {
      // PARSER-143: Nero-style chapter list (r_qtmp4.cpp:987-1018).
      parse_chpl(src, child, out)?;
      Ok(ChildAction::Consumed)
    } else if child.kind.0 == COPYRIGHT_NAME {
      // PARSER-144: a direct QuickTime `udta/©nam` carries the file title for
      // older files without an iTunes `meta/ilst` wrapper
      // (r_qtmp4.cpp:974-979).
      parse_copyright_name(src, child, out)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

/// Read a direct `udta/©nam` string atom.  Layout: 2-byte text length +
/// 2-byte language code, then the UTF-8 string.  mkvtoolnix reads from
/// offset 4 to the end of the atom and strips surrounding whitespace.
fn parse_copyright_name(src: &mut FileSource, header: &BoxHeader, out: &mut MediaMetadata) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, 64 * 1024)?;
  if payload.len() <= 4 {
    return Ok(());
  }
  let text = String::from_utf8_lossy(&payload[4..]).trim().to_string();
  if !text.is_empty() {
    out.container.properties.title = Some(text);
  }
  Ok(())
}

/// Parse a Nero `chpl` chapter list.  Layout: version(1) + flags(3) +
/// reserved(4) + count(1), then `count` entries of `timestamp(8, 100 ns
/// units) + name_len(1) + name`.  We surface the entry count as a single
/// chapter edition (titles/timecodes are not extracted at identification).
fn parse_chpl(src: &mut FileSource, header: &BoxHeader, out: &mut MediaMetadata) -> Result<(), ParseError> {
  // mkvtoolnix only keeps the first chapter source it sees.
  if out.chapters.num_entries != 0 {
    return Ok(());
  }
  let payload = atom::read_payload(src, header, 1024 * 1024)?;
  if payload.len() < 9 {
    return Ok(());
  }
  let count = payload[8] as usize;
  if count == 0 {
    return Ok(());
  }
  // Validate by walking the variable-length entries; stop early on truncation
  // so we report only the entries that are actually present.
  let mut pos = 9usize;
  let mut entries = 0u32;
  for _ in 0..count {
    if pos + 9 > payload.len() {
      break;
    }
    let name_len = payload[pos + 8] as usize;
    pos += 9 + name_len;
    if pos > payload.len() {
      break;
    }
    entries += 1;
  }
  if entries != 0 {
    out.chapters.num_entries = entries;
    out.chapters.num_editions = 1;
  }
  Ok(())
}

pub fn parse_meta(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  // ISO `meta` is a FullBox (4-byte version+flags prefix); QuickTime `meta`
  // is a plain container.  Sniff which one we have by peeking the first
  // 8 bytes — if they look like a child box header, we treat it as QT.
  let payload_start = parent.payload_start();
  src.seek_to(payload_start)?;
  let peeked = match atom::peek_box_header(src) {
    Ok(h) => h,
    Err(_) => return Ok(()),
  };
  let is_iso_full_box = !peeked.kind.is_human_readable();
  if is_iso_full_box {
    // Skip 4-byte FullBox header.
    src.seek_to(payload_start + 4)?;
  }
  let synthetic = BoxHeader {
    start: parent.start,
    kind: parent.kind,
    header_len: (src.position() - parent.start) as u8,
    total_size: parent.total_size,
  };
  atom::walk_children(src, &synthetic, "mp4::meta", deadline, |src, child| {
    if child.kind.eq_ascii(b"ilst") {
      ilst::parse(src, child, deadline, out)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mp4::atom::encode_box;
  use crate::media_metadata::mp4::meta::ilst::{build_data_box, build_ilst_tag};
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn parses_itunes_path_through_udta() {
    let tag = build_ilst_tag(b"\xA9nam", build_data_box(1, b"Track Name"));
    let ilst = encode_box(b"ilst", &tag);
    let mut meta_payload = vec![0u8; 4]; // ISO FullBox header
    meta_payload.extend(ilst);
    let meta = encode_box(b"meta", &meta_payload);
    let udta = encode_box(b"udta", &meta);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.container.properties.title.as_deref(), Some("Track Name"));
  }

  #[test]
  fn parses_quicktime_meta_without_fullbox_header() {
    // QuickTime meta has no FullBox prefix — first child is the hdlr or ilst.
    let tag = build_ilst_tag(b"\xA9nam", build_data_box(1, b"QT Name"));
    let ilst = encode_box(b"ilst", &tag);
    let meta = encode_box(b"meta", &ilst);
    let mut s = FileSource::from_reader_for_test(Cursor::new(meta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_meta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.container.properties.title.as_deref(), Some("QT Name"));
  }

  #[test]
  fn unknown_meta_child_is_skipped() {
    let bogus = encode_box(b"junk", &[0u8; 8]);
    let mut meta_payload = vec![0u8; 4];
    meta_payload.extend(bogus);
    let meta = encode_box(b"meta", &meta_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(meta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_meta(&mut s, &h, &dl(), &mut m).unwrap();
    assert!(m.container.properties.title.is_none());
  }

  // ---- PARSER-144: direct udta/©nam title ------------------------------

  #[test]
  fn direct_udta_copyright_name_sets_title() {
    let mut nam_payload = vec![0u8; 4]; // 2-byte length + 2-byte language
    nam_payload.extend_from_slice(b"Direct Title");
    let nam = encode_box(&COPYRIGHT_NAME, &nam_payload);
    let udta = encode_box(b"udta", &nam);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mov", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.container.properties.title.as_deref(), Some("Direct Title"));
  }

  #[test]
  fn empty_copyright_name_atom_ignored() {
    let nam = encode_box(&COPYRIGHT_NAME, &[0u8; 4]);
    let udta = encode_box(b"udta", &nam);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mov", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert!(m.container.properties.title.is_none());
  }

  // ---- PARSER-143: Nero chpl chapters ----------------------------------

  fn build_chpl(chapters: &[(u64, &str)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(1); // version
    p.extend_from_slice(&[0u8; 3]); // flags
    p.extend_from_slice(&[0u8; 4]); // reserved
    p.push(chapters.len() as u8);
    for (ts, name) in chapters {
      p.extend_from_slice(&ts.to_be_bytes());
      p.push(name.len() as u8);
      p.extend_from_slice(name.as_bytes());
    }
    p
  }

  #[test]
  fn nero_chpl_sets_chapter_count() {
    let chpl = encode_box(
      b"chpl",
      &build_chpl(&[(0, "Intro"), (600_000_000, "Part 2"), (1_200_000_000, "End")]),
    );
    let udta = encode_box(b"udta", &chpl);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.chapters.num_entries, 3);
    assert_eq!(m.chapters.num_editions, 1);
  }

  #[test]
  fn nero_chpl_with_zero_count_adds_nothing() {
    let chpl = encode_box(b"chpl", &build_chpl(&[]));
    let udta = encode_box(b"udta", &chpl);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.chapters.num_entries, 0);
  }

  #[test]
  fn nero_chpl_truncated_counts_only_present_entries() {
    // Declares 3 chapters but the payload only carries 1 complete entry.
    let mut p = build_chpl(&[(0, "Only")]);
    p[8] = 3; // overstate the count
    let chpl = encode_box(b"chpl", &p);
    let udta = encode_box(b"udta", &chpl);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert_eq!(m.chapters.num_entries, 1);
  }

  #[test]
  fn udta_with_no_meta_is_a_noop() {
    let other = encode_box(b"xxxx", &[]);
    let udta = encode_box(b"udta", &other);
    let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
    assert!(m.container.properties.title.is_none());
  }
}
