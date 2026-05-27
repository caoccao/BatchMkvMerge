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

//! Final assembly step for the MP4 reader.  Converts the per-track
//! [`super::moov::TrackBuilder`] collection into protocol-level `Track`s
//! plus syncs derived fields onto the container.

use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::Dimensions2D;

use super::fragments::TrexDefaults;
use super::moov::MoovBuilder;

pub fn finalise(
  moov: MoovBuilder,
  is_fragmented: bool,
  fragment_track_counts: std::collections::HashMap<u32, u32>,
  out: &mut MediaMetadata,
) {
  moov.finalise_container(&mut out.container.properties);
  out.container.properties.is_fragmented = Some(is_fragmented);

  let mvex = &moov.mvex_defaults;
  let movie_matrix = moov.movie_matrix;

  // PARSER-212: QuickTime chapter tracks referenced by `tref/chap` are counted
  // as chapters and excluded from the track list.  A Nero `chpl` list, parsed
  // during the `moov` walk, takes precedence (mkvtoolnix's `read_chapter_track`
  // returns early when chapters already exist, `r_qtmp4.cpp:1172`).
  let chapter_track_ids: std::collections::HashSet<u32> = moov
    .tracks
    .iter()
    .flat_map(|t| t.chapter_track_ids.iter().copied())
    .collect();
  if out.chapters.num_entries == 0 {
    if let Some(count) = moov
      .tracks
      .iter()
      .filter(|t| t.track_id.is_some_and(|id| chapter_track_ids.contains(&id)))
      .find_map(|t| t.sample_count.filter(|&c| c > 0))
    {
      out.chapters.num_entries = count;
      out.chapters.num_editions = 1;
    }
  }

  // PARSER-161: assign ids compactly — a `trak` we end up dropping (metadata
  // handler, unknown handler, invalid timing, unsupported `mp4a`, missing AAC
  // config) must not burn an id, so the next emitted track keeps the lower
  // number.  Mirrors `r_qtmp4.cpp:720-743` where `dmx->id = m_demuxers.size()`
  // is set only after the demuxer is successfully handled and pushed.
  let mut next_id: i64 = 0;
  for builder in moov.tracks {
    // A track that is a chapter source is consumed as chapters and excluded
    // from the output track list (mkvtoolnix erases `is_chapters()` demuxers,
    // `r_qtmp4.cpp:325`); it must not burn a track id.
    if builder.track_id.is_some_and(|id| chapter_track_ids.contains(&id)) {
      continue;
    }
    if let Some(t) = build_track(builder, next_id, mvex, &fragment_track_counts, &movie_matrix) {
      out.tracks.push(t);
      next_id += 1;
    }
  }
  // Defensive: derive display_dimensions where missing.
  for track in &mut out.tracks {
    if let Some(video) = track.properties.video.as_mut() {
      if video.display_dimensions.is_none() {
        video.display_dimensions = video.pixel_dimensions;
      }
    }
  }
  out.tags.per_track_count = out.tracks.iter().map(|t| t.properties.tags.len() as u32).sum();
}

