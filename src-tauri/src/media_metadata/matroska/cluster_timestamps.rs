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

//! Minimum per-track timestamp discovery.  Port of
//! `r_matroska.cpp::determine_minimum_timestamps` (lines 2753-2833).
//!
//! Starting at the first cluster we read block headers — `SimpleBlock` and
//! the `Block` inside a `BlockGroup` — decoding only the track-number VINT and
//! the signed 16-bit cluster-relative timestamp (frame payloads are skipped).
//! The lowest global timestamp seen per track lands on
//! [`CommonTrackProperties::minimum_timestamp_ns`].
//!
//! The walk is bounded exactly like mkvtoolnix's:
//! - stop once 10 s of content (`PROBE_TIME_LIMIT_NS`) has been observed, and
//! - keep a video track active until 1 s (`VIDEO_TIME_LIMIT_NS`) past its
//!   recorded minimum so out-of-order B-frames can still lower it, while
//!   non-video tracks resolve on their first block.
//!
//! The configured parse deadline caps the work regardless.

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::io::varint::{self, VintKind};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::track::TrackType;

use super::ebml::{self, ElementHeader};
use super::ids;

const PROBE_TIME_LIMIT_NS: i64 = 10_000_000_000;
const VIDEO_TIME_LIMIT_NS: i64 = 1_000_000_000;
/// Hard cap on blocks inspected so a degenerate file with non-advancing
/// timestamps cannot loop until the deadline (which still guards us too).
const MAX_BLOCKS: u64 = 2_000_000;

/// Walk the opening clusters and record each track's minimum global timestamp.
/// Best-effort: any I/O error simply ends the scan with whatever was found.
pub fn determine_minimum_timestamps(
  src: &mut FileSource,
  segment: &ElementHeader,
  first_cluster_pos: u64,
  timestamp_scale: u64,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) {
  let mut state = State::new(out, timestamp_scale);
  if state.active.is_empty() {
    return;
  }
  let segment_end = segment.end();
  let stream_end = src.length();
  let _ = walk_clusters(src, first_cluster_pos, segment_end, stream_end, deadline, &mut state);
  state.apply(out);
}

struct State {
  /// track number → is-video flag for every track still being probed.
  active: HashMap<u64, bool>,
  /// track number → lowest global timestamp (ns) seen so far.
  min_by_track: HashMap<u64, i64>,
  timestamp_scale: i64,
  first_ts: Option<i64>,
  blocks_seen: u64,
  done: bool,
}

impl State {
  fn new(out: &MediaMetadata, timestamp_scale: u64) -> Self {
    let mut active = HashMap::new();
    for track in &out.tracks {
      if let Some(number) = track.properties.common.number {
        active.insert(number, track.track_type == TrackType::Video);
      }
    }
    Self {
      active,
      min_by_track: HashMap::new(),
      timestamp_scale: timestamp_scale.max(1) as i64,
      first_ts: None,
      blocks_seen: 0,
      done: false,
    }
  }

  /// Fold one block's (track_number, cluster-relative timestamp) into the
  /// running minimums and update the active set.  Mirrors the body of
  /// mkvtoolnix's per-element loop.
  fn observe(&mut self, cluster_ts: i64, track_number: u64, relative_ts: i64) {
    let last_ts = (cluster_ts + relative_ts) * self.timestamp_scale;
    if self.first_ts.is_none() {
      self.first_ts = Some(last_ts);
    }
    if last_ts - self.first_ts.unwrap() >= PROBE_TIME_LIMIT_NS {
      self.done = true;
      return;
    }
    if !self.active.contains_key(&track_number) {
      return;
    }
    let recorded = self.min_by_track.entry(track_number).or_insert(last_ts);
    if last_ts < *recorded {
      *recorded = last_ts;
    }
    let recorded = *recorded;
    let is_video = self.active[&track_number];
    if is_video && (last_ts - recorded) < VIDEO_TIME_LIMIT_NS {
      return;
    }
    self.active.remove(&track_number);
    if self.active.is_empty() {
      self.done = true;
    }
  }

  fn apply(&self, out: &mut MediaMetadata) {
    for track in &mut out.tracks {
      if let Some(number) = track.properties.common.number {
        if let Some(min) = self.min_by_track.get(&number) {
          track.properties.common.minimum_timestamp_ns = Some(*min);
        }
      }
    }
  }
}

