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

//! Convert per-stream observations into protocol tracks.
//!
//! Classification precedence mirrors `r_mpeg_ps.cpp::found_new_stream`:
//! a Program Stream Map `stream_type` (PARSER-051) wins, then the
//! private-stream-1 substream id (PARSER-050), then the bare stream id.
//! Codec headers in the depacketised payload supply video dimensions, the
//! AVC-vs-MPEG distinction, and audio parameters (PARSER-052).

use crate::media_metadata::audio::{ac3, mp3};
use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::elementary::{avc, mpeg_video};
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;

/// A stream discovered during the start-code walk, with its depacketised
/// elementary payload for codec-header decoding.
#[derive(Debug, Clone)]
pub struct StreamObservation {
    pub stream_id: u8,
    pub sub_id: Option<u8>,
    pub psm_stream_type: Option<u8>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Codec {
    kind: TrackKind,
    id: &'static str,
    name: &'static str,
}

/// Program-Stream-Map `stream_type` → codec mapping.
fn codec_from_stream_type(stream_type: u8) -> Option<Codec> {
    Some(match stream_type {
        0x01 => Codec { kind: TrackKind::Video, id: "V_MPEG1", name: "MPEG-1 Video" },
        0x02 => Codec { kind: TrackKind::Video, id: "V_MPEG2", name: "MPEG-2 Video" },
        0x03 => Codec { kind: TrackKind::Audio, id: "A_MPEG/L2", name: "MPEG-1 Audio" },
        0x04 => Codec { kind: TrackKind::Audio, id: "A_MPEG/L2", name: "MPEG-2 Audio" },
        0x0F | 0x11 => Codec { kind: TrackKind::Audio, id: "A_AAC", name: "AAC" },
        0x10 => Codec { kind: TrackKind::Video, id: "V_MPEG4/ISO/ASP", name: "MPEG-4 Visual" },
        0x1B => Codec { kind: TrackKind::Video, id: "V_MPEG4/ISO/AVC", name: "AVC/H.264" },
        0x24 => Codec { kind: TrackKind::Video, id: "V_MPEGH/ISO/HEVC", name: "HEVC/H.265" },
        0x80 => Codec { kind: TrackKind::Audio, id: "A_PCM/INT/BIG", name: "LPCM" },
        0x81 => Codec { kind: TrackKind::Audio, id: "A_AC3", name: "AC-3" },
        0x82 => Codec { kind: TrackKind::Audio, id: "A_DTS", name: "DTS" },
        0x83 => Codec { kind: TrackKind::Audio, id: "A_TRUEHD", name: "TrueHD" },
        0x84 | 0x87 => Codec { kind: TrackKind::Audio, id: "A_EAC3", name: "E-AC-3" },
        _ => return None,
    })
}

/// `0xBD` private-stream-1 substream classification (PARSER-050).
///
/// PARSER-095: unknown sub-IDs are returned as `None`; mkvtoolnix sets
/// `track->type = '?'` and then drops the track instead of defaulting to AC-3
/// (see `r_mpeg_ps.cpp:1031-1033`).
fn codec_from_sub_id(sub_id: u8) -> Option<Codec> {
    Some(match sub_id {
        0x20..=0x3F => Codec { kind: TrackKind::Subtitle, id: "S_VOBSUB", name: "VobSub" },
        0x80..=0x87 | 0xC0..=0xC7 => Codec { kind: TrackKind::Audio, id: "A_AC3", name: "AC-3" },
        0x88..=0x9F => Codec { kind: TrackKind::Audio, id: "A_DTS", name: "DTS" },
        0xA0..=0xA7 => Codec { kind: TrackKind::Audio, id: "A_PCM/INT/BIG", name: "LPCM" },
        0xB0..=0xBF => Codec { kind: TrackKind::Audio, id: "A_TRUEHD", name: "TrueHD" },
        _ => return None,
    })
}

/// PARSER-094: stream id `0xFD` is VC-1 (mkvtoolnix `r_mpeg_ps.cpp:1042-1044`).
fn codec_from_bare_id(id: u8) -> Option<Codec> {
    match id {
        0xC0..=0xDF => Some(Codec { kind: TrackKind::Audio, id: "A_MPEG/L3", name: "MPEG-1/2 Audio" }),
        0xE0..=0xEF => Some(Codec { kind: TrackKind::Video, id: "V_MPEG2", name: "MPEG-2 Video" }),
        0xFD => Some(Codec { kind: TrackKind::Video, id: "V_VC1", name: "VC-1" }),
        _ => None,
    }
}

/// Backwards-compatible single-byte classification used by older callers/tests.
pub fn classify_stream_id(id: u8) -> Option<StreamObservation> {
    codec_from_bare_id(id)?;
    Some(StreamObservation {
        stream_id: id,
        sub_id: None,
        psm_stream_type: None,
        payload: Vec::new(),
    })
}

fn resolve_codec(obs: &StreamObservation) -> Option<Codec> {
    if let Some(st) = obs.psm_stream_type {
        if let Some(c) = codec_from_stream_type(st) {
            return Some(c);
        }
    }
    if obs.stream_id == 0xBD {
        return obs.sub_id.and_then(codec_from_sub_id);
    }
    codec_from_bare_id(obs.stream_id)
}

/// Decode codec headers from the depacketised payload (PARSER-052).
fn decode_payload(
    codec: &mut Codec,
    payload: &[u8],
) -> (Option<VideoTrackProperties>, Option<AudioTrackProperties>) {
    match codec.kind {
        TrackKind::Video => {
            // Prefer an AVC SPS when present; otherwise an MPEG sequence header.
            if let Some(sps) = first_avc_sps(payload) {
                codec.id = "V_MPEG4/ISO/AVC";
                codec.name = "AVC/H.264";
                let mut v = VideoTrackProperties::default();
                v.pixel_dimensions = Some(Dimensions2D {
                    width: sps.display_width,
                    height: sps.display_height,
                });
                return (Some(v), None);
            }
            if let Some(seq) = mpeg_video::decode_sequence_header(payload) {
                if seq.horizontal_size != 0 && seq.vertical_size != 0 {
                    let mut v = VideoTrackProperties::default();
                    v.pixel_dimensions = Some(Dimensions2D {
                        width: seq.horizontal_size,
                        height: seq.vertical_size,
                    });
                    return (Some(v), None);
                }
            }
            (Some(VideoTrackProperties::default()), None)
        }
        TrackKind::Audio => {
            let mut a = AudioTrackProperties::default();
            if matches!(codec.id, "A_AC3" | "A_EAC3") {
                if let Some(off) = ac3::find_frame_sync(payload) {
                    if let Some(f) = ac3::decode_frame(&payload[off..]) {
                        a.sampling_frequency = Some(f.sample_rate as f64);
                        a.channels = Some(f.channels);
                    }
                }
            } else if codec.id.starts_with("A_MPEG") {
                if let Some((_off, h)) = mp3::find_consecutive_mp3_headers(payload, 2) {
                    a.sampling_frequency = Some(h.sampling_frequency as f64);
                    a.channels = Some(h.channels);
                }
            }
            (None, Some(a))
        }
        _ => (None, None),
    }
}

fn first_avc_sps(payload: &[u8]) -> Option<avc::sps::AvcSps> {
    for nal in avc::nal::split_nal_units(payload) {
        if nal.nal_unit_type == 7 {
            let rbsp = avc::nal::strip_emulation_prevention(nal.payload);
            if let Ok(sps) = avc::sps::parse(&rbsp) {
                return Some(sps);
            }
        }
    }
    None
}

pub fn finalise(observations: Vec<StreamObservation>, out: &mut MediaMetadata) {
    out.container.format = ContainerFormat::MpegPs;
    out.container.recognized = true;
    out.container.supported = true;
    out.container.properties.is_fragmented = Some(false);

    let mut idx: i64 = 0;
    for obs in observations {
        let Some(mut codec) = resolve_codec(&obs) else {
            continue;
        };
        let (video, audio) = decode_payload(&mut codec, &obs.payload);
        let track_type = match codec.kind {
            TrackKind::Video => TrackType::Video,
            TrackKind::Audio => TrackType::Audio,
            TrackKind::Subtitle => TrackType::Subtitles,
            _ => continue,
        };
        let mut common = CommonTrackProperties::default();
        common.number = Some((idx as u64) + 1);
        common.stream_id = Some(obs.stream_id as u32);
        if let Some(sub) = obs.sub_id {
            common.sub_stream_id = Some(sub as u32);
        }
        let mut properties = TrackProperties {
            common,
            ..TrackProperties::default()
        };
        match track_type {
            TrackType::Video => properties.video = Some(video.unwrap_or_default()),
            TrackType::Audio => properties.audio = Some(audio.unwrap_or_default()),
            TrackType::Subtitles => {
                // PARSER-096: VobSub on private-stream-1 (sub-id 0x20..=0x3F).
                properties.subtitle = Some(SubtitleTrackProperties {
                    text_subtitles: false,
                    encoding: None,
                    variant: Some("VobSub".to_string()),
                    teletext_page: None,
                });
            }
            _ => {}
        }
        out.tracks.push(Track {
            id: idx,
            track_type,
            codec: CodecInfo {
                id: codec.id.to_string(),
                name: Some(codec.name.to_string()),
                codec_private: None,
            },
            properties,
        });
        idx += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(stream_id: u8, sub_id: Option<u8>, psm: Option<u8>) -> StreamObservation {
        StreamObservation { stream_id, sub_id, psm_stream_type: psm, payload: Vec::new() }
    }

    #[test]
    fn bare_video_and_audio_ids() {
        assert_eq!(resolve_codec(&obs(0xE0, None, None)).unwrap().kind, TrackKind::Video);
        assert_eq!(resolve_codec(&obs(0xC0, None, None)).unwrap().kind, TrackKind::Audio);
        assert!(resolve_codec(&obs(0x42, None, None)).is_none());
    }

    #[test]
    fn private_stream_1_substreams_classified() {
        assert_eq!(resolve_codec(&obs(0xBD, Some(0x20), None)).unwrap().id, "S_VOBSUB");
        assert_eq!(resolve_codec(&obs(0xBD, Some(0x80), None)).unwrap().id, "A_AC3");
        assert_eq!(resolve_codec(&obs(0xBD, Some(0x88), None)).unwrap().id, "A_DTS");
        assert_eq!(resolve_codec(&obs(0xBD, Some(0xA0), None)).unwrap().id, "A_PCM/INT/BIG");
        assert_eq!(resolve_codec(&obs(0xBD, Some(0xB1), None)).unwrap().id, "A_TRUEHD");
    }

    #[test]
    fn psm_stream_type_wins() {
        let c = resolve_codec(&obs(0xE0, None, Some(0x1B))).unwrap();
        assert_eq!(c.id, "V_MPEG4/ISO/AVC");
        let a = resolve_codec(&obs(0xC0, None, Some(0x81))).unwrap();
        assert_eq!(a.id, "A_AC3");
    }

    #[test]
    fn finalise_emits_tracks_and_sets_container() {
        let mut m = MediaMetadata::new("clip.mpg", 0);
        finalise(vec![obs(0xE0, None, None), obs(0xC0, None, None)], &mut m);
        assert_eq!(m.container.format, ContainerFormat::MpegPs);
        assert_eq!(m.tracks.len(), 2);
        assert_eq!(m.tracks[0].track_type, TrackType::Video);
        assert_eq!(m.tracks[1].track_type, TrackType::Audio);
        assert_eq!(m.tracks[0].properties.common.stream_id, Some(0xE0));
    }

    // ---- PARSER-094: VC-1 stream id 0xFD --------------------------------

    #[test]
    fn vc1_stream_id_classified_as_video() {
        let c = resolve_codec(&obs(0xFD, None, None)).unwrap();
        assert_eq!(c.id, "V_VC1");
        assert_eq!(c.kind, TrackKind::Video);
    }

    // ---- PARSER-095: unknown sub-IDs are dropped ------------------------

    #[test]
    fn unknown_private_substream_is_not_classified() {
        // 0x40 / 0x70 / 0xD0 are not in any documented sub-id range.
        assert!(resolve_codec(&obs(0xBD, Some(0x40), None)).is_none());
        assert!(resolve_codec(&obs(0xBD, Some(0x70), None)).is_none());
        assert!(resolve_codec(&obs(0xBD, Some(0xD0), None)).is_none());
    }

    // ---- PARSER-096: VobSub subtitle props ------------------------------

    #[test]
    fn vobsub_subtitle_track_has_subtitle_props() {
        let mut m = MediaMetadata::new("clip.vob", 0);
        finalise(vec![obs(0xBD, Some(0x20), None)], &mut m);
        assert_eq!(m.tracks.len(), 1);
        let t = &m.tracks[0];
        assert_eq!(t.track_type, TrackType::Subtitles);
        let sub = t.properties.subtitle.as_ref().unwrap();
        assert!(!sub.text_subtitles);
        assert_eq!(sub.variant.as_deref(), Some("VobSub"));
        assert_eq!(t.properties.common.stream_id, Some(0xBD));
        assert_eq!(t.properties.common.sub_stream_id, Some(0x20));
    }

    #[test]
    fn mpeg_video_dimensions_decoded() {
        // Sequence header: 0x000001B3 + 720x480.
        let mut payload = vec![0x00, 0x00, 0x01, 0xB3];
        payload.push(0x2D); // top 8 bits of horizontal_size (720 = 0x2D0)
        payload.push((0x0 << 4) | 0x1); // h low nibble + v high nibble (480 = 0x1E0)
        payload.push(0xE0); // v low byte
        payload.push(0x13); // aspect + frame-rate code
        payload.extend_from_slice(&[0u8; 4]);
        let mut m = MediaMetadata::new("c.mpg", 0);
        finalise(
            vec![StreamObservation {
                stream_id: 0xE0,
                sub_id: None,
                psm_stream_type: None,
                payload,
            }],
            &mut m,
        );
        let v = m.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions.unwrap().width, 720);
        assert_eq!(v.pixel_dimensions.unwrap().height, 480);
    }
}
