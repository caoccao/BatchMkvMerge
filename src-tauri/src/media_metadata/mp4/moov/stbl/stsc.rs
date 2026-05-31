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

//! `stsc` (sample-to-chunk) box.  PARSER-183 needs the chunk-map so the bounded
//! first-bytes read can reconstruct the FIRST few samples' file offsets across
//! MULTIPLE chunks, mirroring mkvtoolnix's `chunkmap_table` / `update_tables`
//! (`r_qtmp4.cpp:2544-2564`).
//!
//! Layout: FullBox(4) + entry_count(4) + entry_count × { first_chunk(4)
//! samples_per_chunk(4) sample_description_id(4) }.  `first_chunk` is 1-based.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

/// One `stsc` entry.  `first_chunk` is 1-based exactly as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StscEntry {
  pub first_chunk: u32,
  pub samples_per_chunk: u32,
}

/// Validate the declared `stsc` entry count exactly, then retain up to
/// `max_entries` entries for the bounded first-sample index.
pub fn parse(src: &mut FileSource, header: &BoxHeader, max_entries: usize) -> Result<Vec<StscEntry>, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.payload_start(),
      reason: "truncated stsc atom".to_string(),
    });
  }
  src.skip(4)?; // FullBox header
  let entry_count = src.read_u32_be()?;
  if entry_count == 0 {
    return Ok(Vec::new());
  }
  let available = payload.saturating_sub(8) / 12;
  if u64::from(entry_count) > available {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.payload_start(),
      reason: "stsc entry_count overruns atom payload".to_string(),
    });
  }
  // Cap how many entries we retain: enough chunk-map runs for a bounded read.
  let to_read = (entry_count as usize).min(max_entries);
  let mut entries = Vec::with_capacity(to_read);
  for _ in 0..to_read {
    let first_chunk = src.read_u32_be()?;
    let samples_per_chunk = src.read_u32_be()?;
    let _sample_description_id = src.read_u32_be()?;
    entries.push(StscEntry {
      first_chunk,
      samples_per_chunk,
    });
  }
  Ok(entries)
}

#[cfg(test)]
pub(crate) fn build_stsc_payload(entries: &[(u32, u32, u32)]) -> Vec<u8> {
  let mut p = vec![0u8; 4]; // FullBox header
  p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
  for (first_chunk, samples_per_chunk, sample_desc) in entries {
    p.extend_from_slice(&first_chunk.to_be_bytes());
    p.extend_from_slice(&samples_per_chunk.to_be_bytes());
    p.extend_from_slice(&sample_desc.to_be_bytes());
  }
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::{self, encode_box};
  use std::io::Cursor;

  fn read(payload: Vec<u8>) -> (BoxHeader, FileSource) {
    let bytes = encode_box(b"stsc", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    (h, s)
  }

  #[test]
  fn reads_all_entries() {
    let (h, mut s) = read(build_stsc_payload(&[(1, 4, 1), (3, 2, 1)]));
    let entries = parse(&mut s, &h, 64).unwrap();
    assert_eq!(
      entries,
      vec![
        StscEntry {
          first_chunk: 1,
          samples_per_chunk: 4
        },
        StscEntry {
          first_chunk: 3,
          samples_per_chunk: 2
        },
      ]
    );
  }

  #[test]
  fn caps_at_max_entries() {
    let (h, mut s) = read(build_stsc_payload(&[(1, 4, 1), (3, 2, 1), (5, 1, 1)]));
    let entries = parse(&mut s, &h, 2).unwrap();
    assert_eq!(entries.len(), 2);
  }

  #[test]
  fn empty_yields_empty_and_truncated_is_malformed() {
    let (h, mut s) = read(build_stsc_payload(&[]));
    assert!(parse(&mut s, &h, 64).unwrap().is_empty());
    let (h2, mut s2) = read(vec![0u8; 4]); // < 8 bytes
    assert!(matches!(parse(&mut s2, &h2, 64), Err(ParseError::Malformed { .. })));
  }

  #[test]
  fn declared_count_overrun_is_malformed() {
    // entry_count claims 999 but payload only carries one entry.
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&999u32.to_be_bytes());
    p.extend_from_slice(&1u32.to_be_bytes());
    p.extend_from_slice(&4u32.to_be_bytes());
    p.extend_from_slice(&1u32.to_be_bytes());
    let (h, mut s) = read(p);
    assert!(matches!(parse(&mut s, &h, 64), Err(ParseError::Malformed { .. })));
  }
}