fn build_track(
  builder: super::moov::TrackBuilder,
  id: i64,
  mvex: &TrexDefaults,
  fragment_track_counts: &std::collections::HashMap<u32, u32>,
  movie_matrix: &[[i32; 3]; 3],
) -> Option<Track> {
  // PARSER-177: tracks the reader's first-sample verification pass rejected
  // (broken / missing decoder config that mkvtoolnix could not salvage) are
  // dropped before the compact-id assignment, so they do not burn an id.
  if builder.probe_failed {
    return None;
  }
  // PARSER-146: tracks whose mdhd was unsupported / had a zero timescale are
  // dropped (mkvtoolnix skips them rather than emitting bad timing).
  if builder.media_invalid {
    return None;
  }
  let mut codec_id = builder.codec_id_str.clone().unwrap_or_default();
  if codec_id.is_empty() {
    return None;
  }
  let handler_type = builder.handler_type?;
  let handler = super::moov::hdlr::Handler {
    handler_type,
    name: String::new(),
  };
  if handler.is_metadata_handler() {
    return None;
  }
  let track_type = match handler.classify() {
    TrackType::Unknown => return None, // skip non-track handlers
    t => t,
  };

  // PARSER-043/PARSER-389: mkvtoolnix checks the supported esds
  // objectTypeIndication table before falling back to the sample-entry FourCC,
  // even when the sample entry is not one of the generic MPEG-4 system codes.
  let mut codec_name = builder.codec_name.clone();
  let mut codec_from_esds = false;
  if let Some(ot) = builder.esds_object_type {
    if let Some((id, name)) = codec_from_object_type(ot) {
      codec_id = id.to_string();
      codec_name = Some(name.to_string());
      codec_from_esds = true;
    }
  }
  if !codec_from_esds {
    if let Some((id, name)) = pcm_codec_from_sample_entry(&builder) {
      codec_id = id.to_string();
      codec_name = Some(name.to_string());
    }
  }

  // PARSER-150: mkvtoolnix drops `mp4a` tracks whose esds objectTypeIndication
  // is missing or unsupported (r_qtmp4.cpp:3733-3739) instead of emitting a
  // generic, unusable `mp4a` track.
  if codec_id == "mp4a" {
    return None;
  }
  // PARSER-150: AAC tracks require the esds DecoderSpecificInfo
  // (AudioSpecificConfig); without it the track cannot be muxed faithfully,
  // so it is skipped (r_qtmp4.cpp:3741-3743).
  if codec_id == "A_AAC"
    && builder
      .audio_codec_config
      .as_ref()
      .and_then(|c| c.aac_object_type)
      .is_none()
  {
    return None;
  }

  let mut common = CommonTrackProperties::default();
  common.number = builder.track_id.map(|id| id as u64);
  common.track_name = builder.handler_name;
  common.language = builder.language_iso_639_2.as_deref().and_then(Language::from_valid_hint);
  if let Some(enabled) = builder.enabled {
    common.enabled = crate::media_metadata::model::track_properties_common::TrackFlag::from_bool(enabled);
  }
  // PARSER-145: report the per-track index-entry count.  A non-fragmented
  // track's count comes from the `stsz` sample count; a fragmented track's
  // from the aggregated `trun` sample counts.
  if let Some(count) = builder.sample_count {
    common.num_index_entries = Some(count as u64);
  }
  if let Some(track_id) = builder.track_id {
    if let Some(count) = fragment_track_counts.get(&track_id) {
      common.num_index_entries = Some(*count as u64);
    }
  }

  // Derive default_duration_ns from stts (preferred) or mvex defaults.
  let mut default_duration_ns: Option<u64> = None;
  if let (Some(timescale), Some(delta)) = (builder.media_timescale, builder.stts_first_sample_delta) {
    if timescale > 0 {
      default_duration_ns = Some(((delta as u128) * 1_000_000_000 / timescale as u128) as u64);
    }
  }
  if default_duration_ns.is_none() {
    if let (Some(track_id), Some(timescale)) = (builder.track_id, builder.media_timescale) {
      if let Some(dur) = mvex.default_duration_for(track_id) {
        if timescale > 0 {
          default_duration_ns = Some(((dur as u128) * 1_000_000_000 / timescale as u128) as u64);
        }
      }
    }
  }

  let codec_private = builder.codec_private_hex.as_ref().map(|hex| CodecPrivate {
    length: (hex.len() / 2) as u64,
    hex: hex.clone(),
  });

  let codec = CodecInfo {
    id: codec_id,
    name: codec_name,
    codec_private,
  };

  let mut properties = TrackProperties {
    common,
    tags: builder.tags,
    ..TrackProperties::default()
  };
  // PARSER-179: carry the builder's block-addition mappings (dvcC / dvvC /
  // hvcE) onto the video track.  Done before the per-type move of
  // `builder.video` so the bytes survive even when no other video config is
  // present.
  let block_addition_mappings: Vec<crate::media_metadata::model::track_properties_video::BlockAdditionMapping> =
    builder
      .block_additions
      .iter()
      .map(
        |(fourcc, bytes)| crate::media_metadata::model::track_properties_video::BlockAdditionMapping {
          id_type: fourcc.clone(),
          data_hex: super::codec_specific::hex_encode(bytes),
          // MP4 Dolby Vision config boxes carry no BlockAddIDName / Value.
          ..Default::default()
        },
      )
      .collect();

  match track_type {
    TrackType::Video => {
      let mut video = builder.video.unwrap_or_default();
      video.block_addition_mappings = block_addition_mappings;
      if video.display_dimensions.is_none() {
        if let (Some(w), Some(h)) = display_from_fixed(builder.display_width_fixed, builder.display_height_fixed) {
          video.display_dimensions = Some(Dimensions2D { width: w, height: h });
        }
      }
      video.default_duration_ns = default_duration_ns;
      // PARSER-147: combine the track and movie matrices to recover the
      // projection yaw/roll, including non-cardinal rotations the simple
      // tkhd rotation field cannot express.
      if let Some(track_matrix) = builder.display_matrix {
        if let Some((yaw, roll)) = compute_projection_pose(&track_matrix, movie_matrix) {
          if yaw != 0.0 || roll.abs() >= 0.5 {
            let projection = video
              .projection
              .get_or_insert_with(crate::media_metadata::model::track_properties_video::ProjectionMetadata::default);
            projection.pose =
              Some(crate::media_metadata::model::track_properties_video::ProjectionPose { yaw, pitch: 0.0, roll });
          }
        }
      }
      properties.video = Some(video);
    }
    TrackType::Audio => {
      properties.audio = Some(builder.audio.unwrap_or_default());
    }
    TrackType::Subtitles => {
      properties.subtitle = Some(builder.video.as_ref().map(|_| ()).map_or_else(
        || crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties {
          text_subtitles: matches!(codec.id.as_str(), "text" | "tx3g" | "wvtt" | "stpp"),
          encoding: None,
          variant: Some(codec.id.clone()),
          teletext_page: None,
        },
        |_| crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties {
          text_subtitles: matches!(codec.id.as_str(), "text" | "tx3g" | "wvtt" | "stpp"),
          encoding: None,
          variant: Some(codec.id.clone()),
          teletext_page: None,
        },
      ));
    }
    TrackType::Buttons | TrackType::Unknown => {}
  }

  Some(Track {
    id,
    track_type,
    codec,
    properties,
  })
}

