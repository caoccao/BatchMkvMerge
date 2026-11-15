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

//! Fragmented-MP4 support.
//!
//! `mvex` (movie extends) sits inside `moov` and declares per-track defaults:
//!
//! ```text
//! trex {
//!   u32 track_id
//!   u32 default_sample_description_index
//!   u32 default_sample_duration
//!   u32 default_sample_size
//!   u32 default_sample_flags
//! }
//! ```
//!
//! `moof` (movie fragment) lives at the top level and contains:
//! - `mfhd { u32 sequence_number }`
//! - `traf*` — one per fragmented track:
//!   - `tfhd` — track-id + per-fragment defaults (override mvex).
//!   - `trun` — runs of samples.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use super::atom::{self, BoxHeader, ChildAction};

#[derive(Debug, Default, Clone)]
pub struct TrexDefaults {
  pub entries: Vec<TrexEntry>,
}

#[derive(Debug, Clone, Copy)]
pub struct TrexEntry {
  pub track_id: u32,
  pub default_sample_duration: u32,
  pub default_sample_size: u32,
}

impl TrexDefaults {
  pub fn default_duration_for(&self, track_id: u32) -> Option<u32> {
    self
      .entries
      .iter()
      .find(|e| e.track_id == track_id)
      .map(|e| e.default_sample_duration)
  }
}

/// Walk `moov/mvex` populating per-track defaults.
pub fn parse_mvex(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  defaults: &mut TrexDefaults,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::mvex", deadline, |src, child| {
    if !child.kind.eq_ascii(b"trex") {
      return Ok(ChildAction::Skip);
    }
    let payload = atom::read_payload(src, child, 256)?;
    if payload.len() < 4 + 4 * 5 {
      return Ok(ChildAction::Consumed);
    }
    // skip FullBox header (4)
    let body = &payload[4..];
    let track_id = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let _desc_index = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    let default_sample_duration = u32::from_be_bytes([body[8], body[9], body[10], body[11]]);
    let default_sample_size = u32::from_be_bytes([body[12], body[13], body[14], body[15]]);
    defaults.entries.push(TrexEntry {
      track_id,
      default_sample_duration,
      default_sample_size,
    });
    Ok(ChildAction::Consumed)
  })
}

#[derive(Debug, Default, Clone)]
pub struct MoofSummary {
  pub track_runs: Vec<TrackFragment>,
}

#[derive(Debug, Clone, Copy)]
pub struct TrackFragment {
  pub track_id: u32,
  pub sample_count: u32,
}

/// Walk a top-level `moof` box.  Currently returns a summary used by the
/// reader to flag `is_fragmented` and aggregate fragment sample counts.
pub fn parse_moof(src: &mut FileSource, parent: &BoxHeader, deadline: &Deadline) -> Result<MoofSummary, ParseError> {
  let mut summary = MoofSummary::default();
  atom::walk_children(src, parent, "mp4::moof", deadline, |src, child| {
    if !child.kind.eq_ascii(b"traf") {
      return Ok(ChildAction::Skip);
    }
    if let Some(frag) = parse_traf(src, child, deadline)? {
      summary.track_runs.push(frag);
    }
    Ok(ChildAction::Consumed)
  })?;
  Ok(summary)
}

fn parse_traf(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
) -> Result<Option<TrackFragment>, ParseError> {
  let mut track_id: Option<u32> = None;
  let mut sample_count: u32 = 0;
  atom::walk_children(src, parent, "mp4::traf", deadline, |src, child| match &child.kind.0 {
    b"tfhd" => {
      let payload = atom::read_payload(src, child, 128)?;
      if payload.len() >= 8 {
        track_id = Some(u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]));
      }
      Ok(ChildAction::Consumed)
    }
    b"trun" => {
      let payload = atom::read_payload(src, child, 16 * 1024)?;
      if payload.len() >= 8 {
        let count = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        sample_count = sample_count.saturating_add(count);
      }
      Ok(ChildAction::Consumed)
    }
    _ => Ok(ChildAction::Skip),
  })?;
  Ok(track_id.map(|id| TrackFragment {
    track_id: id,
    sample_count,
  }))
}

