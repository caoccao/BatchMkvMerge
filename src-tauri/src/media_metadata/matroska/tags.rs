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

//! Tags parser.  Port of `r_matroska.cpp::handle_tags` (lines 979-1055).
//!
//! Each `Tag` element carries an optional `Targets` block; when that block
//! references a TrackUID we bucket the tag under that track (mkvtoolnix
//! stores `track->tags` per-track in `kax_track_t`), otherwise the tag goes
//! to the global tags list.
//!
//! We surface each `SimpleTag` (name/value/language) as a flat
//! [`TagEntry`]; nested SimpleTag children are flattened in source order so
//! the resulting list is easy to consume from the frontend.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::tag::TagEntry;

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::tags", deadline, |src, child| {
    if child.id != ids::TAG {
      return Ok(ChildAction::Skip);
    }
    let parsed = read_tag(src, child, deadline)?;
    route_tag(out, parsed);
    Ok(ChildAction::Consumed)
  })?;

  // Derived per_track_count for the top-level bundle.
  out.tags.per_track_count = out.tracks.iter().map(|t| t.properties.tags.len() as u32).sum();
  Ok(())
}

#[derive(Debug, Default)]
struct ParsedTag {
  track_uid: Option<u64>,
  entries: Vec<TagEntry>,
}

fn read_tag(src: &mut FileSource, parent: &ElementHeader, deadline: &Deadline) -> Result<ParsedTag, ParseError> {
  let mut tag = ParsedTag::default();
  ebml::walk_children(src, parent, "matroska::tag", deadline, |src, child| match child.id {
    ids::TAG_TARGETS => {
      ebml::walk_children(src, child, "matroska::tag_targets", deadline, |src, t| {
        if t.id == ids::TAG_TRACK_UID {
          tag.track_uid = Some(ebml::read_uint(src, t)?);
          Ok(ChildAction::Consumed)
        } else {
          Ok(ChildAction::Skip)
        }
      })?;
      Ok(ChildAction::Consumed)
    }
    ids::TAG_SIMPLE => {
      read_simple_tag(src, child, deadline, &mut tag.entries)?;
      Ok(ChildAction::Consumed)
    }
    _ => Ok(ChildAction::Skip),
  })?;
  Ok(tag)
}

