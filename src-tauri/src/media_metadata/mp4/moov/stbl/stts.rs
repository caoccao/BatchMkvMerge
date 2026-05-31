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

//! `stts` (decoding time-to-sample) box.  We only read the first entry per
//! mkvmerge's identification behaviour — it gives us the most common sample
//! duration, which feeds `VideoTrackProperties.default_duration_ns`.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone, Copy)]
pub struct SttsSummary {
  pub first_sample_count: u32,
  pub first_sample_delta: u32,
}

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<SttsSummary, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("stts payload {payload} bytes is too small"),
    });
  }
  // 1 + 3 + 4 = 8 byte header
  src.skip(4)?; // version + flags
  let entry_count = src.read_u32_be()?;
  if entry_count == 0 {
    return Ok(SttsSummary {
      first_sample_count: 0,
      first_sample_delta: 0,
    });
  }
  let needed = 8u64.saturating_add(entry_count as u64 * 8);
  if needed > payload {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("stts declares {entry_count} entries needing {needed} bytes but payload has {payload}"),
    });
  }
  let first_sample_count = src.read_u32_be()?;
  let first_sample_delta = src.read_u32_be()?;
  Ok(SttsSummary {
    first_sample_count,
    first_sample_delta,
  })
}

#[cfg(test)]
pub(crate) fn build_stts_payload(entries: &[(u32, u32)]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 4]); // version + flags
  p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
  for (count, delta) in entries {
    p.extend_from_slice(&count.to_be_bytes());
    p.extend_from_slice(&delta.to_be_bytes());
  }
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::{self, encode_box};
  use std::io::Cursor;

  fn read(payload: Vec<u8>) -> (BoxHeader, FileSource) {
    let bytes = encode_box(b"stts", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    (h, s)
  }

  #[test]
  fn reads_first_entry() {
    let (h, mut s) = read(build_stts_payload(&[(60, 100), (30, 200)]));
    let summary = parse(&mut s, &h).unwrap();
    assert_eq!(summary.first_sample_count, 60);
    assert_eq!(summary.first_sample_delta, 100);
  }

  #[test]
  fn zero_entries_returns_zero_summary() {
    let (h, mut s) = read(build_stts_payload(&[]));
    let summary = parse(&mut s, &h).unwrap();
    assert_eq!(summary.first_sample_count, 0);
    assert_eq!(summary.first_sample_delta, 0);
  }

  #[test]
  fn rejects_truncated_payload() {
    let (h, mut s) = read(vec![0u8; 4]);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_entry_count_overflowing_payload() {
    let mut p = vec![0u8; 4];
    p.extend_from_slice(&999u32.to_be_bytes());
    let (h, mut s) = read(p);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
