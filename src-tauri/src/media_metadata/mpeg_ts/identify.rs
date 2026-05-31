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

/// PARSER-158 / PARSER-170: codec parameters recovered from a bounded PES
/// header probe.  Audio channels / sampling frequency / bit depth and video
/// pixel dimensions that the PMT alone cannot supply, plus the TextST dialog
/// style codec_private.
#[derive(Debug, Clone, Default)]
pub struct EsEnrichment {
  pub channels: Option<u32>,
  pub sampling_frequency: Option<f64>,
  pub bits_per_sample: Option<u32>,
  pub pixel_dimensions: Option<(u32, u32)>,
  /// PARSER-170: TextST codec_private (the dialog style segment).
  pub codec_private: Option<Vec<u8>>,
  /// PARSER-250: codec id + display name recovered from the probed elementary
  /// header, overriding the PMT table default.  mkvtoolnix's `new_stream_a_mpeg`
  /// replaces the codec with `header.get_codec()` so a stream the PMT defaulted
  /// to `A_MPEG/L3` is relabelled to the actual Layer I / II / III.
  pub codec_override: Option<(String, String)>,
}

/// PARSER-169: per-row probe outcome.  `keep == false` drops the row, mirroring
/// mkvtoolnix's `probed_ok && codec` filter (r_mpeg_ts.cpp:1547-1572).  Tracks
/// whose bounded PES content probe never succeeded must NOT be emitted; tracks
/// that need no content probe (PGS / DVBSUB / Teletext) are kept unconditionally
/// (r_mpeg_ts.cpp:1080-1084, 527-533, 820-843).
#[derive(Debug, Clone, Default)]
pub struct RowProbe {
  pub keep: bool,
  pub enrichment: EsEnrichment,
}

pub fn finalise(rows: Vec<StreamRow>, out: &mut MediaMetadata) {
  // Back-compat: keep every classified row (probed_ok = true) with no probe
  // enrichment.  Used by tests that build rows directly and don't model the
  // PES content probe.
  let probes: Vec<RowProbe> = rows
    .iter()
    .map(|_| RowProbe {
      keep: true,
      enrichment: EsEnrichment::default(),
    })
    .collect();
  finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, out);
}

/// Back-compat shim for the older `(rows, sdt, enrichment_by_pid)` signature.
/// Builds a keep-all probe list and merges the PID-keyed enrichment.  Used only
/// by tests; the reader now calls [`finalise_with_probes`] directly.
#[cfg(test)]
pub fn finalise_with_sdt(
  rows: Vec<StreamRow>,
  sdt: &std::collections::HashMap<u16, (String, String)>,
  enrichment: &std::collections::HashMap<u16, EsEnrichment>,
  out: &mut MediaMetadata,
) {
  let probes: Vec<RowProbe> = rows
    .iter()
    .map(|row| RowProbe {
      keep: true,
      enrichment: enrichment.get(&row.pid).cloned().unwrap_or_default(),
    })
    .collect();
  finalise_with_probes(rows, sdt, &probes, out);
}

/// Finalise the per-PID stream registry into protocol tracks + programs.
///
/// `probes` runs parallel to `rows`: each entry's `keep` flag mirrors
/// mkvtoolnix's `probed_ok && codec` filter (PARSER-169,
/// r_mpeg_ts.cpp:1547-1572) and its `enrichment` carries the bounded-PES
/// codec parameters (PARSER-158 / PARSER-170).  Applies SDT service
/// provider/name keyed by program (PARSER-055) and pairs Dolby Vision
/// base/enhancement layers (PARSER-173).
pub fn finalise_with_probes(
  rows: Vec<StreamRow>,
  sdt: &std::collections::HashMap<u16, (String, String)>,
  probes: &[RowProbe],
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

  // PARSER-173: pair Dolby Vision base/enhancement layers and hide (drop) the
  // enhancement layer.  Mirrors `pair_dovi_base_and_enhancement_layer_tracks`
  // (r_mpeg_ts.cpp:1429-1472) + the hidden-track skip during identification
  // (r_mpeg_ts.cpp:1699-1701).
  let hidden = compute_hidden_dovi_layers(&rows, probes);

  // PARSER-160 / PARSER-169: assign ids with a compact `track_id++` sequence
  // over the *emitted* tracks only.  A skipped row (unknown, not probed_ok, or
  // a hidden DV enhancement layer) must not leave a gap, matching mkvtoolnix's
  // `r_mpeg_ts.cpp:1546-1562` which only bumps `track_id` for surviving tracks.
  let mut track_id = 0i64;
  for (idx, row) in rows.iter().enumerate() {
    if matches!(row.track_kind, crate::media_metadata::codec::TrackKind::Unknown) {
      // Skip system/private streams we can't classify.
      continue;
    }
    // PARSER-169: drop rows whose bounded PES content probe never succeeded.
    if !probes.get(idx).map(|p| p.keep).unwrap_or(false) {
      continue;
    }
    // PARSER-173: drop the hidden DV enhancement layer.
    if hidden[idx] {
      continue;
    }
    let enrichment = probes.get(idx).map(|p| &p.enrichment);
    let track = make_track(track_id, row, enrichment);
    if let Some(entry) = seen_programs.get_mut(&row.program_number) {
      entry.track_ids.push(track.id);
    }
    out.tracks.push(track);
    track_id += 1;
  }
  out.container.properties.programs = seen_programs.into_values().collect();
}

