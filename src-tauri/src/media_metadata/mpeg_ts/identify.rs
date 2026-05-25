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

//! Convert the per-PID stream registry into protocol Tracks + container
//! programs.

use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::program::Program;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use super::stream_table::StreamRow;

/// PARSER-158: codec parameters recovered from a bounded PES header probe,
/// keyed by elementary PID.  Audio channels / sampling frequency and video
/// pixel dimensions that the PMT alone cannot supply.
#[derive(Debug, Clone, Default)]
pub struct EsEnrichment {
  pub channels: Option<u32>,
  pub sampling_frequency: Option<f64>,
  pub pixel_dimensions: Option<(u32, u32)>,
}

pub fn finalise(rows: Vec<StreamRow>, out: &mut MediaMetadata) {
  finalise_with_sdt(rows, &std::collections::HashMap::new(), &std::collections::HashMap::new(), out);
}

/// As [`finalise`], but also applies SDT service provider/name keyed by
/// program (service id) — PARSER-055 — and PES-probed codec parameters keyed
/// by elementary PID — PARSER-158.
pub fn finalise_with_sdt(
  rows: Vec<StreamRow>,
  sdt: &std::collections::HashMap<u16, (String, String)>,
  enrichment: &std::collections::HashMap<u16, EsEnrichment>,
  out: &mut MediaMetadata,
) {
  out.container.format = ContainerFormat::MpegTs;
  out.container.recognized = true;
  out.container.supported = true;
  out.container.properties.is_fragmented = Some(false);

  // Build container programs from the unique program_number values.
  let mut seen_programs: std::collections::BTreeMap<u16, Program> = std::collections::BTreeMap::new();
  for row in &rows {
    let entry = seen_programs.entry(row.program_number).or_insert(Program {
      program_number: row.program_number as u32,
      pmt_pid: None,
      service_name: row.service_name.clone(),
      service_provider: None,
      track_ids: Vec::new(),
    });
    if entry.service_name.is_none() {
      entry.service_name = row.service_name.clone();
    }
  }
  // Apply SDT provider + service name keyed by service id (= program number).
  for (service_id, (provider, name)) in sdt {
    if let Some(entry) = seen_programs.get_mut(service_id) {
      if !provider.is_empty() {
        entry.service_provider = Some(provider.clone());
      }
      if !name.is_empty() {
        entry.service_name = Some(name.clone());
      }
    }
  }

  // PARSER-160: assign ids with a compact `track_id++` sequence over the
  // *emitted* tracks only.  Skipping an unknown/unsupported PMT row must not
  // leave a gap, so the first valid track always gets id 0 — matching
  // mkvtoolnix's `r_mpeg_ts.cpp:1546-1562` which only bumps `track_id` for
  // probed-OK tracks.
  let mut track_id = 0i64;
  for row in rows.into_iter() {
    if matches!(row.track_kind, crate::media_metadata::codec::TrackKind::Unknown) {
      // Skip system/private streams we can't classify.
      continue;
    }
    let track = make_track(track_id, &row, enrichment.get(&row.pid));
    if let Some(entry) = seen_programs.get_mut(&row.program_number) {
      entry.track_ids.push(track.id);
    }
    out.tracks.push(track);
    track_id += 1;
  }
  out.container.properties.programs = seen_programs.into_values().collect();
}

