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
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::program::Program;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;
use crate::media_metadata::model::MediaMetadata;

use super::stream_table::StreamRow;

pub fn finalise(rows: Vec<StreamRow>, out: &mut MediaMetadata) {
    finalise_with_sdt(rows, &std::collections::HashMap::new(), out);
}

/// As [`finalise`], but also applies SDT service provider/name keyed by
/// program (service id) — PARSER-055.
pub fn finalise_with_sdt(
    rows: Vec<StreamRow>,
    sdt: &std::collections::HashMap<u16, (String, String)>,
    out: &mut MediaMetadata,
) {
    out.container.format = ContainerFormat::MpegTs;
    out.container.recognized = true;
    out.container.supported = true;
    out.container.properties.is_fragmented = Some(false);

    // Build container programs from the unique program_number values.
    let mut seen_programs: std::collections::BTreeMap<u16, Program> =
        std::collections::BTreeMap::new();
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

    for (idx, row) in rows.into_iter().enumerate() {
        if matches!(
            row.track_kind,
            crate::media_metadata::codec::TrackKind::Unknown
        ) {
            // Skip system/private streams we can't classify.
            continue;
        }
        let track = make_track(idx as i64, &row);
        if let Some(entry) = seen_programs.get_mut(&row.program_number) {
            entry.track_ids.push(track.id);
        }
        out.tracks.push(track);
    }
    out.container.properties.programs = seen_programs.into_values().collect();
}

fn make_track(id: i64, row: &StreamRow) -> Track {
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
    if let Some(lang) = &row.language {
        common.language = Some(Language::resolve(None, Some(lang), false));
    }

    let mut properties = TrackProperties {
        common,
        ..TrackProperties::default()
    };
    match track_type {
        TrackType::Video => {
            properties.video = Some(VideoTrackProperties::default());
        }
        TrackType::Audio => {
            properties.audio = Some(AudioTrackProperties::default());
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

    Track {
        id,
        track_type,
        codec: CodecInfo {
            id: row.codec_id.clone(),
            name: Some(row.codec_name.clone()),
            codec_private: None,
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
