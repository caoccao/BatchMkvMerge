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

//! `stbl` (sample table) — wraps `stsd` (sample descriptions), `stts`
//! (decoding time-to-sample), `stsz` / `stz2` (sample sizes), and the chunk
//! offset tables `stco` / `co64`.
//!
//! PARSER-145 needs the per-track index-entry count (the `stsz` sample count).
//! PARSER-177 additionally needs to locate the FIRST sample so the reader can
//! perform a bounded first-sample verification read (mirroring
//! `r_qtmp4.cpp:2881-2906 read_first_bytes`): we capture `first_sample_size`
//! (the `stsz` first entry, or the fixed sample size when non-zero) and
//! `first_sample_file_offset` (= `stco`/`co64` chunk_offset[0]).  We do NOT
//! build a full index — only sample 0 is located.  The remaining sub-boxes
//! (`stsc`, `stss`, `ctts`) stay skipped.

pub mod stsd;
pub mod stts;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::trak::TrackBuilder;

pub fn parse(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::stbl", deadline, |src, child| match &child.kind.0 {
    b"stsd" => {
      stsd::parse(src, child, deadline, builder)?;
      Ok(ChildAction::Consumed)
    }
    b"stts" => {
      let s = stts::parse(src, child)?;
      builder.stts_first_sample_count = Some(s.first_sample_count);
      builder.stts_first_sample_delta = Some(s.first_sample_delta);
      Ok(ChildAction::Consumed)
    }
    // PARSER-145: `stsz` / `stz2` carry the sample count, which is the number
    // of index entries mkvtoolnix reports for a non-fragmented track. Both
    // layouts place a 32-bit sample_count at payload offset 8 (after the
    // FullBox header + 4 bytes of sample_size / field_size).
    // PARSER-177: `stsz` also yields the FIRST sample's size.
    b"stsz" => {
      let (count, first_size) = read_stsz(src, child)?;
      builder.sample_count = count;
      if first_size.is_some() {
        builder.first_sample_size = first_size;
      }
      Ok(ChildAction::Consumed)
    }
    b"stz2" => {
      builder.sample_count = read_stz2_sample_count(src, child)?;
      Ok(ChildAction::Consumed)
    }
    // PARSER-177: chunk-offset tables — capture chunk_offset[0] as the first
    // sample's file offset.  Sample 0 always lives at the start of chunk 0
    // (stsc's first run), so the chunk offset is the byte offset of sample 0.
    b"stco" => {
      if let Some(off) = read_first_chunk_offset_32(src, child)? {
        builder.first_sample_file_offset = Some(off);
      }
      Ok(ChildAction::Consumed)
    }
    b"co64" => {
      if let Some(off) = read_first_chunk_offset_64(src, child)? {
        builder.first_sample_file_offset = Some(off);
      }
      Ok(ChildAction::Consumed)
    }
    _ => Ok(ChildAction::Skip),
  })
}

/// Read the `stsz` sample count and the first sample's size.  Layout:
/// FullBox(4) + sample_size(4) + sample_count(4) + [if sample_size==0:
/// sample_count × u32].  When `sample_size != 0` it is the fixed size for
/// every sample (so it is the first sample's size too); otherwise the first
/// per-sample u32 is the first sample's size.  Returns `(None, None)` for a
/// truncated payload rather than failing the parse.
fn read_stsz(src: &mut FileSource, header: &BoxHeader) -> Result<(Option<u32>, Option<u64>), ParseError> {
  if header.payload_size().unwrap_or(0) < 12 {
    return Ok((None, None));
  }
  src.skip(4)?; // FullBox header
  let sample_size = src.read_u32_be()?;
  let sample_count = src.read_u32_be()?;
  let first_size = if sample_size != 0 {
    Some(sample_size as u64)
  } else if header.payload_size().unwrap_or(0) >= 16 && sample_count > 0 {
    Some(src.read_u32_be()? as u64)
  } else {
    None
  };
  Ok((Some(sample_count), first_size))
}