/// PARSER-177: resolve the effective codec id for a track builder, mirroring
/// `r_qtmp4.cpp::determine_codec` — the `esds` objectTypeIndication wins over
/// the raw sample-entry FOURCC whenever mkvtoolnix's object-type table
/// recognises it.  Factored out so both the reader's verification pass and
/// `build_track` agree on the codec used for gating.
/// Returns the empty string when the builder carries no codec id.
pub fn effective_codec_id(builder: &super::moov::TrackBuilder) -> String {
  let codec_id = builder.codec_id_str.clone().unwrap_or_default();
  if let Some(ot) = builder.esds_object_type {
    if let Some((id, _name)) = codec_from_object_type(ot) {
      return id.to_string();
    }
  }
  if let Some((id, _name)) = pcm_codec_from_sample_entry(builder) {
    return id.to_string();
  }
  codec_id
}

/// Map an MPEG-4 `objectTypeIndication` to a (codec_id, name) pair. Mirrors
/// `codec_c::look_up_object_type_id`, which `r_qtmp4.cpp::determine_codec`
/// uses before falling back to the sample-entry FOURCC.
fn codec_from_object_type(object_type: u8) -> Option<(&'static str, &'static str)> {
  Some(match object_type {
    0x40 | 0x66 | 0x67 | 0x68 => ("A_AAC", "AAC"),
    0x69 => ("A_MPEG/L3", "MP3"),
    0x6B => ("A_MPEG/L2", "MP2"),
    0xA9 => ("A_DTS", "DTS"),
    0xDD => ("A_VORBIS", "Vorbis"),
    0x60 | 0x61 | 0x62 | 0x63 | 0x64 | 0x65 | 0x6A => ("V_MPEG12", "MPEG-1/2"),
    0x20 => ("V_MPEG4/ISO/ASP", "MPEG-4 Visual"),
    0xE0 => ("S_VOBSUB", "VobSub"),
    _ => return None,
  })
}