/// PARSER-173: for each surviving track carrying a Dolby Vision config, find a
/// matching base-layer track — by the descriptor's base-layer PID when present,
/// else by the resolution/codec heuristic — and mark the enhancement layer
/// hidden.  Returns a per-row `hidden` flag parallel to `rows`.  Port of
/// `pair_dovi_base_and_enhancement_layer_tracks` (r_mpeg_ts.cpp:1429-1472) +
/// `contains_dovi_base_layer_for_enhancement_layer` (r_mpeg_ts.cpp:1132-1165).
fn compute_hidden_dovi_layers(rows: &[StreamRow], probes: &[RowProbe]) -> Vec<bool> {
  let mut hidden = vec![false; rows.len()];

  let dims = |idx: usize| -> Option<(u32, u32)> { probes.get(idx).and_then(|p| p.enrichment.pixel_dimensions) };

  for (el_idx, el) in rows.iter().enumerate() {
    let Some(el_profile) = el.dovi_profile else {
      continue;
    };
    // The EL must survive the probe filter to be considered.
    if !probes.get(el_idx).map(|p| p.keep).unwrap_or(false) {
      continue;
    }

    let mut bl_idx: Option<usize> = None;

    if let Some(base_pid) = el.dovi_base_layer_pid {
      // Pair by the explicit base-layer PID from the descriptor.
      bl_idx = (0..rows.len()).find(|&idx| idx != el_idx && rows[idx].pid == base_pid);
    } else {
      // Resolution/codec heuristic over all candidate base-layer tracks.
      for (cand_idx, cand) in rows.iter().enumerate() {
        if cand_idx == el_idx {
          continue;
        }
        if contains_dovi_base_layer(cand, el, el_profile, dims(cand_idx), dims(el_idx)) {
          bl_idx = Some(cand_idx);
          break;
        }
      }
    }

    if bl_idx.is_some() {
      hidden[el_idx] = true;
    }
  }

  hidden
}