/// Read the 32-bit sample count from an `stz2` box.  The packed per-sample
/// field widths make extracting the first sample's size awkward, so we only
/// surface the count (the read budget is capped regardless).  Returns `None`
/// for a truncated payload.
fn read_stz2_sample_count(src: &mut FileSource, header: &BoxHeader) -> Result<Option<u32>, ParseError> {
  if header.payload_size().unwrap_or(0) < 12 {
    return Ok(None);
  }
  src.skip(8)?; // FullBox header (4) + reserved(3) + field_size(1)
  Ok(Some(src.read_u32_be()?))
}

/// Read `chunk_offset[0]` from an `stco` box: FullBox(4) + entry_count(4) +
/// entry_count × u32.  Returns `None` when empty / truncated.
fn read_first_chunk_offset_32(src: &mut FileSource, header: &BoxHeader) -> Result<Option<u64>, ParseError> {
  if header.payload_size().unwrap_or(0) < 12 {
    return Ok(None);
  }
  src.skip(4)?; // FullBox header
  let entry_count = src.read_u32_be()?;
  if entry_count == 0 {
    return Ok(None);
  }
  Ok(Some(src.read_u32_be()? as u64))
}

/// Read `chunk_offset[0]` from a `co64` box: FullBox(4) + entry_count(4) +
/// entry_count × u64.  Returns `None` when empty / truncated.
fn read_first_chunk_offset_64(src: &mut FileSource, header: &BoxHeader) -> Result<Option<u64>, ParseError> {
  if header.payload_size().unwrap_or(0) < 16 {
    return Ok(None);
  }
  src.skip(4)?; // FullBox header
  let entry_count = src.read_u32_be()?;
  if entry_count == 0 {
    return Ok(None);
  }
  Ok(Some(src.read_u64_be()?))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use crate::media_metadata::mp4::moov::stbl::stsd::{build_audio_sample_entry_v0, build_stsd_payload};
  use crate::media_metadata::mp4::moov::stbl::stts::build_stts_payload;
  use std::io::Cursor;

  #[test]
  fn parses_stsd_and_stts_into_builder() {
    let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48_000, &[]);
    let stsd = encode_box(b"stsd", &build_stsd_payload(&[entry]));
    let stts = encode_box(b"stts", &build_stts_payload(&[(60, 1000)]));
    let mut payload = stsd;
    payload.extend(stts);
    let stbl = encode_box(b"stbl", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"soun");
    let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
    parse(&mut s, &parent, &deadline, &mut b).unwrap();
    assert_eq!(b.codec_id_str.as_deref(), Some("mp4a"));
    assert_eq!(b.stts_first_sample_count, Some(60));
    assert_eq!(b.stts_first_sample_delta, Some(1000));
  }

  #[test]
  fn stsz_sample_count_recorded() {
    // stsz: version+flags(4) + sample_size(4) + sample_count(4).
    let mut stsz_payload = vec![0u8; 4];
    stsz_payload.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0 (per-sample sizes)
    stsz_payload.extend_from_slice(&512u32.to_be_bytes()); // sample_count
    let stsz = encode_box(b"stsz", &stsz_payload);
    let stbl = encode_box(b"stbl", &stsz);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
    parse(&mut s, &parent, &deadline, &mut b).unwrap();
    assert_eq!(b.sample_count, Some(512));
  }

  #[test]
  fn stz2_sample_count_recorded() {
    // stz2: version+flags(4) + reserved(3)+field_size(1) + sample_count(4).
    let mut stz2_payload = vec![0u8; 4];
    stz2_payload.extend_from_slice(&[0, 0, 0, 16]); // reserved + field_size
    stz2_payload.extend_from_slice(&99u32.to_be_bytes());
    let stz2 = encode_box(b"stz2", &stz2_payload);
    let stbl = encode_box(b"stbl", &stz2);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
    parse(&mut s, &parent, &deadline, &mut b).unwrap();
    assert_eq!(b.sample_count, Some(99));
  }

  #[test]
  fn unknown_stbl_child_skipped_silently() {
    let bogus = encode_box(b"junk", &[0u8; 4]);
    let stbl = encode_box(b"stbl", &bogus);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
    parse(&mut s, &parent, &deadline, &mut b).unwrap();
    assert!(b.stts_first_sample_count.is_none());
  }

  // ---- PARSER-177: first-sample location (stsz first size + stco/co64) -----

  fn run_stbl(children: Vec<u8>) -> TrackBuilder {
    let stbl = encode_box(b"stbl", &children);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
    parse(&mut s, &parent, &deadline, &mut b).unwrap();
    b
  }

  #[test]
  fn stsz_first_sample_size_from_per_sample_table() {
    // sample_size=0 ⇒ per-sample table; first entry is the first sample size.
    let mut p = vec![0u8; 4]; // version+flags
    p.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0
    p.extend_from_slice(&3u32.to_be_bytes()); // sample_count
    p.extend_from_slice(&777u32.to_be_bytes()); // first sample size
    let b = run_stbl(encode_box(b"stsz", &p));
    assert_eq!(b.sample_count, Some(3));
    assert_eq!(b.first_sample_size, Some(777));
  }

  #[test]
  fn stsz_fixed_sample_size_is_first_sample_size() {
    // Non-zero sample_size is the fixed size for every sample.
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&512u32.to_be_bytes()); // sample_size = 512 (fixed)
    p.extend_from_slice(&10u32.to_be_bytes()); // sample_count
    let b = run_stbl(encode_box(b"stsz", &p));
    assert_eq!(b.sample_count, Some(10));
    assert_eq!(b.first_sample_size, Some(512));
  }

  #[test]
  fn stsz_zero_sample_count_yields_no_first_size() {
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&0u32.to_be_bytes()); // sample_size = 0
    p.extend_from_slice(&0u32.to_be_bytes()); // sample_count = 0
    let b = run_stbl(encode_box(b"stsz", &p));
    assert_eq!(b.sample_count, Some(0));
    assert!(b.first_sample_size.is_none());
  }

  #[test]
  fn stsz_truncated_payload_yields_none() {
    let b = run_stbl(encode_box(b"stsz", &[0u8; 4])); // < 12 bytes
    assert!(b.sample_count.is_none());
    assert!(b.first_sample_size.is_none());
  }

  #[test]
  fn stz2_truncated_payload_yields_none() {
    let b = run_stbl(encode_box(b"stz2", &[0u8; 4]));
    assert!(b.sample_count.is_none());
  }

  #[test]
  fn stco_first_chunk_offset_recorded() {
    let mut p = vec![0u8; 4]; // version+flags
    p.extend_from_slice(&2u32.to_be_bytes()); // entry_count
    p.extend_from_slice(&4096u32.to_be_bytes()); // chunk_offset[0]
    p.extend_from_slice(&9000u32.to_be_bytes()); // chunk_offset[1] (ignored)
    let b = run_stbl(encode_box(b"stco", &p));
    assert_eq!(b.first_sample_file_offset, Some(4096));
  }

  #[test]
  fn stco_empty_or_truncated_yields_none() {
    // Empty entry_count.
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&0u32.to_be_bytes()); // entry_count = 0
    p.extend_from_slice(&1234u32.to_be_bytes());
    let b = run_stbl(encode_box(b"stco", &p));
    assert!(b.first_sample_file_offset.is_none());
    // Truncated payload (< 12 bytes).
    let b2 = run_stbl(encode_box(b"stco", &[0u8; 8]));
    assert!(b2.first_sample_file_offset.is_none());
  }

  #[test]
  fn co64_first_chunk_offset_recorded() {
    let mut p = vec![0u8; 4]; // version+flags
    p.extend_from_slice(&1u32.to_be_bytes()); // entry_count
    p.extend_from_slice(&0x1_0000_0000u64.to_be_bytes()); // 64-bit offset > 4 GiB
    let b = run_stbl(encode_box(b"co64", &p));
    assert_eq!(b.first_sample_file_offset, Some(0x1_0000_0000));
  }

  #[test]
  fn co64_empty_or_truncated_yields_none() {
    // Empty entry_count.
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&0u64.to_be_bytes());
    let b = run_stbl(encode_box(b"co64", &p));
    assert!(b.first_sample_file_offset.is_none());
    // Truncated payload (< 16 bytes).
    let b2 = run_stbl(encode_box(b"co64", &[0u8; 8]));
    assert!(b2.first_sample_file_offset.is_none());
  }
}