fn pcm_codec_from_sample_entry(builder: &super::moov::TrackBuilder) -> Option<(&'static str, &'static str)> {
  let codec_id = builder.codec_id_str.as_deref()?;
  let key = codec_id.to_ascii_lowercase();
  match key.as_str() {
    "twos" => Some(("A_PCM/INT/BIG", "PCM (signed integer, big-endian)")),
    "sowt" | "raw " | "in24" | "pcm " => Some(("A_PCM/INT/LIT", "PCM (signed integer, little-endian)")),
    "lpcm" => {
      let flags = builder.audio_format_flags.unwrap_or(0);
      if flags & 0x01 != 0 {
        Some(("A_PCM/FLOAT/IEEE", "PCM (IEEE float)"))
      } else if flags & 0x02 != 0 {
        Some(("A_PCM/INT/BIG", "PCM (signed integer, big-endian)"))
      } else {
        Some(("A_PCM/INT/LIT", "PCM (signed integer, little-endian)"))
      }
    }
    _ => None,
  }
}

/// Combine the track and movie display matrices and derive `(yaw, roll)` in
/// degrees, or `None` when the result is not an orthogonal transform.  Port of
/// `r_qtmp4.cpp:1572-1618`: columns 0/1 are 16.16 fixed-point, column 2 is
/// 2.30, so the products are shifted by `[16, 16, 30]` before summing.
fn compute_projection_pose(track: &[[i32; 3]; 3], movie: &[[i32; 3]; 3]) -> Option<(f64, f64)> {
  const SHIFTS: [i32; 3] = [16, 16, 30];
  let mut m = [[0i64; 3]; 3];
  for i in 0..3 {
    for j in 0..3 {
      for k in 0..3 {
        m[i][j] += ((track[i][k] as i64) * (movie[k][j] as i64)) >> SHIFTS[k];
      }
    }
  }
  // Reject affine (perspective) transforms and singular 2×2 blocks.
  if m[0][2] != 0 || m[1][2] != 0 {
    return None;
  }
  if m[0][0] == 0 && m[0][1] == 0 {
    return None;
  }
  let yaw = if m[0][0] == m[1][1] && -m[0][1] == m[1][0] {
    0.0
  } else if -m[0][0] == m[1][1] && m[0][1] == m[1][0] {
    180.0
  } else {
    return None;
  };
  let roll = (m[1][0] as f64).atan2(m[1][1] as f64) * 180.0 / std::f64::consts::PI;
  Some((yaw, roll))
}