/// Port of `track_c::contains_dovi_base_layer_for_enhancement_layer`
/// (r_mpeg_ts.cpp:1132-1165).  `cand` is the candidate base layer; `el` is the
/// enhancement layer carrying the DV config.
fn contains_dovi_base_layer(
  cand: &StreamRow,
  el: &StreamRow,
  el_profile: u32,
  cand_dims: Option<(u32, u32)>,
  el_dims: Option<(u32, u32)>,
) -> bool {
  // Same codec.
  if cand.codec_id != el.codec_id {
    return false;
  }
  // Only DV profiles 4 and 7 use a separate base layer.
  if el_profile != 4 && el_profile != 7 {
    return false;
  }
  let Some((cw, ch)) = cand_dims else {
    return false;
  };
  let Some((ew, eh)) = el_dims else {
    return false;
  };
  let resolution_type = if cw == 3840 && ch == 2160 {
    'U'
  } else if cw == 1920 && ch == 1080 {
    'F'
  } else {
    '?'
  };

  if el_profile == 4 {
    resolution_type == 'F' && ew == cw / 2 && eh == ch / 2
  } else {
    // profile == 7
    (resolution_type == 'F' && ew == cw && eh == ch) || (resolution_type == 'U' && ew == cw / 2 && eh == ch / 2)
  }
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
  // PARSER-171: mkvtoolnix sets both `number` and `stream_id` to the
  // elementary PID (r_mpeg_ts.cpp:1705-1706); only the compact 0-based `id`
  // (m_id) is a sequence over surviving tracks (r_mpeg_ts.cpp:1562).
  common.number = Some(row.pid as u64);
  common.stream_id = Some(row.pid as u32);
  common.program_number = Some(row.program_number as u32);
  common.teletext_page = row.teletext_page;
  common.hearing_impaired = row.hearing_impaired;
  if let Some(lang) = &row.language {
    common.language = Language::from_valid_hint(lang);
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
      // PARSER-158 / PARSER-170: channels / sampling frequency / bit depth
      // recovered from the first audio frame header in the PES payload.
      let mut audio = AudioTrackProperties::default();
      if let Some(e) = enrichment {
        audio.channels = e.channels;
        audio.sampling_frequency = e.sampling_frequency;
        audio.bit_depth = e.bits_per_sample;
      }
      properties.audio = Some(audio);
    }
    TrackType::Subtitles => {
      properties.subtitle = Some(SubtitleTrackProperties {
        // PARSER-346: mkvtoolnix does not mark Teletext or HDMV TextST rows as
        // text subtitles here; `text_subtitles` is reserved for SRT-style text
        // streams, which the TS reader does not emit.
        text_subtitles: false,
        encoding: None,
        variant: Some(row.codec_name.clone()),
        teletext_page: row.teletext_page,
      });
    }
    _ => {}
  }

  // PARSER-170: TextST codec_private is built from the PES dialog-style segment
  // (enrichment), so prefer it over the (absent) descriptor-derived bytes.
  let codec_private = enrichment
    .and_then(|e| e.codec_private.as_ref())
    .or(row.codec_private.as_ref())
    .map(|bytes| CodecPrivate::from_bytes(bytes));

  // PARSER-250: prefer the codec id/name recovered from the elementary header
  // probe (e.g. MPEG audio Layer I/II/III) over the PMT table default.
  let (codec_id, codec_name) = match enrichment.and_then(|e| e.codec_override.clone()) {
    Some((id, name)) => (id, name),
    None => (row.codec_id.clone(), row.codec_name.clone()),
  };

  Track {
    id,
    track_type,
    codec: CodecInfo {
      id: codec_id,
      name: Some(codec_name),
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
      dovi_profile: None,
      dovi_base_layer_pid: None,
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
    // PARSER-171: `number` is the elementary PID, not id+1.
    assert_eq!(m.tracks[0].properties.common.number, Some(0x101));
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0x101));
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
    assert_eq!(
      v.pixel_dimensions.as_ref().map(|d| (d.width, d.height)),
      Some((1920, 1080))
    );
    let a = m.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn codec_override_relabels_mpeg_audio_layer() {
    // PARSER-250: a row defaulted to A_MPEG/L3 by the PMT table is relabelled
    // to the probed layer (A_MPEG/L2) via the enrichment codec override.
    let rows = vec![row(0x101, TrackKind::Audio, "A_MPEG/L3")];
    let mut enrichment = std::collections::HashMap::new();
    enrichment.insert(
      0x101u16,
      EsEnrichment {
        channels: Some(2),
        sampling_frequency: Some(44100.0),
        codec_override: Some(("A_MPEG/L2".to_string(), "MP2".to_string())),
        ..EsEnrichment::default()
      },
    );
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_sdt(rows, &std::collections::HashMap::new(), &enrichment, &mut m);
    assert_eq!(m.tracks[0].codec.id, "A_MPEG/L2");
    assert_eq!(m.tracks[0].codec.name.as_deref(), Some("MP2"));
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
  fn invalid_language_is_omitted() {
    let mut r = row(0x100, TrackKind::Audio, "A_AAC");
    r.language = Some("zzz".to_string());
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![r], &mut m);
    assert!(m.tracks[0].properties.common.language.is_none());
  }

  #[test]
  fn teletext_subtitle_keeps_page_without_text_flag() {
    let mut r = row(0x100, TrackKind::Subtitle, "S_TELETEXT");
    r.teletext_page = Some(888);
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise(vec![r], &mut m);
    let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(!sub.text_subtitles);
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

  // ---- PARSER-169: probed_ok gating drops rows -------------------------

  #[test]
  fn finalise_with_probes_drops_unprobed_rows() {
    let rows = vec![
      row(0x100, TrackKind::Video, "V_MPEG2"),
      row(0x101, TrackKind::Audio, "A_AAC"),
    ];
    // Video probed_ok, audio not.
    let probes = vec![
      RowProbe {
        keep: true,
        enrichment: EsEnrichment {
          pixel_dimensions: Some((1920, 1080)),
          ..EsEnrichment::default()
        },
      },
      RowProbe::default(),
    ];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].codec.id, "V_MPEG2");
    assert_eq!(m.tracks[0].id, 0);
  }

  // ---- PARSER-173: Dolby Vision base/enhancement pairing ---------------

  fn dv_row(pid: u16, codec_id: &str, profile: u32, base_layer_pid: Option<u16>) -> StreamRow {
    let mut r = row(pid, TrackKind::Video, codec_id);
    r.dovi_profile = Some(profile);
    r.dovi_base_layer_pid = base_layer_pid;
    r
  }

  fn video_probe(w: u32, h: u32) -> RowProbe {
    RowProbe {
      keep: true,
      enrichment: EsEnrichment {
        pixel_dimensions: Some((w, h)),
        ..EsEnrichment::default()
      },
    }
  }

  #[test]
  fn dovi_pairing_by_base_layer_pid_hides_enhancement_layer() {
    // Base layer on PID 0x100 (real HEVC); the enhancement layer on PID 0x101
    // names the base PID in its DV descriptor.  The EL is hidden/dropped.
    let rows = vec![
      row(0x100, TrackKind::Video, "V_MPEGH/ISO/HEVC"),
      dv_row(0x101, "V_MPEGH/ISO/HEVC", 7, Some(0x100)),
    ];
    let probes = vec![video_probe(3840, 2160), video_probe(3840, 2160)];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0x100));
  }

  #[test]
  fn dovi_pairing_by_resolution_heuristic_profile_7() {
    // Profile 7, base 4K ('U'), EL at half resolution (1920x1080), same codec.
    let rows = vec![
      row(0x100, TrackKind::Video, "V_MPEGH/ISO/HEVC"),
      dv_row(0x101, "V_MPEGH/ISO/HEVC", 7, None),
    ];
    let probes = vec![video_probe(3840, 2160), video_probe(1920, 1080)];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0x100));
  }

  #[test]
  fn dovi_no_base_layer_keeps_enhancement_track() {
    // No base-layer PID and no matching base resolution → the DV track is kept
    // as-is (r_mpeg_ts.cpp:1462-1465).
    let rows = vec![dv_row(0x101, "V_MPEGH/ISO/HEVC", 7, None)];
    let probes = vec![video_probe(1920, 1080)];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0x101));
  }

  #[test]
  fn dovi_base_layer_pid_not_found_keeps_enhancement_track() {
    // The DV descriptor names a base PID that no row carries → fallback keeps
    // the EL track.
    let rows = vec![dv_row(0x101, "V_MPEGH/ISO/HEVC", 7, Some(0x999))];
    let probes = vec![video_probe(3840, 2160)];
    let mut m = MediaMetadata::new("clip.ts", 0);
    finalise_with_probes(rows, &std::collections::HashMap::new(), &probes, &mut m);
    assert_eq!(m.tracks.len(), 1);
  }

  #[test]
  fn contains_dovi_base_layer_profile_4_requires_half_resolution() {
    // Profile 4: base 1080p ('F'), EL at half resolution (960x540).
    let bl = row(0x100, TrackKind::Video, "V_MPEGH/ISO/HEVC");
    let el = dv_row(0x101, "V_MPEGH/ISO/HEVC", 4, None);
    assert!(contains_dovi_base_layer(
      &bl,
      &el,
      4,
      Some((1920, 1080)),
      Some((960, 540))
    ));
    // Mismatched codec rejects.
    let bl_other = row(0x100, TrackKind::Video, "V_MPEG4/ISO/AVC");
    assert!(!contains_dovi_base_layer(
      &bl_other,
      &el,
      4,
      Some((1920, 1080)),
      Some((960, 540))
    ));
    // Unsupported profile rejects.
    assert!(!contains_dovi_base_layer(
      &bl,
      &el,
      5,
      Some((1920, 1080)),
      Some((960, 540))
    ));
    // Missing dimensions reject.
    assert!(!contains_dovi_base_layer(&bl, &el, 4, None, Some((960, 540))));
    assert!(!contains_dovi_base_layer(&bl, &el, 4, Some((1920, 1080)), None));
  }
}