fn make_track(id: i64, row: &StreamRow, enrichment: Option<&EsEnrichment>) -> Track {
  let track_type = match row.track_kind {
    crate::media_metadata::codec::TrackKind::Video => TrackType::Video,
    crate::media_metadata::codec::TrackKind::Audio => TrackType::Audio,
    crate::media_metadata::codec::TrackKind::Subtitle => TrackType::Subtitles,
    crate::media_metadata::codec::TrackKind::Button => TrackType::Buttons,
    crate::media_metadata::codec::TrackKind::Unknown => TrackType::Unknown,
  };

  let mut common = CommonTrackProperties::default();
  common.number = Some((id as u64) + 1);
  common.stream_id = Some(row.pid as u32);
  common.program_number = Some(row.program_number as u32);
  common.teletext_page = row.teletext_page;
  common.hearing_impaired = row.hearing_impaired;
  if let Some(lang) = &row.language {
    common.language = Some(Language::resolve(None, Some(lang), false));
  }

  let mut properties = TrackProperties {
    common,
    ..TrackProperties::default()
  };
  match track_type {
    TrackType::Video => {
      // PARSER-158: pixel dimensions recovered from the PES header probe.
      let mut video = VideoTrackProperties::default();
      if let Some((w, h)) = enrichment.and_then(|e| e.pixel_dimensions) {
        let dims = Dimensions2D { width: w, height: h };
        video.pixel_dimensions = Some(dims);
        video.display_dimensions = Some(dims);
      }
      properties.video = Some(video);
    }
    TrackType::Audio => {
      // PARSER-158: channels / sampling frequency recovered from the first
      // audio frame header in the PES payload.
      let mut audio = AudioTrackProperties::default();
      if let Some(e) = enrichment {
        audio.channels = e.channels;
        audio.sampling_frequency = e.sampling_frequency;
      }
      properties.audio = Some(audio);
    }
    TrackType::Subtitles => {
      let is_text = matches!(row.codec_id.as_str(), "S_TELETEXT" | "S_HDMV/TEXTST");
      properties.subtitle = Some(SubtitleTrackProperties {
        text_subtitles: is_text,
        encoding: None,
        variant: Some(row.codec_name.clone()),
        teletext_page: row.teletext_page,
      });
    }
    _ => {}
  }

  let codec_private = row.codec_private.as_ref().map(|bytes| CodecPrivate::from_bytes(bytes));

  Track {
    id,
    track_type,
    codec: CodecInfo {
      id: row.codec_id.clone(),
      name: Some(row.codec_name.clone()),
      codec_private,
    },
    properties,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::codec::TrackKind;

  fn row(pid: u16, kind: TrackKind, codec_id: &str) -> StreamRow {
    StreamRow {
      pid,
      stream_type: 0,
      program_number: 1,
      language: None,
      teletext_page: None,
      service_name: None,
      codec_id: codec_id.to_string(),
      codec_name: codec_id.to_string(),
      track_kind: kind,
      codec_private: None,
      hearing_impaired: None,
    }
  }

  #[test]
  fn finalise_emits_video_and_audio_tracks() {
    let rows = vec![
      row(0x100, TrackKind::Video, "V_MPEG4/ISO/AVC"),
      row(0x101, TrackKind::Audio, "A_AAC"),
    ];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(rows, &mut m);
    assert_eq!(m.container.format, ContainerFormat::MpegTs);
    assert_eq!(m.tracks.len(), 2);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.tracks[1].track_type, TrackType::Audio);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0x100));
    assert_eq!(m.tracks[1].properties.common.stream_id, Some(0x101));
  }

  #[test]
  fn unknown_track_kind_dropped() {
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![row(0x100, TrackKind::Unknown, "0xEE")], &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn skipped_unknown_row_does_not_create_id_gap() {
    // PARSER-160: an unknown row ahead of a valid one must not push the valid
    // track's id off 0.
    let rows = vec![
      row(0x100, TrackKind::Unknown, "0xEE"),
      row(0x101, TrackKind::Video, "V_MPEG4/ISO/AVC"),
      row(0x102, TrackKind::Audio, "A_AAC"),
    ];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(rows, &mut m);
    assert_eq!(m.tracks.len(), 2);
    assert_eq!(m.tracks[0].id, 0);
    assert_eq!(m.tracks[0].properties.common.number, Some(1));
    assert_eq!(m.tracks[1].id, 1);
    // The program's track_ids reference the compact ids, no gap.
    let prog = &m.container.properties.programs[0];
    assert_eq!(prog.track_ids, vec![0, 1]);
  }

  #[test]
  fn enrichment_populates_audio_and_video_params() {
    // PARSER-158: probed channels / sampling frequency / dimensions land on
    // the track properties.
    let rows = vec![
      row(0x100, TrackKind::Video, "V_MPEG4/ISO/AVC"),
      row(0x101, TrackKind::Audio, "A_AC3"),
    ];
    let mut enrichment = std::collections::HashMap::new();
    enrichment.insert(
      0x100u16,
      EsEnrichment {
        pixel_dimensions: Some((1920, 1080)),
        ..EsEnrichment::default()
      },
    );
    enrichment.insert(
      0x101u16,
      EsEnrichment {
        channels: Some(6),
        sampling_frequency: Some(48000.0),
        ..EsEnrichment::default()
      },
    );
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_sdt(rows, &std::collections::HashMap::new(), &enrichment, &mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.as_ref().map(|d| (d.width, d.height)), Some((1920, 1080)));
    let a = m.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn programs_built_from_unique_program_numbers() {
    let mut a = row(0x100, TrackKind::Video, "V_MPEG4/ISO/AVC");
    a.program_number = 1;
    let mut b = row(0x101, TrackKind::Audio, "A_AAC");
    b.program_number = 1;
    let mut c = row(0x200, TrackKind::Video, "V_MPEGH/ISO/HEVC");
    c.program_number = 2;
    c.service_name = Some("BBC One".to_string());

    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![a, b, c], &mut m);
    assert_eq!(m.container.properties.programs.len(), 2);
    let p1 = &m.container.properties.programs[0];
    assert_eq!(p1.program_number, 1);
    assert_eq!(p1.track_ids.len(), 2);
    let p2 = &m.container.properties.programs[1];
    assert_eq!(p2.program_number, 2);
    assert_eq!(p2.service_name.as_deref(), Some("BBC One"));
  }

  #[test]
  fn language_resolved_via_iso_639() {
    let mut r = row(0x100, TrackKind::Audio, "A_AAC");
    r.language = Some("fra".to_string());
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![r], &mut m);
    let lang = m.tracks[0].properties.common.language.as_ref().unwrap();
    assert_eq!(lang.iso639_2, "fra");
  }

  #[test]
  fn teletext_subtitle_marked_text_with_page() {
    let mut r = row(0x100, TrackKind::Subtitle, "S_TELETEXT");
    r.teletext_page = Some(888);
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![r], &mut m);
    let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(sub.text_subtitles);
    assert_eq!(sub.teletext_page, Some(888));
  }

  #[test]
  fn pgs_subtitle_marked_image() {
    let r = row(0x100, TrackKind::Subtitle, "S_HDMV/PGS");
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![r], &mut m);
    let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(!sub.text_subtitles);
  }
}