#[cfg(test)]
pub(crate) fn build_trex(track_id: u32, default_duration: u32, default_size: u32) -> Vec<u8> {
  let mut p = vec![0u8; 4]; // version+flags
  p.extend_from_slice(&track_id.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // desc_index
  p.extend_from_slice(&default_duration.to_be_bytes());
  p.extend_from_slice(&default_size.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // default flags
  crate::media_metadata::mp4::atom::encode_box(b"trex", &p)
}

#[cfg(test)]
pub(crate) fn build_tfhd(track_id: u32) -> Vec<u8> {
  let mut p = vec![0u8; 4]; // version+flags
  p.extend_from_slice(&track_id.to_be_bytes());
  crate::media_metadata::mp4::atom::encode_box(b"tfhd", &p)
}

#[cfg(test)]
pub(crate) fn build_trun(sample_count: u32) -> Vec<u8> {
  let mut p = vec![0u8; 4]; // version+flags
  p.extend_from_slice(&sample_count.to_be_bytes());
  crate::media_metadata::mp4::atom::encode_box(b"trun", &p)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn mvex_with_one_trex_populates_defaults() {
    let trex = build_trex(1, 1024, 0);
    let mvex = encode_box(b"mvex", &trex);
    let mut s = FileSource::from_reader_for_test(Cursor::new(mvex));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut d = TrexDefaults::default();
    parse_mvex(&mut s, &h, &dl(), &mut d).unwrap();
    assert_eq!(d.entries.len(), 1);
    assert_eq!(d.entries[0].track_id, 1);
    assert_eq!(d.default_duration_for(1), Some(1024));
    assert!(d.default_duration_for(99).is_none());
  }

  #[test]
  fn moof_summary_sums_sample_counts() {
    let tfhd = build_tfhd(3);
    let trun_a = build_trun(60);
    let trun_b = build_trun(30);
    let mut traf_payload = tfhd;
    traf_payload.extend(trun_a);
    traf_payload.extend(trun_b);
    let traf = encode_box(b"traf", &traf_payload);
    let moof = encode_box(b"moof", &traf);
    let mut s = FileSource::from_reader_for_test(Cursor::new(moof));
    let h = atom::read_box_header(&mut s).unwrap();
    let summary = parse_moof(&mut s, &h, &dl()).unwrap();
    assert_eq!(summary.track_runs.len(), 1);
    assert_eq!(summary.track_runs[0].track_id, 3);
    assert_eq!(summary.track_runs[0].sample_count, 90);
  }

  #[test]
  fn traf_without_tfhd_is_dropped() {
    let trun = build_trun(10);
    let traf = encode_box(b"traf", &trun);
    let moof = encode_box(b"moof", &traf);
    let mut s = FileSource::from_reader_for_test(Cursor::new(moof));
    let h = atom::read_box_header(&mut s).unwrap();
    let summary = parse_moof(&mut s, &h, &dl()).unwrap();
    assert!(summary.track_runs.is_empty());
  }

  #[test]
  fn unknown_mvex_child_is_skipped() {
    let bogus = encode_box(b"xxxx", &[0u8; 4]);
    let mvex = encode_box(b"mvex", &bogus);
    let mut s = FileSource::from_reader_for_test(Cursor::new(mvex));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut d = TrexDefaults::default();
    parse_mvex(&mut s, &h, &dl(), &mut d).unwrap();
    assert!(d.entries.is_empty());
  }

  #[test]
  fn truncated_trex_does_not_populate() {
    let trex_bad = encode_box(b"trex", &vec![0u8; 8]);
    let mvex = encode_box(b"mvex", &trex_bad);
    let mut s = FileSource::from_reader_for_test(Cursor::new(mvex));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut d = TrexDefaults::default();
    parse_mvex(&mut s, &h, &dl(), &mut d).unwrap();
    assert!(d.entries.is_empty());
  }
}