fn read_simple_tag(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut Vec<TagEntry>,
) -> Result<(), ParseError> {
  let mut name: Option<String> = None;
  let mut value: Option<String> = None;
  let mut language: Option<String> = None;
  let mut language_ietf: Option<String> = None;
  let mut nested: Vec<TagEntry> = Vec::new();
  ebml::walk_children(
    src,
    parent,
    "matroska::simple_tag",
    deadline,
    |src, child| match child.id {
      ids::TAG_NAME => {
        name = Some(ebml::read_string(src, child, deadline.max_element_size())?);
        Ok(ChildAction::Consumed)
      }
      ids::TAG_STRING => {
        value = Some(ebml::read_string(src, child, deadline.max_element_size())?);
        Ok(ChildAction::Consumed)
      }
      ids::TAG_LANGUAGE => {
        language = Some(ebml::read_string(src, child, 64)?);
        Ok(ChildAction::Consumed)
      }
      ids::TAG_LANGUAGE_IETF => {
        language_ietf = Some(ebml::read_string(src, child, 64)?);
        Ok(ChildAction::Consumed)
      }
      ids::TAG_SIMPLE => {
        read_simple_tag(src, child, deadline, &mut nested)?;
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    },
  )?;
  if let (Some(name), Some(value)) = (name, value) {
    out.push(TagEntry {
      name,
      value,
      language: language_ietf.or(language),
    });
  }
  out.extend(nested);
  Ok(())
}

fn route_tag(out: &mut MediaMetadata, parsed: ParsedTag) {
  let ParsedTag { track_uid, entries } = parsed;
  if entries.is_empty() {
    return;
  }
  match track_uid {
    Some(uid_hex_target) => {
      // Convert the numeric UID to the same hex form we stored on
      // the track's CommonTrackProperties.uid_hex.
      let target_hex = format!("{:016x}", uid_hex_target);
      for track in &mut out.tracks {
        if track.properties.common.uid_hex.as_deref() == Some(&target_hex) {
          track.properties.tags.extend(entries);
          return;
        }
      }
      // PARSER-139: a tag carrying a KaxTagTrackUID is non-global. When the
      // target track is missing or was filtered out, mkvtoolnix deletes the
      // tag (r_matroska.cpp:1018-1052) rather than promoting it to a
      // file-wide tag — dropping it preserves that meaning.
    }
    None => {
      out.tags.global.extend(entries);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_string, encode_element_uint};
  use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
  use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_simple_tag(name: &str, value: &str, language: Option<&str>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::TAG_NAME, 2, name));
    payload.extend(encode_element_string(ids::TAG_STRING, 2, value));
    if let Some(l) = language {
      payload.extend(encode_element_string(ids::TAG_LANGUAGE, 2, l));
    }
    encode_element(ids::TAG_SIMPLE, 2, &payload)
  }

  fn build_tag(track_uid: Option<u64>, simples: Vec<Vec<u8>>) -> Vec<u8> {
    let mut payload = Vec::new();
    if let Some(uid) = track_uid {
      let target = encode_element_uint(ids::TAG_TRACK_UID, 2, uid);
      let targets = encode_element(ids::TAG_TARGETS, 2, &target);
      payload.extend(targets);
    }
    for s in simples {
      payload.extend(s);
    }
    encode_element(ids::TAG, 2, &payload)
  }

  fn parse_tags(payload: Vec<u8>, tracks: Vec<Track>) -> MediaMetadata {
    let bytes = encode_element(ids::TAGS, 4, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    let mut out = MediaMetadata::new("clip.mkv", 0);
    out.tracks = tracks;
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    out
  }

  fn make_track(uid_hex: &str) -> Track {
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
          uid_hex: Some(uid_hex.to_string()),
          ..CommonTrackProperties::default()
        },
        ..TrackProperties::default()
      },
    }
  }

  #[test]
  fn global_tag_routes_to_global_bundle() {
    let tag = build_tag(None, vec![build_simple_tag("TITLE", "Movie", None)]);
    let m = parse_tags(tag, vec![]);
    assert_eq!(m.tags.global.len(), 1);
    assert_eq!(m.tags.global[0].name, "TITLE");
    assert_eq!(m.tags.global[0].value, "Movie");
    assert_eq!(m.tags.per_track_count, 0);
  }

  #[test]
  fn simple_tag_strings_use_shared_element_budget() {
    let value = "A".repeat(17 * 1024);
    let tag = build_tag(None, vec![build_simple_tag("COMMENT", &value, None)]);
    let m = parse_tags(tag, vec![]);
    assert_eq!(m.tags.global.len(), 1);
    assert_eq!(m.tags.global[0].value, value);
  }

  #[test]
  fn per_track_tag_routes_to_matching_uid() {
    let track_uid: u64 = 0xCAFEBABE;
    let track = make_track(&format!("{:016x}", track_uid));
    let tag = build_tag(Some(track_uid), vec![build_simple_tag("ARTIST", "Hans", None)]);
    let m = parse_tags(tag, vec![track]);
    assert!(m.tags.global.is_empty());
    assert_eq!(m.tracks[0].properties.tags.len(), 1);
    assert_eq!(m.tracks[0].properties.tags[0].name, "ARTIST");
    assert_eq!(m.tags.per_track_count, 1);
  }

  // ---- PARSER-139: tag targeting a missing TrackUID is dropped ----------

  #[test]
  fn per_track_tag_with_unknown_uid_is_dropped() {
    // A tag carrying a TrackUID that matches no track is non-global and must
    // be discarded, not promoted to the file-wide tag list.
    let tag = build_tag(Some(0xDEAD), vec![build_simple_tag("X", "Y", None)]);
    let m = parse_tags(tag, vec![]);
    assert!(m.tags.global.is_empty());
    assert_eq!(m.tags.per_track_count, 0);
  }

  #[test]
  fn per_track_tag_with_unknown_uid_does_not_touch_other_tracks() {
    // Present a track whose UID does NOT match the tag's target; the tag must
    // still be dropped rather than attaching to the unrelated track or global.
    let other = make_track(&format!("{:016x}", 0x1111u64));
    let tag = build_tag(Some(0xDEAD), vec![build_simple_tag("X", "Y", None)]);
    let m = parse_tags(tag, vec![other]);
    assert!(m.tags.global.is_empty());
    assert!(m.tracks[0].properties.tags.is_empty());
  }

  #[test]
  fn nested_simple_tag_flattened() {
    // Build outer SimpleTag with TAG_NAME, TAG_STRING, plus a nested
    // SimpleTag for SUBTAG/value.
    let inner = build_simple_tag("SUBTAG", "child", None);
    let mut outer = Vec::new();
    outer.extend(encode_element_string(ids::TAG_NAME, 2, "PARENT"));
    outer.extend(encode_element_string(ids::TAG_STRING, 2, "parent value"));
    outer.extend(inner);
    let simple = encode_element(ids::TAG_SIMPLE, 2, &outer);
    let tag = build_tag(None, vec![simple]);
    let m = parse_tags(tag, vec![]);
    assert_eq!(m.tags.global.len(), 2);
    assert_eq!(m.tags.global[0].name, "PARENT");
    assert_eq!(m.tags.global[1].name, "SUBTAG");
  }

  #[test]
  fn simple_tag_language_round_trip() {
    let st = build_simple_tag("LANG", "Hello", Some("en"));
    let tag = build_tag(None, vec![st]);
    let m = parse_tags(tag, vec![]);
    assert_eq!(m.tags.global[0].language.as_deref(), Some("en"));
  }

  #[test]
  fn empty_simple_tag_skipped() {
    // SimpleTag with neither name nor string — should not produce an entry.
    let empty_simple = encode_element(ids::TAG_SIMPLE, 2, &[]);
    let tag = build_tag(None, vec![empty_simple]);
    let m = parse_tags(tag, vec![]);
    assert!(m.tags.global.is_empty());
  }

  #[test]
  fn per_track_count_sums_across_tracks() {
    let uid = 0x12345678u64;
    let track = make_track(&format!("{:016x}", uid));
    let tag1 = build_tag(Some(uid), vec![build_simple_tag("A", "1", None)]);
    let tag2 = build_tag(Some(uid), vec![build_simple_tag("B", "2", None)]);

    let mut payload = Vec::new();
    payload.extend(tag1);
    payload.extend(tag2);
    let m = parse_tags(payload, vec![track]);
    assert_eq!(m.tags.per_track_count, 2);
  }

  #[test]
  fn language_ietf_preferred_over_iso() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::TAG_NAME, 2, "X"));
    payload.extend(encode_element_string(ids::TAG_STRING, 2, "Y"));
    payload.extend(encode_element_string(ids::TAG_LANGUAGE, 2, "eng"));
    payload.extend(encode_element_string(ids::TAG_LANGUAGE_IETF, 2, "en-US"));
    let simple = encode_element(ids::TAG_SIMPLE, 2, &payload);
    let tag = build_tag(None, vec![simple]);
    let m = parse_tags(tag, vec![]);
    assert_eq!(m.tags.global[0].language.as_deref(), Some("en-US"));
  }
}