fn walk_clusters(
  src: &mut FileSource,
  first_cluster_pos: u64,
  segment_end: Option<u64>,
  stream_end: Option<u64>,
  deadline: &Deadline,
  state: &mut State,
) -> Result<(), crate::media_metadata::error::ParseError> {
  let mut pos = first_cluster_pos;
  loop {
    if state.done {
      break;
    }
    deadline.check("matroska::minimum_timestamps")?;
    if reached_end(pos, segment_end, stream_end) {
      break;
    }
    src.seek_to(pos)?;
    let header = match ebml::read_element_header(src) {
      Ok(h) => h,
      Err(_) => break,
    };
    if header.id == ids::CLUSTER {
      walk_cluster(src, &header, stream_end, deadline, state)?;
    }
    // Advance to the next L1 element; a missing size means we cannot continue.
    match header.end() {
      Some(end) if end > pos => pos = end,
      _ => break,
    }
  }
  Ok(())
}

fn walk_cluster(
  src: &mut FileSource,
  cluster: &ElementHeader,
  stream_end: Option<u64>,
  deadline: &Deadline,
  state: &mut State,
) -> Result<(), crate::media_metadata::error::ParseError> {
  let cluster_end = cluster.end();
  let mut cluster_ts: i64 = 0;
  let mut pos = cluster.payload_start();
  loop {
    if state.done {
      break;
    }
    if reached_end(pos, cluster_end, stream_end) {
      break;
    }
    deadline.check("matroska::minimum_timestamps")?;
    src.seek_to(pos)?;
    let child = match ebml::read_element_header(src) {
      Ok(h) => h,
      Err(_) => break,
    };
    // A known L1 element here means an unknown-size cluster has ended.
    if ebml::is_segment_level_1(child.id) && child.id != ids::VOID && child.id != ids::CRC32 {
      break;
    }
    match child.id {
      ids::CLUSTER_TIMESTAMP => {
        cluster_ts = ebml::read_uint(src, &child).unwrap_or(0) as i64;
      }
      ids::CLUSTER_SIMPLE_BLOCK => {
        observe_block(src, child.payload_start(), cluster_ts, state)?;
      }
      ids::CLUSTER_BLOCK_GROUP => {
        observe_block_group(src, &child, stream_end, cluster_ts, deadline, state)?;
      }
      _ => {}
    }
    match child.end() {
      Some(end) if end > pos => pos = end,
      _ => break,
    }
  }
  Ok(())
}

fn observe_block_group(
  src: &mut FileSource,
  group: &ElementHeader,
  stream_end: Option<u64>,
  cluster_ts: i64,
  deadline: &Deadline,
  state: &mut State,
) -> Result<(), crate::media_metadata::error::ParseError> {
  let group_end = group.end();
  let mut pos = group.payload_start();
  loop {
    if reached_end(pos, group_end, stream_end) {
      break;
    }
    deadline.check("matroska::minimum_timestamps")?;
    src.seek_to(pos)?;
    let child = match ebml::read_element_header(src) {
      Ok(h) => h,
      Err(_) => break,
    };
    if child.id == ids::CLUSTER_BLOCK {
      observe_block(src, child.payload_start(), cluster_ts, state)?;
    }
    match child.end() {
      Some(end) if end > pos => pos = end,
      _ => break,
    }
  }
  Ok(())
}

/// Decode the track-number VINT + signed 16-bit relative timestamp at the
/// start of a block payload, then fold it into the running state.
fn observe_block(
  src: &mut FileSource,
  payload_start: u64,
  cluster_ts: i64,
  state: &mut State,
) -> Result<(), crate::media_metadata::error::ParseError> {
  state.blocks_seen += 1;
  if state.blocks_seen > MAX_BLOCKS {
    state.done = true;
    return Ok(());
  }
  src.seek_to(payload_start)?;
  let track_number = match varint::read(src, VintKind::Stripped) {
    Ok(v) => v.value,
    Err(_) => return Ok(()),
  };
  let relative_ts = match src.read_u16_be() {
    Ok(v) => v as i16 as i64,
    Err(_) => return Ok(()),
  };
  state.observe(cluster_ts, track_number, relative_ts);
  Ok(())
}

