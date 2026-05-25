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

//! Matroska Tracks dispatcher.  Mirrors `r_matroska.cpp::read_headers_tracks`
//! (lines 1352-1507) — walks every TrackEntry under the Tracks element and
//! delegates to the per-domain parsers in [`common`], [`video`], [`audio`],
//! [`subtitles`], and [`block_addition`].

pub mod audio;
pub mod block_addition;
pub mod common;
pub mod subtitles;
pub mod video;

use crate::media_metadata::codec::matroska_codec_ids;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;

/// Cap for any single track-entry-level binary payload (CodecPrivate, etc.).
/// 4 MiB matches the largest realistic AV1/HEVC config we'd see header-only.
const TRACK_BINARY_CAP: u64 = 4 * 1024 * 1024;

/// Track type byte values per the Matroska spec.
const KAX_TRACK_VIDEO: u64 = 1;
const KAX_TRACK_AUDIO: u64 = 2;
const KAX_TRACK_COMPLEX: u64 = 3;
const KAX_TRACK_LOGO: u64 = 16;
const KAX_TRACK_SUBTITLE: u64 = 17;
const KAX_TRACK_BUTTONS: u64 = 18;
const KAX_TRACK_CONTROL: u64 = 32;

/// Walk Tracks element and populate `out.tracks`.
pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  // Track numbers already seen — duplicates are rejected (PARSER-035).
  let mut used_track_numbers = std::collections::HashSet::new();
  ebml::walk_children(src, parent, "matroska::tracks", deadline, |src, child| {
    if child.id == ids::TRACK_ENTRY {
      if let Some(track) = read_track_entry(src, child, deadline, out.tracks.len() as i64, &mut used_track_numbers)? {
        out.tracks.push(track);
      }
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

fn read_track_entry(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  id_seed: i64,
  used_track_numbers: &mut std::collections::HashSet<u64>,
) -> Result<Option<Track>, ParseError> {
  let mut common = common::CommonBuilder::default();
  let mut track_type_byte: Option<u64> = None;
  let mut codec_id: Option<String> = None;
  let mut codec_name: Option<String> = None;
  let mut codec_private: Option<Vec<u8>> = None;
  let mut video = video::VideoBuilder::default();
  let mut audio = audio::AudioBuilder::default();
  let subtitle = subtitles::SubtitleBuilder::default();
  let mut block_additions: Vec<block_addition::BlockAdditionMapping> = Vec::new();

  ebml::walk_children(
    src,
    parent,
    "matroska::track_entry",
    deadline,
    |src, child| match child.id {
      ids::TRACK_TYPE => {
        track_type_byte = Some(ebml::read_uint(src, child)?);
        Ok(ChildAction::Consumed)
      }
      ids::CODEC_ID => {
        codec_id = Some(ebml::read_string(src, child, 256)?);
        Ok(ChildAction::Consumed)
      }
      ids::CODEC_NAME => {
        codec_name = Some(ebml::read_string(src, child, 1024)?);
        Ok(ChildAction::Consumed)
      }
      ids::CODEC_PRIVATE => {
        codec_private = Some(ebml::read_binary(src, child, TRACK_BINARY_CAP)?);
        Ok(ChildAction::Consumed)
      }
      ids::TRACK_VIDEO => {
        video::parse(src, child, deadline, &mut video)?;
        Ok(ChildAction::Consumed)
      }
      ids::TRACK_AUDIO => {
        audio::parse(src, child, deadline, &mut audio)?;
        Ok(ChildAction::Consumed)
      }
      ids::BLOCK_ADDITION_MAPPING => {
        if let Some(mapping) = block_addition::parse(src, child, deadline)? {
          block_additions.push(mapping);
        }
        Ok(ChildAction::Consumed)
      }
      other => {
        if common::CommonBuilder::owns_id(other) {
          common.consume_child(src, child, deadline)?;
          Ok(ChildAction::Consumed)
        } else {
          Ok(ChildAction::Skip)
        }
      }
    },
  )?;

  // mkvmerge skips tracks missing their TrackNumber (r_matroska.cpp:1383-1387,
  // PARSER-034), and rejects duplicate track numbers, keeping the first
  // occurrence (PARSER-035).
  let track_number = match common.number {
    Some(n) => n,
    None => return Ok(None),
  };
  if !used_track_numbers.insert(track_number) {
    return Ok(None);
  }

  let track_type_byte = match track_type_byte {
    Some(v) => v,
    None => return Ok(None), // mkvmerge skips type-less tracks
  };
  let codec_id = codec_id.unwrap_or_default();
  if codec_id.is_empty() {
    // mkvmerge skips tracks without a CodecID (r_matroska.cpp:1481-1485).
    return Ok(None);
  }

  // Track type comes solely from the Matroska TrackType byte (audio/video/
  // subtitle); every other value — including "complex" — is Unknown, matching
  // mkvtoolnix, which never infers the type from the codec ID (PARSER-036).
  let track_type = classify_track_type(track_type_byte);

  // Resolve codec catalogue entry from the CodecID (e.g. V_MPEG4/ISO/AVC) for
  // the human-readable codec name only.
  let resolved = matroska_codec_ids::lookup(&codec_id);
  let resolved_name = codec_name.clone().or_else(|| resolved.map(|r| r.name.to_string()));

  let codec = CodecInfo {
    id: codec_id,
    name: resolved_name,
    codec_private: codec_private
      .as_deref()
      .map(crate::media_metadata::model::track::CodecPrivate::from_bytes),
  };

  // KaxTrackDefaultDuration is exposed on the per-domain properties
  // (PARSER-032).
  let default_duration_ns = common.default_duration_ns;

  let properties_common = common.build();
  let mut properties = TrackProperties {
    common: properties_common,
    ..TrackProperties::default()
  };
  // MaxBlockAdditionID stays the value read from the element — it is NOT the
  // count of BlockAdditionMapping entries (PARSER-037). The mappings are
  // walked for validation but not yet otherwise exposed.
  let _ = &block_additions;

  match track_type {
    TrackType::Video => {
      let mut v = video.build();
      v.default_duration_ns = default_duration_ns;
      properties.video = Some(v);
    }
    TrackType::Audio => {
      let mut a = audio.build();
      a.default_duration_ns = default_duration_ns;
      properties.audio = Some(a);
    }
    TrackType::Subtitles => {
      properties.subtitle = Some(subtitle.build_from_codec_id(&codec.id));
    }
    TrackType::Buttons | TrackType::Unknown => {}
  }

  Ok(Some(Track {
    id: id_seed,
    track_type,
    codec,
    properties,
  }))
}

fn classify_track_type(byte: u64) -> TrackType {
  // mkvtoolnix maps only audio/video/subtitle; everything else (complex,
  // logo, buttons, control, unknown) becomes Unknown (PARSER-036).
  match byte {
    KAX_TRACK_VIDEO => TrackType::Video,
    KAX_TRACK_AUDIO => TrackType::Audio,
    KAX_TRACK_SUBTITLE => TrackType::Subtitles,
    _ => TrackType::Unknown,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{
    encode_element, encode_element_float, encode_element_string, encode_element_uint,
  };
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_tracks(track_entries: Vec<Vec<u8>>) -> (Vec<u8>, ElementHeader, FileSource) {
    let mut payload = Vec::new();
    for e in track_entries {
      payload.extend(e);
    }
    let bytes = encode_element(ids::TRACKS, 4, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    let header = ebml::read_element_header(&mut s).unwrap();
    (bytes, header, s)
  }

  fn build_simple_video_track(codec_id: &str) -> Vec<u8> {
    build_simple_video_track_num(1, codec_id)
  }

  fn build_simple_video_track_num(number: u64, codec_id: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, number));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_VIDEO));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, codec_id));
    let mut video_payload = Vec::new();
    video_payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920));
    video_payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    payload.extend(encode_element(ids::TRACK_VIDEO, 1, &video_payload));
    encode_element(ids::TRACK_ENTRY, 1, &payload)
  }

  fn build_simple_audio_track(codec_id: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 2));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_AUDIO));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, codec_id));
    let mut audio_payload = Vec::new();
    audio_payload.extend(encode_element_float(ids::AUDIO_SAMPLING_FREQ, 1, 48_000.0));
    audio_payload.extend(encode_element_uint(ids::AUDIO_CHANNELS, 1, 2));
    payload.extend(encode_element(ids::TRACK_AUDIO, 1, &audio_payload));
    encode_element(ids::TRACK_ENTRY, 1, &payload)
  }

  fn build_simple_subtitle_track(codec_id: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 3));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_SUBTITLE));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, codec_id));
    encode_element(ids::TRACK_ENTRY, 1, &payload)
  }

  #[test]
  fn parse_one_video_one_audio_one_subtitle() {
    let (_b, header, mut s) = build_tracks(vec![
      build_simple_video_track("V_MPEG4/ISO/AVC"),
      build_simple_audio_track("A_AC3"),
      build_simple_subtitle_track("S_TEXT/UTF8"),
    ]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 3);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert_eq!(out.tracks[1].track_type, TrackType::Audio);
    assert_eq!(out.tracks[2].track_type, TrackType::Subtitles);

    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
    assert_eq!(v.pixel_dimensions.unwrap().height, 1080);

    let a = out.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48_000.0));
    assert_eq!(a.channels, Some(2));

    let sub = out.tracks[2].properties.subtitle.as_ref().unwrap();
    assert!(sub.text_subtitles);
  }

  #[test]
  fn track_without_codec_id_is_skipped() {
    let mut bad_payload = Vec::new();
    bad_payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    bad_payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_VIDEO));
    let bad = encode_element(ids::TRACK_ENTRY, 1, &bad_payload);

    let (_b, header, mut s) = build_tracks(vec![bad]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn track_without_track_type_is_skipped() {
    let mut bad_payload = Vec::new();
    bad_payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    bad_payload.extend(encode_element_string(ids::CODEC_ID, 1, "V_VP9"));
    let bad = encode_element(ids::TRACK_ENTRY, 1, &bad_payload);

    let (_b, header, mut s) = build_tracks(vec![bad]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn id_increments_per_track() {
    let (_b, header, mut s) = build_tracks(vec![
      build_simple_video_track_num(1, "V_VP8"),
      build_simple_video_track_num(2, "V_VP9"),
    ]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.tracks[0].id, 0);
    assert_eq!(out.tracks[1].id, 1);
  }

  #[test]
  fn classify_track_type_handles_logo_and_control_as_unknown() {
    assert_eq!(classify_track_type(KAX_TRACK_LOGO), TrackType::Unknown);
    assert_eq!(classify_track_type(KAX_TRACK_CONTROL), TrackType::Unknown);
    assert_eq!(classify_track_type(KAX_TRACK_BUTTONS), TrackType::Unknown);
  }

  // ---- PARSER-036: complex tracks are Unknown, not Video ----------------

  #[test]
  fn classify_track_type_handles_complex_as_unknown() {
    assert_eq!(classify_track_type(KAX_TRACK_COMPLEX), TrackType::Unknown);
  }

  #[test]
  fn complex_track_with_video_codec_is_not_video() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_COMPLEX));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, "V_MPEG4/ISO/AVC"));
    let entry = encode_element(ids::TRACK_ENTRY, 1, &payload);
    let (_b, header, mut s) = build_tracks(vec![entry]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Unknown);
    assert!(out.tracks[0].properties.video.is_none());
  }

  // ---- PARSER-034: tracks without TrackNumber are skipped ---------------

  #[test]
  fn track_without_track_number_is_skipped() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_VIDEO));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, "V_VP9"));
    let entry = encode_element(ids::TRACK_ENTRY, 1, &payload);
    let (_b, header, mut s) = build_tracks(vec![entry]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
  }

  // ---- PARSER-035: duplicate track numbers are rejected -----------------

  #[test]
  fn duplicate_track_number_is_rejected() {
    let (_b, header, mut s) = build_tracks(vec![
      build_simple_video_track_num(1, "V_VP8"),
      build_simple_video_track_num(1, "V_VP9"), // duplicate number 1
    ]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "V_VP8"); // first kept
  }

  // ---- PARSER-032: DefaultDuration reaches video/audio props ------------

  #[test]
  fn default_duration_exposed_on_video_and_audio() {
    let mut vpayload = Vec::new();
    vpayload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    vpayload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_VIDEO));
    vpayload.extend(encode_element_string(ids::CODEC_ID, 1, "V_VP9"));
    vpayload.extend(encode_element_uint(ids::DEFAULT_DURATION, 3, 41_708_333));
    let v = encode_element(ids::TRACK_ENTRY, 1, &vpayload);

    let mut apayload = Vec::new();
    apayload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 2));
    apayload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_AUDIO));
    apayload.extend(encode_element_string(ids::CODEC_ID, 1, "A_AC3"));
    apayload.extend(encode_element_uint(ids::DEFAULT_DURATION, 3, 24_000_000));
    let a = encode_element(ids::TRACK_ENTRY, 1, &apayload);

    let (_b, header, mut s) = build_tracks(vec![v, a]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(
      out.tracks[0].properties.video.as_ref().unwrap().default_duration_ns,
      Some(41_708_333)
    );
    assert_eq!(
      out.tracks[1].properties.audio.as_ref().unwrap().default_duration_ns,
      Some(24_000_000)
    );
  }

  // ---- PARSER-033: audio defaults ---------------------------------------

  #[test]
  fn audio_track_without_audio_block_uses_defaults() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_AUDIO));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, "A_AAC"));
    let entry = encode_element(ids::TRACK_ENTRY, 1, &payload);
    let (_b, header, mut s) = build_tracks(vec![entry]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(8000.0));
    assert_eq!(a.channels, Some(1));
  }

  // ---- PARSER-037: MaxBlockAdditionID kept, not overwritten -------------

  #[test]
  fn max_block_addition_id_is_the_element_value() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    payload.extend(encode_element_uint(ids::TRACK_TYPE, 1, KAX_TRACK_VIDEO));
    payload.extend(encode_element_string(ids::CODEC_ID, 1, "V_VP9"));
    payload.extend(encode_element_uint(ids::MAX_BLOCK_ADDITION_ID, 2, 4));
    // One BlockAdditionMapping with a valid id_type so it is recorded.
    let mut mapping_payload = Vec::new();
    mapping_payload.extend(encode_element_uint(ids::BLOCK_ADD_ID_TYPE, 2, 1));
    payload.extend(encode_element(ids::BLOCK_ADDITION_MAPPING, 2, &mapping_payload));
    let entry = encode_element(ids::TRACK_ENTRY, 1, &payload);
    let (_b, header, mut s) = build_tracks(vec![entry]);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    // 4 from the element, NOT 1 (the mapping count).
    assert_eq!(out.tracks[0].properties.common.max_block_addition_id, Some(4));
  }
}