fn display_from_fixed(width_fixed: Option<u32>, height_fixed: Option<u32>) -> (Option<u32>, Option<u32>) {
  let w = width_fixed.and_then(|f| if f != 0 { Some(f >> 16) } else { None });
  let h = height_fixed.and_then(|f| if f != 0 { Some(f >> 16) } else { None });
  (w, h)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::model::container::ContainerFormat;
  use std::collections::HashMap;

  fn video_builder(track_id: u32, codec: &str, lang: Option<&str>) -> super::super::moov::TrackBuilder {
    let mut b = super::super::moov::TrackBuilder::default();
    b.track_id = Some(track_id);
    b.codec_id_str = Some(codec.to_string());
    b.handler_type = Some(*b"vide");
    b.media_timescale = Some(48_000);
    b.stts_first_sample_delta = Some(1000);
    b.stts_first_sample_count = Some(60);
    b.display_width_fixed = Some(1920u32 << 16);
    b.display_height_fixed = Some(1080u32 << 16);
    b.video = Some(
      crate::media_metadata::model::track_properties_video::VideoTrackProperties {
        pixel_dimensions: Some(crate::media_metadata::model::track_properties_video::Dimensions2D {
          width: 1920,
          height: 1080,
        }),
        ..Default::default()
      },
    );
    if let Some(l) = lang {
      b.language_iso_639_2 = Some(l.to_string());
    }
    b
  }

  fn audio_builder(track_id: u32, codec: &str) -> super::super::moov::TrackBuilder {
    let mut b = super::super::moov::TrackBuilder::default();
    b.track_id = Some(track_id);
    b.codec_id_str = Some(codec.to_string());
    b.handler_type = Some(*b"soun");
    b.media_timescale = Some(48_000);
    b.audio = Some(
      crate::media_metadata::model::track_properties_audio::AudioTrackProperties {
        channels: Some(2),
        sampling_frequency: Some(48_000.0),
        ..Default::default()
      },
    );
    // A bare `mp4a` entry needs an esds object type + AudioSpecificConfig to
    // survive PARSER-150's filtering; give it an AAC decoder config.
    if codec == "mp4a" {
      b.esds_object_type = Some(0x40);
      b.audio_codec_config = Some(crate::media_metadata::model::track_properties_audio::AudioCodecConfig {
        aac_object_type: Some(2),
        ..Default::default()
      });
    }
    b
  }

  fn subtitle_builder(track_id: u32, codec: &str) -> super::super::moov::TrackBuilder {
    let mut b = super::super::moov::TrackBuilder::default();
    b.track_id = Some(track_id);
    b.codec_id_str = Some(codec.to_string());
    b.handler_type = Some(*b"text");
    b.media_timescale = Some(1000);
    b
  }

  #[test]
  fn empty_moov_yields_no_tracks() {
    let mut m = MediaMetadata::new("clip.mp4", 0);
    m.container.format = ContainerFormat::Mp4;
    finalise(MoovBuilder::default(), false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
    assert_eq!(m.container.properties.is_fragmented, Some(false));
  }

  #[test]
  fn fragmented_flag_round_trips() {
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(MoovBuilder::default(), true, HashMap::new(), &mut m);
    assert_eq!(m.container.properties.is_fragmented, Some(true));
  }

  #[test]
  fn video_track_finalised_with_stts_default_duration() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(video_builder(1, "avc1", Some("eng")));
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks.len(), 1);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.default_duration_ns, Some(20_833_333));
    assert_eq!(m.tracks[0].properties.common.language.as_ref().unwrap().iso639_2, "eng");
  }

  #[test]
  fn mvex_default_duration_used_when_stts_absent() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(7, "avc1", None);
    b.stts_first_sample_delta = None;
    b.stts_first_sample_count = None;
    moov.tracks.push(b);
    moov.mvex_defaults.entries.push(super::super::fragments::TrexEntry {
      track_id: 7,
      default_sample_duration: 2000,
      default_sample_size: 0,
    });
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, true, HashMap::new(), &mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    // 2000 / 48000 = 41_666_666 ns
    assert_eq!(v.default_duration_ns, Some(41_666_666));
  }

  #[test]
  fn audio_track_propagated() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(audio_builder(2, "mp4a"));
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks.len(), 1);
    let a = m.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
  }

  #[test]
  fn subtitle_track_text_marked_for_text_codecs() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(subtitle_builder(3, "tx3g"));
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(sub.text_subtitles);
    assert_eq!(sub.variant.as_deref(), Some("tx3g"));
  }

  #[test]
  fn subtitle_track_image_for_unknown_codec() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(subtitle_builder(3, "image"));
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(!sub.text_subtitles);
  }

  #[test]
  fn track_without_codec_id_dropped() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(5, "", Some("eng"));
    b.codec_id_str = Some(String::new());
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn track_with_metadata_handler_dropped() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(5, "mdir", Some("eng"));
    b.handler_type = Some(*b"meta");
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn fragment_track_count_routed_to_num_index_entries() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(video_builder(9, "avc1", None));
    let mut counts = HashMap::new();
    counts.insert(9u32, 120u32);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, true, counts, &mut m);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(120));
  }

  // ---- PARSER-147: combined matrix yaw/roll ----------------------------

  fn fixed_matrix(cells: [[f64; 3]; 3]) -> [[i32; 3]; 3] {
    let mut m = [[0i32; 3]; 3];
    for i in 0..3 {
      for j in 0..3 {
        let scale = if j == 2 { 1u64 << 30 } else { 1u64 << 16 } as f64;
        m[i][j] = (cells[i][j] * scale) as i32;
      }
    }
    m
  }

  #[test]
  fn projection_pose_identity_is_none_worthwhile() {
    let id = super::super::moov::mvhd::IDENTITY_MATRIX;
    let (yaw, roll) = compute_projection_pose(&id, &id).unwrap();
    assert_eq!(yaw, 0.0);
    assert!(roll.abs() < 0.01);
  }

  #[test]
  fn projection_pose_90_degree_track_matrix() {
    // 90° rotation track matrix, identity movie matrix.
    let track = fixed_matrix([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let movie = super::super::moov::mvhd::IDENTITY_MATRIX;
    let (yaw, roll) = compute_projection_pose(&track, &movie).unwrap();
    assert_eq!(yaw, 0.0);
    assert!((roll.abs() - 90.0).abs() < 0.01);
  }

  #[test]
  fn projection_pose_flip_reports_yaw_180() {
    // Horizontal flip + rotation: yaw 180 branch.
    let track = fixed_matrix([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let movie = super::super::moov::mvhd::IDENTITY_MATRIX;
    let (yaw, _roll) = compute_projection_pose(&track, &movie).unwrap();
    assert_eq!(yaw, 180.0);
  }

  #[test]
  fn projection_pose_rejects_affine() {
    let mut track = super::super::moov::mvhd::IDENTITY_MATRIX;
    track[0][2] = 0x1000; // non-zero perspective column → rejected
    let movie = super::super::moov::mvhd::IDENTITY_MATRIX;
    assert!(compute_projection_pose(&track, &movie).is_none());
  }

  #[test]
  fn video_track_gets_projection_pose_from_rotated_matrix() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(1, "avc1", None);
    b.display_matrix = Some(fixed_matrix([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]));
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    let pose = v.projection.as_ref().and_then(|p| p.pose).unwrap();
    assert!((pose.roll.abs() - 90.0).abs() < 0.01);
  }

  // ---- PARSER-146: invalid mdhd drops the track ------------------------

  #[test]
  fn media_invalid_track_dropped() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(1, "avc1", Some("eng"));
    b.media_invalid = true;
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
  }

  // ---- PARSER-150: unsupported mp4a / missing AAC config dropped --------

  #[test]
  fn bare_mp4a_without_esds_object_type_dropped() {
    let mut moov = MoovBuilder::default();
    let mut b = audio_builder(2, "mp4a");
    b.esds_object_type = None; // unsupported / missing object type
    b.audio_codec_config = None;
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn aac_without_decoder_config_dropped() {
    let mut moov = MoovBuilder::default();
    let mut b = audio_builder(2, "mp4a");
    b.esds_object_type = Some(0x40); // AAC object type, but no AudioSpecificConfig
    b.audio_codec_config = None;
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn aac_with_decoder_config_kept() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(audio_builder(2, "mp4a")); // helper sets object type + config
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].codec.id, "A_AAC");
  }

  #[test]
  fn esds_object_type_table_matches_mkvtoolnix_supported_set() {
    assert_eq!(codec_from_object_type(0x40).unwrap().0, "A_AAC");
    assert_eq!(codec_from_object_type(0x69).unwrap().0, "A_MPEG/L3");
    assert_eq!(codec_from_object_type(0x6B).unwrap().0, "A_MPEG/L2");
    assert_eq!(codec_from_object_type(0xA9).unwrap().0, "A_DTS");
    assert_eq!(codec_from_object_type(0x60).unwrap(), ("V_MPEG12", "MPEG-1/2"));
    assert_eq!(codec_from_object_type(0x65).unwrap(), ("V_MPEG12", "MPEG-1/2"));
    assert_eq!(codec_from_object_type(0x6A).unwrap(), ("V_MPEG12", "MPEG-1/2"));
    assert_eq!(codec_from_object_type(0x20).unwrap().0, "V_MPEG4/ISO/ASP");
    assert_eq!(codec_from_object_type(0xE0).unwrap().0, "S_VOBSUB");
    assert!(codec_from_object_type(0xA5).is_none());
    assert!(codec_from_object_type(0xA6).is_none());
    assert!(codec_from_object_type(0x23).is_none());
    assert!(codec_from_object_type(0x6C).is_none());
  }

  #[test]
  fn recognised_esds_object_type_overrides_nongeneric_sample_entry() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(1, "avc1", None);
    b.esds_object_type = Some(0x20);
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].codec.id, "V_MPEG4/ISO/ASP");
    assert_eq!(m.tracks[0].codec.name.as_deref(), Some("MPEG-4 Visual"));

    let mut b = video_builder(1, "avc1", None);
    b.esds_object_type = Some(0x20);
    assert_eq!(effective_codec_id(&b), "V_MPEG4/ISO/ASP");
  }

  // ---- PARSER-265: QuickTime PCM sample-entry mapping ------------------

  #[test]
  fn lpcm_flags_select_pcm_variant() {
    let mut moov = MoovBuilder::default();
    let mut b = audio_builder(2, "lpcm");
    b.audio_format_flags = Some(0x01);
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mov", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks[0].codec.id, "A_PCM/FLOAT/IEEE");

    let mut moov = MoovBuilder::default();
    let mut b = audio_builder(2, "lpcm");
    b.audio_format_flags = Some(0x02);
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mov", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks[0].codec.id, "A_PCM/INT/BIG");
  }

  #[test]
  fn in24_maps_to_little_endian_pcm() {
    let mut moov = MoovBuilder::default();
    moov.tracks.push(audio_builder(2, "in24"));
    let mut m = MediaMetadata::new("clip.mov", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks[0].codec.id, "A_PCM/INT/LIT");
  }

  // ---- PARSER-145: stsz sample count → num_index_entries ---------------

  #[test]
  fn sample_count_routed_to_num_index_entries() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(1, "avc1", None);
    b.sample_count = Some(250);
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    assert_eq!(m.tracks[0].properties.common.num_index_entries, Some(250));
  }

  // ---- PARSER-212: QuickTime tref/chap chapter tracks -----------------

  #[test]
  fn quicktime_chapter_track_counted_and_excluded() {
    let mut moov = MoovBuilder::default();
    let mut video = video_builder(1, "avc1", Some("eng"));
    video.chapter_track_ids = vec![2];
    moov.tracks.push(video);
    // Chapter text track (id 2) with 5 samples → 5 chapters.
    let mut chap = subtitle_builder(2, "text");
    chap.sample_count = Some(5);
    moov.tracks.push(chap);
    let mut m = MediaMetadata::new("clip.mov", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    // The chapter track is consumed, not emitted as a subtitle track.
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.chapters.num_entries, 5);
    assert_eq!(m.chapters.num_editions, 1);
  }

  #[test]
  fn nero_chapters_take_precedence_over_chapter_track() {
    let mut moov = MoovBuilder::default();
    let mut video = video_builder(1, "avc1", None);
    video.chapter_track_ids = vec![2];
    moov.tracks.push(video);
    let mut chap = subtitle_builder(2, "text");
    chap.sample_count = Some(5);
    moov.tracks.push(chap);
    let mut m = MediaMetadata::new("clip.mov", 0);
    // Simulate a Nero `chpl` list already parsed during the moov walk.
    m.chapters.num_entries = 3;
    m.chapters.num_editions = 1;
    finalise(moov, false, HashMap::new(), &mut m);
    // chpl wins, but the chapter track is still excluded from the list.
    assert_eq!(m.chapters.num_entries, 3);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
  }

  #[test]
  fn display_dimensions_filled_from_fixed_when_video_lacks_them() {
    let mut moov = MoovBuilder::default();
    let mut b = video_builder(1, "avc1", None);
    // Wipe the pre-filled display dimensions
    b.video.as_mut().unwrap().display_dimensions = None;
    moov.tracks.push(b);
    let mut m = MediaMetadata::new("clip.mp4", 0);
    finalise(moov, false, HashMap::new(), &mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.display_dimensions.unwrap().width, 1920);
  }
}
