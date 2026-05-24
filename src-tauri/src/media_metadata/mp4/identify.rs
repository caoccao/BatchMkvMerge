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
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::Dimensions2D;
use crate::media_metadata::model::MediaMetadata;

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
    let mut next_id: i64 = 0;
    for builder in moov.tracks {
        let track = build_track(builder, next_id, mvex, &fragment_track_counts);
        next_id += 1;
        if let Some(t) = track {
            out.tracks.push(t);
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
    out.tags.per_track_count = out
        .tracks
        .iter()
        .map(|t| t.properties.tags.len() as u32)
        .sum();
}

fn build_track(
    builder: super::moov::TrackBuilder,
    id: i64,
    mvex: &TrexDefaults,
    fragment_track_counts: &std::collections::HashMap<u32, u32>,
) -> Option<Track> {
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

    // PARSER-043: a generic MPEG-4 system sample entry (mp4a / mp4v / mp4s) is
    // refined to the real codec from the esds objectTypeIndication so MP3, AC-3,
    // DTS, AAC, AVC, etc. are not all reported as "mp4a"/"mp4v".
    let mut codec_name = builder.codec_name.clone();
    if matches!(codec_id.as_str(), "mp4a" | "mp4v" | "mp4s" | "mp4 ") {
        if let Some(ot) = builder.esds_object_type {
            if let Some((id, name)) = codec_from_object_type(ot) {
                codec_id = id.to_string();
                codec_name = Some(name.to_string());
            }
        }
    }

    let mut common = CommonTrackProperties::default();
    common.number = builder.track_id.map(|id| id as u64);
    common.track_name = builder.handler_name;
    common.language = builder.language_iso_639_2.as_deref().and_then(|code| {
        Some(Language::resolve(None, Some(code), /*default_eng=*/ false))
    });
    if let Some(enabled) = builder.enabled {
        common.enabled =
            crate::media_metadata::model::track_properties_common::TrackFlag::from_bool(enabled);
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
            default_duration_ns = Some(
                ((delta as u128) * 1_000_000_000 / timescale as u128) as u64,
            );
        }
    }
    if default_duration_ns.is_none() {
        if let (Some(track_id), Some(timescale)) = (builder.track_id, builder.media_timescale) {
            if let Some(dur) = mvex.default_duration_for(track_id) {
                if timescale > 0 {
                    default_duration_ns = Some(
                        ((dur as u128) * 1_000_000_000 / timescale as u128) as u64,
                    );
                }
            }
        }
    }

    let codec_private = builder
        .codec_private_hex
        .as_ref()
        .map(|hex| CodecPrivate {
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
    match track_type {
        TrackType::Video => {
            let mut video = builder.video.unwrap_or_default();
            if video.display_dimensions.is_none() {
                if let (Some(w), Some(h)) = display_from_fixed(
                    builder.display_width_fixed,
                    builder.display_height_fixed,
                ) {
                    video.display_dimensions = Some(Dimensions2D { width: w, height: h });
                }
            }
            video.default_duration_ns = default_duration_ns;
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

/// Map an MPEG-4 `objectTypeIndication` to a (codec_id, name) pair. Mirrors the
/// audio/video branches of `r_qtmp4.cpp::determine_codec`.
fn codec_from_object_type(object_type: u8) -> Option<(&'static str, &'static str)> {
    Some(match object_type {
        0x40 | 0x66 | 0x67 | 0x68 => ("A_AAC", "AAC"),
        0x69 | 0x6B => ("A_MPEG/L3", "MP3"),
        0x6A => ("A_MPEG/L2", "MP2"),
        0xA5 => ("A_AC3", "AC-3"),
        0xA6 => ("A_EAC3", "E-AC-3"),
        0xA9 | 0xAA | 0xAB => ("A_DTS", "DTS"),
        0xDD => ("A_VORBIS", "Vorbis"),
        0x20 => ("V_MPEG4/ISO/ASP", "MPEG-4 Visual"),
        0x21 => ("V_MPEG4/ISO/AVC", "AVC/H.264"),
        0x23 => ("V_MPEGH/ISO/HEVC", "HEVC/H.265"),
        0x6C => ("V_MJPEG", "JPEG"),
        _ => return None,
    })
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
                pixel_dimensions: Some(
                    crate::media_metadata::model::track_properties_video::Dimensions2D {
                        width: 1920,
                        height: 1080,
                    },
                ),
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
        b
    }

    fn subtitle_builder(track_id: u32, codec: &str) -> super::super::moov::TrackBuilder {
        let mut b = super::super::moov::TrackBuilder::default();
        b.track_id = Some(track_id);
        b.codec_id_str = Some(codec.to_string());
        b.handler_type = Some(*b"subt");
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
        assert_eq!(
            m.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
            "eng"
        );
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
