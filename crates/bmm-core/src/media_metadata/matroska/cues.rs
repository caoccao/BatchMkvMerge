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

//! Cues parser.  Port of `r_matroska.cpp::handle_cues` (lines 1072-1121).
//!
//! Header-only scope: we walk every `CuePoint` → `CueTrackPositions` →
//! `CueTrack` and tally one index entry per track number, exactly as
//! mkvtoolnix increments `kax_track_t::num_cue_points`.  The result lands on
//! [`CommonTrackProperties::num_index_entries`] (identified as
//! `num_index_entries`).  The cluster offsets the cues reference are never
//! followed — we only need the per-track counts.

use std::collections::HashMap;

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
  // Tally index entries per CueTrack value (track number).
  let mut counts: HashMap<u64, u64> = HashMap::new();
  ebml::walk_children(src, parent, "matroska::cues", deadline, |src, child| {
    if child.id != ids::CUE_POINT {
      return Ok(ChildAction::Skip);
    }
    count_cue_point(src, child, deadline, &mut counts)?;
    Ok(ChildAction::Consumed)
  })?;

  apply_counts(out, &counts);
  Ok(())
}

fn count_cue_point(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  counts: &mut HashMap<u64, u64>,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::cue_point", deadline, |src, child| {
    if child.id != ids::CUE_TRACK_POSITIONS {
      return Ok(ChildAction::Skip);
    }
    let mut cue_track: Option<u64> = None;
    ebml::walk_children(src, child, "matroska::cue_track_positions", deadline, |src, leaf| {
      if leaf.id == ids::CUE_TRACK {
        cue_track = Some(ebml::read_uint(src, leaf)?);
        Ok(ChildAction::Consumed)
      } else {
        Ok(ChildAction::Skip)
      }
    })?;
    if let Some(track_number) = cue_track {
      *counts.entry(track_number).or_insert(0) += 1;
    }
    Ok(ChildAction::Consumed)
  })
}

/// Apply the tallied per-track-number counts onto matching tracks.  Tracks
/// referenced by a cue but already filtered out are ignored, matching
/// mkvtoolnix's `tracks_by_number.find(...) != not_found` guard.
fn apply_counts(out: &mut MediaMetadata, counts: &HashMap<u64, u64>) {
  for track in &mut out.tracks {
    if let Some(number) = track.properties.common.number {
      if let Some(count) = counts.get(&number) {
        track.properties.common.num_index_entries = Some(*count);
      }
    }
  }
}

/// mkvtoolnix always reports `num_index_entries` for every track (it uses
/// `info.set`, defaulting to a zero `num_cue_points`).  Default any track that
/// no cue referenced — and any track in a file without a Cues element — to 0
/// so native output matches.
pub fn default_missing_counts(out: &mut MediaMetadata) {
  for track in &mut out.tracks {
    if track.properties.common.num_index_entries.is_none() {
      track.properties.common.num_index_entries = Some(0);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint};
  use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
  use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn track_with_number(number: u64) -> Track {
    Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_VP9".to_owned(),
        name: None,
        codec_private: None,
      },
      properties: TrackProperties {
        common: CommonTrackProperties {
          number: Some(number),
          ..CommonTrackProperties::default()
        },
        ..TrackProperties::default()
      },
    }
  }

  fn cue_track_positions(track_number: u64, cluster_pos: u64) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(encode_element_uint(ids::CUE_TRACK, 1, track_number));
    p.extend(encode_element_uint(ids::CUE_CLUSTER_POSITION, 1, cluster_pos));
    encode_element(ids::CUE_TRACK_POSITIONS, 1, &p)
  }

  fn cue_point(time: u64, positions: Vec<Vec<u8>>) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(encode_element_uint(ids::CUE_TIME, 1, time));
    for pos in positions {
      p.extend(pos);
    }
    encode_element(ids::CUE_POINT, 1, &p)
  }

  fn parse_cues(points: Vec<Vec<u8>>, tracks: Vec<Track>) -> MediaMetadata {
    let mut payload = Vec::new();
    for point in points {
      payload.extend(point);
    }
    let bytes = encode_element(ids::CUES, 4, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    let mut out = MediaMetadata::new("clip.mkv", 0);
    out.tracks = tracks;
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    out
  }

  #[test]
  fn counts_cue_points_per_track() {
    let points = vec![
      cue_point(0, vec![cue_track_positions(1, 100)]),
      cue_point(1000, vec![cue_track_positions(1, 200)]),
      cue_point(2000, vec![cue_track_positions(1, 300)]),
    ];
    let m = parse_cues(points, vec![track_with_number(1)]);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(3));
  }

  #[test]
  fn counts_each_track_in_a_multi_track_cue_point() {
    let points = vec![
      cue_point(0, vec![cue_track_positions(1, 100), cue_track_positions(2, 100)]),
      cue_point(1000, vec![cue_track_positions(1, 200)]),
    ];
    let m = parse_cues(points, vec![track_with_number(1), track_with_number(2)]);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(2));
    assert_eq!(m.tracks[1].properties.common.num_index_entries, Some(1));
  }

  #[test]
  fn cue_for_unknown_track_number_is_ignored() {
    // Track 1 exists, the cue points at track 7 → no track updated, no panic.
    let points = vec![cue_point(0, vec![cue_track_positions(7, 100)])];
    let m = parse_cues(points, vec![track_with_number(1)]);
    assert!(m.tracks[0].properties.common.num_index_entries.is_none());
  }

  #[test]
  fn default_missing_counts_fills_zero() {
    let mut m = MediaMetadata::new("clip.mkv", 0);
    m.tracks.push(track_with_number(1));
    default_missing_counts(&mut m);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(0));
  }

  #[test]
  fn default_missing_counts_preserves_existing() {
    let mut m = MediaMetadata::new("clip.mkv", 0);
    let mut t = track_with_number(1);
    t.properties.common.num_index_entries = Some(42);
    m.tracks.push(t);
    default_missing_counts(&mut m);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(42));
  }
}