fn reached_end(pos: u64, parent_end: Option<u64>, stream_end: Option<u64>) -> bool {
  if let Some(end) = parent_end {
    if pos >= end {
      return true;
    }
  }
  if let Some(end) = stream_end {
    if pos >= end {
      return true;
    }
  }
  false
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_uint};
  use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties};
  use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn track(number: u64, track_type: TrackType) -> Track {
    Track {
      id: 0,
      track_type,
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

  /// Build a SimpleBlock payload: track-number VINT (1-byte) + s16 rel ts + flags.
  fn simple_block(track_number: u8, relative_ts: i16) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(0x80 | track_number); // 1-byte VINT marker
    payload.extend(relative_ts.to_be_bytes());
    payload.push(0x00); // flags
    payload.extend([0u8; 4]); // dummy frame data
    encode_element(ids::CLUSTER_SIMPLE_BLOCK, 1, &payload)
  }

  fn cluster(timestamp: u64, blocks: Vec<Vec<u8>>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::CLUSTER_TIMESTAMP, 1, timestamp));
    for b in blocks {
      payload.extend(b);
    }
    encode_element(ids::CLUSTER, 4, &payload)
  }

  fn run(bytes: Vec<u8>, tracks: Vec<Track>, tc_scale: u64) -> MediaMetadata {
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    // Treat the whole buffer as one segment payload starting at 0.
    let seg = ElementHeader {
      start: 0,
      id: ids::SEGMENT,
      size: s.length(),
      header_len: 0,
    };
    let mut out = MediaMetadata::new("clip.mkv", 0);
    out.tracks = tracks;
    out.container.properties.timestamp_scale = Some(tc_scale);
    determine_minimum_timestamps(&mut s, &seg, 0, tc_scale, &no_deadline(), &mut out);
    out
  }

  #[test]
  fn records_minimum_for_audio_track_first_block() {
    // Audio track resolves on its first block.
    let bytes = cluster(0, vec![simple_block(1, 40)]);
    let m = run(bytes, vec![track(1, TrackType::Audio)], 1_000_000);
    // (0 + 40) * 1_000_000 = 40_000_000 ns
    assert_eq!(m.tracks[0].properties.common.minimum_timestamp_ns, Some(40_000_000));
  }

  #[test]
  fn video_track_minimum_lowered_by_later_b_frame() {
    // First block ts=100ms, a reordered frame at ts=0 within the 1s window
    // must lower the recorded minimum.
    let blocks = vec![simple_block(1, 100), simple_block(1, 0), simple_block(1, 200)];
    let bytes = cluster(0, blocks);
    let m = run(bytes, vec![track(1, TrackType::Video)], 1_000_000);
    assert_eq!(m.tracks[0].properties.common.minimum_timestamp_ns, Some(0));
  }

  #[test]
  fn negative_relative_timestamp_yields_negative_minimum() {
    let bytes = cluster(0, vec![simple_block(1, -25)]);
    let m = run(bytes, vec![track(1, TrackType::Audio)], 1_000_000);
    assert_eq!(m.tracks[0].properties.common.minimum_timestamp_ns, Some(-25_000_000));
  }

  #[test]
  fn block_group_blocks_are_counted() {
    let block = {
      let mut payload = Vec::new();
      payload.push(0x82); // track number 2
      payload.extend((10i16).to_be_bytes());
      payload.push(0x00);
      payload.extend([0u8; 2]);
      encode_element(ids::CLUSTER_BLOCK, 1, &payload)
    };
    let group = encode_element(ids::CLUSTER_BLOCK_GROUP, 1, &block);
    let mut cpayload = Vec::new();
    cpayload.extend(encode_element_uint(ids::CLUSTER_TIMESTAMP, 1, 5));
    cpayload.extend(group);
    let bytes = encode_element(ids::CLUSTER, 4, &cpayload);
    let m = run(bytes, vec![track(2, TrackType::Audio)], 1_000_000);
    // (5 + 10) * 1_000_000
    assert_eq!(m.tracks[0].properties.common.minimum_timestamp_ns, Some(15_000_000));
  }

  #[test]
  fn cluster_timestamp_added_to_relative() {
    let bytes = cluster(1000, vec![simple_block(1, 5)]);
    let m = run(bytes, vec![track(1, TrackType::Audio)], 1_000_000);
    assert_eq!(m.tracks[0].properties.common.minimum_timestamp_ns, Some(1_005_000_000));
  }

  #[test]
  fn track_without_blocks_keeps_none() {
    let bytes = cluster(0, vec![simple_block(1, 0)]);
    // Track 9 has no blocks → minimum stays None.
    let m = run(bytes, vec![track(9, TrackType::Audio)], 1_000_000);
    assert!(m.tracks[0].properties.common.minimum_timestamp_ns.is_none());
  }

  #[test]
  fn no_tracks_is_noop() {
    let bytes = cluster(0, vec![simple_block(1, 0)]);
    let m = run(bytes, vec![], 1_000_000);
    assert!(m.tracks.is_empty());
  }
}
