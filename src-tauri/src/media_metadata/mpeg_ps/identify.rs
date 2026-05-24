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

//! Convert per-stream-ID observations into protocol tracks.

use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;
use crate::media_metadata::model::MediaMetadata;

#[derive(Debug, Clone, Copy)]
pub struct StreamObservation {
    pub stream_id: u8,
    pub kind: TrackKind,
    pub codec_id: &'static str,
    pub codec_name: &'static str,
}

pub fn classify_stream_id(id: u8) -> Option<StreamObservation> {
    match id {
        0xBD => Some(StreamObservation {
            stream_id: id,
            kind: TrackKind::Audio,
            codec_id: "A_AC3",
            codec_name: "AC-3 (Private Stream 1)",
        }),
        0xC0..=0xDF => Some(StreamObservation {
            stream_id: id,
            kind: TrackKind::Audio,
            codec_id: "A_MPEG/L3",
            codec_name: "MPEG-1/2 Audio",
        }),
        0xE0..=0xEF => Some(StreamObservation {
            stream_id: id,
            kind: TrackKind::Video,
            codec_id: "V_MPEG2",
            codec_name: "MPEG-2 Video",
        }),
        _ => None,
    }
}

pub fn finalise(observations: Vec<StreamObservation>, out: &mut MediaMetadata) {
    out.container.format = ContainerFormat::MpegPs;
    out.container.recognized = true;
    out.container.supported = true;
    out.container.properties.is_fragmented = Some(false);

    for (idx, obs) in observations.into_iter().enumerate() {
        let track_type = match obs.kind {
            TrackKind::Video => TrackType::Video,
            TrackKind::Audio => TrackType::Audio,
            TrackKind::Subtitle => TrackType::Subtitles,
            _ => continue,
        };
        let mut common = CommonTrackProperties::default();
        common.number = Some((idx as u64) + 1);
        common.stream_id = Some(obs.stream_id as u32);
        let mut properties = TrackProperties {
            common,
            ..TrackProperties::default()
        };
        match track_type {
            TrackType::Video => properties.video = Some(VideoTrackProperties::default()),
            TrackType::Audio => properties.audio = Some(AudioTrackProperties::default()),
            _ => {}
        }
        out.tracks.push(Track {
            id: idx as i64,
            track_type,
            codec: CodecInfo {
                id: obs.codec_id.to_string(),
                name: Some(obs.codec_name.to_string()),
                codec_private: None,
            },
            properties,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_stream_ids_recognised() {
        for id in [0xE0u8, 0xE5, 0xEF] {
            assert_eq!(classify_stream_id(id).unwrap().kind, TrackKind::Video);
        }
    }

    #[test]
    fn audio_stream_ids_recognised() {
        for id in [0xC0u8, 0xC5, 0xDF] {
            assert_eq!(classify_stream_id(id).unwrap().kind, TrackKind::Audio);
        }
    }

    #[test]
    fn private_stream_1_classified_as_ac3() {
        let obs = classify_stream_id(0xBD).unwrap();
        assert_eq!(obs.codec_id, "A_AC3");
    }

    #[test]
    fn unrecognised_stream_id_returns_none() {
        assert!(classify_stream_id(0x42).is_none());
        assert!(classify_stream_id(0xBE).is_none());
    }

    #[test]
    fn finalise_emits_tracks_and_sets_container() {
        let mut m = MediaMetadata::new("clip.mpg", 0);
        finalise(
            vec![
                classify_stream_id(0xE0).unwrap(),
                classify_stream_id(0xC0).unwrap(),
            ],
            &mut m,
        );
        assert_eq!(m.container.format, ContainerFormat::MpegPs);
        assert_eq!(m.tracks.len(), 2);
        assert_eq!(m.tracks[0].track_type, TrackType::Video);
        assert_eq!(m.tracks[1].track_type, TrackType::Audio);
        assert_eq!(m.tracks[0].properties.common.stream_id, Some(0xE0));
    }
}
