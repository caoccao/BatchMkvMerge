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

//! Convert the parsed AVI builders into protocol-level `Track`s.

use crate::media_metadata::codec::fourcc;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::container::ContainerProperties;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;

use super::avih::MainAviHeader;
use super::odml::OdmlInfo;
use super::strl::{
    AviStreamKind, BitmapInfoHeader, StreamBuilder, StreamFormat, WaveFormatEx,
};

/// Drive the per-stream conversion and stamp container fields.
pub fn finalise(
    avih: Option<MainAviHeader>,
    streams: Vec<StreamBuilder>,
    odml: OdmlInfo,
    out: &mut MediaMetadata,
) {
    if let Some(avih) = avih {
        set_container_props(&avih, odml, &mut out.container.properties);
    }
    for (idx, builder) in streams.into_iter().enumerate() {
        if let Some(track) = make_track(idx as i64, builder) {
            out.tracks.push(track);
        }
    }
    out.tags.per_track_count = out
        .tracks
        .iter()
        .map(|t| t.properties.tags.len() as u32)
        .sum();
}

fn set_container_props(
    avih: &MainAviHeader,
    odml: OdmlInfo,
    props: &mut ContainerProperties,
) {
    props.bitrate_bps = avih.average_bitrate_bps();
    // Derive total file duration from frame count × per-frame microseconds.
    let frames = odml.total_frames.unwrap_or(avih.total_frames);
    if frames > 0 && avih.microsec_per_frame > 0 {
        let ns: u64 = (frames as u64).saturating_mul(avih.microsec_per_frame as u64) * 1000;
        props.duration = Some(
            crate::media_metadata::model::duration::DurationValue::from_ns(ns),
        );
    }
    props.is_fragmented = Some(false);
}

fn make_track(id: i64, builder: StreamBuilder) -> Option<Track> {
    let header = builder.header?;
    let track_type = match header.kind {
        AviStreamKind::Video => TrackType::Video,
        AviStreamKind::Audio => TrackType::Audio,
        AviStreamKind::Text => TrackType::Subtitles,
        _ => return None,
    };

    let (codec_id, codec_name, codec_private, video, audio, subtitle) =
        match (track_type, builder.format) {
            (TrackType::Video, Some(StreamFormat::Video(bmih))) => {
                let codec_id = fourcc_to_string(&bmih.compression);
                let codec_name = fourcc::lookup(&codec_id).map(|e| e.name.to_string());
                let video = video_properties(&bmih, &header);
                (codec_id, codec_name, None, Some(video), None, None)
            }
            (TrackType::Audio, Some(StreamFormat::Audio(wf))) => {
                let codec_id = format!("0x{:04X}", wf.format_tag);
                let codec_name = name_from_wave_tag(wf.format_tag);
                let private = if wf.extra.is_empty() {
                    None
                } else {
                    Some(CodecPrivate::from_bytes(&wf.extra))
                };
                let audio = audio_properties(&wf);
                (codec_id, codec_name, private, None, Some(audio), None)
            }
            (TrackType::Subtitles, _) => {
                let codec_id = fourcc_to_string(&header.fcc_handler);
                let subtitle = Some(SubtitleTrackProperties {
                    text_subtitles: true,
                    encoding: None,
                    variant: Some("AVI Text".to_string()),
                    teletext_page: None,
                });
                (codec_id, None, None, None, None, subtitle)
            }
            _ => return None,
        };

    let mut common = CommonTrackProperties::default();
    common.number = Some(id as u64 + 1);
    common.track_name = builder.name.clone();
    if header.length > 0 {
        common.num_index_entries = Some(header.length as u64);
    }
    if header.language != 0 {
        // AVI's language is an LCID — we don't have a full mapping table.
        // Surface the raw value when present.
        common.language = Some(Language::resolve(
            None,
            Some(&format!("lcid-{}", header.language)),
            false,
        ));
    }

    Some(Track {
        id,
        track_type,
        codec: CodecInfo {
            id: codec_id,
            name: codec_name,
            codec_private,
        },
        properties: TrackProperties {
            common,
            video,
            audio,
            subtitle,
            ..TrackProperties::default()
        },
    })
}

fn video_properties(bmih: &BitmapInfoHeader, header: &super::strl::StreamHeader) -> VideoTrackProperties {
    let width = bmih.width.max(0) as u32;
    let height = bmih.height.unsigned_abs(); // height may be negative ⇒ top-down DIB
    let pixel = if width > 0 && height > 0 {
        Some(Dimensions2D { width, height })
    } else {
        None
    };
    let default_duration_ns = header.frame_duration_ns();
    let mut props = VideoTrackProperties {
        pixel_dimensions: pixel,
        display_dimensions: pixel,
        default_duration_ns,
        ..VideoTrackProperties::default()
    };
    if bmih.bit_count != 0 && bmih.bit_count != 24 {
        let mut color =
            crate::media_metadata::model::track_properties_video::ColorMetadata::default();
        color.bits_per_channel = Some(bmih.bit_count as u32);
        props.color = Some(color);
    }
    props
}

fn audio_properties(wf: &WaveFormatEx) -> AudioTrackProperties {
    AudioTrackProperties {
        channels: if wf.channels == 0 { None } else { Some(wf.channels as u32) },
        sampling_frequency: if wf.samples_per_sec == 0 {
            None
        } else {
            Some(wf.samples_per_sec as f64)
        },
        bit_depth: if wf.bits_per_sample == 0 {
            None
        } else {
            Some(wf.bits_per_sample as u32)
        },
        ..AudioTrackProperties::default()
    }
}

fn fourcc_to_string(bytes: &[u8; 4]) -> String {
    bytes
        .iter()
        .map(|b| {
            if (0x20..=0x7E).contains(b) {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

fn name_from_wave_tag(tag: u16) -> Option<String> {
    let name = match tag {
        0x0001 => "PCM",
        0x0002 => "ADPCM",
        0x0003 => "PCM (Float)",
        0x0050 => "MPEG Audio Layer 1/2",
        0x0055 => "MP3",
        0x00FF => "AAC",
        0x0161 => "WMA v2",
        0x0162 => "WMA Pro",
        0x0163 => "WMA Lossless",
        0x2000 => "AC-3",
        0x2001 => "DTS",
        0x6771 => "Vorbis",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::avi::strl::StreamHeader;

    fn dummy_video_header() -> StreamHeader {
        StreamHeader {
            kind: AviStreamKind::Video,
            fcc_handler: *b"H264",
            flags: 0,
            priority: 0,
            language: 0,
            scale: 1001,
            rate: 24000,
            start: 0,
            length: 240,
            sample_size: 0,
        }
    }

    fn dummy_audio_header() -> StreamHeader {
        StreamHeader {
            kind: AviStreamKind::Audio,
            fcc_handler: [0; 4],
            flags: 0,
            priority: 0,
            language: 0,
            scale: 1,
            rate: 48000,
            start: 0,
            length: 0,
            sample_size: 4,
        }
    }

    fn dummy_text_header() -> StreamHeader {
        StreamHeader {
            kind: AviStreamKind::Text,
            fcc_handler: *b"DXSA",
            flags: 0,
            priority: 0,
            language: 0,
            scale: 1,
            rate: 1000,
            start: 0,
            length: 0,
            sample_size: 0,
        }
    }

    #[test]
    fn video_track_emitted_with_dims_and_duration() {
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1920,
            height: 1080,
            planes: 1,
            bit_count: 24,
            compression: *b"H264",
            image_size: 0,
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
        };
        let track = make_track(0, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Video);
        assert_eq!(track.codec.id, "H264");
        let v = track.properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
        assert_eq!(v.default_duration_ns, Some(41_708_333));
    }

    #[test]
    fn negative_height_top_down_dib_is_flipped_positive() {
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1920,
            height: -1080,
            planes: 1,
            bit_count: 24,
            compression: *b"H264",
            image_size: 0,
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
        };
        let track = make_track(0, builder).unwrap();
        let v = track.properties.video.unwrap();
        assert_eq!(v.pixel_dimensions.unwrap().height, 1080);
    }

    #[test]
    fn audio_track_emitted_with_channels_and_rate() {
        let wf = WaveFormatEx {
            format_tag: 0x0055,
            channels: 2,
            samples_per_sec: 48000,
            avg_bytes_per_sec: 16000,
            block_align: 4,
            bits_per_sample: 16,
            extra: Vec::new(),
        };
        let builder = StreamBuilder {
            header: Some(dummy_audio_header()),
            format: Some(StreamFormat::Audio(wf)),
            name: None,
            private: None,
        };
        let track = make_track(1, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Audio);
        assert_eq!(track.codec.id, "0x0055");
        assert_eq!(track.codec.name.as_deref(), Some("MP3"));
        let a = track.properties.audio.unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.sampling_frequency, Some(48000.0));
        assert_eq!(a.bit_depth, Some(16));
    }

    #[test]
    fn audio_extra_bytes_become_codec_private() {
        let wf = WaveFormatEx {
            format_tag: 0x00FF,
            channels: 2,
            samples_per_sec: 48000,
            avg_bytes_per_sec: 16000,
            block_align: 4,
            bits_per_sample: 16,
            extra: vec![0x11, 0x90],
        };
        let builder = StreamBuilder {
            header: Some(dummy_audio_header()),
            format: Some(StreamFormat::Audio(wf)),
            name: None,
            private: None,
        };
        let track = make_track(1, builder).unwrap();
        let private = track.codec.codec_private.unwrap();
        assert_eq!(private.length, 2);
    }

    #[test]
    fn text_stream_becomes_subtitle_track() {
        let builder = StreamBuilder {
            header: Some(dummy_text_header()),
            format: None,
            name: Some("English".to_string()),
            private: None,
        };
        let track = make_track(2, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Subtitles);
        let sub = track.properties.subtitle.unwrap();
        assert!(sub.text_subtitles);
        assert_eq!(track.properties.common.track_name.as_deref(), Some("English"));
    }

    #[test]
    fn missing_header_drops_track() {
        let builder = StreamBuilder::default();
        assert!(make_track(0, builder).is_none());
    }

    #[test]
    fn unknown_kind_returns_none() {
        let mut header = dummy_video_header();
        header.kind = AviStreamKind::Midi;
        let builder = StreamBuilder {
            header: Some(header),
            format: None,
            name: None,
            private: None,
        };
        assert!(make_track(0, builder).is_none());
    }

    #[test]
    fn name_from_wave_tag_table() {
        assert_eq!(name_from_wave_tag(0x0001).as_deref(), Some("PCM"));
        assert_eq!(name_from_wave_tag(0x0055).as_deref(), Some("MP3"));
        assert_eq!(name_from_wave_tag(0x00FF).as_deref(), Some("AAC"));
        assert_eq!(name_from_wave_tag(0x2000).as_deref(), Some("AC-3"));
        assert_eq!(name_from_wave_tag(0x6771).as_deref(), Some("Vorbis"));
        assert!(name_from_wave_tag(0xFFFF).is_none());
    }

    #[test]
    fn finalise_sets_container_duration_from_avih() {
        let avih = MainAviHeader {
            microsec_per_frame: 41_708,
            max_bytes_per_sec: 0,
            flags: 0,
            total_frames: 240,
            initial_frames: 0,
            streams: 2,
            width: 1920,
            height: 1080,
        };
        let mut m = MediaMetadata::new("clip.avi", 0);
        finalise(Some(avih), vec![], OdmlInfo::default(), &mut m);
        assert!(m.container.properties.duration.is_some());
        assert_eq!(m.container.properties.is_fragmented, Some(false));
    }

    #[test]
    fn finalise_uses_odml_frame_count_when_set() {
        let avih = MainAviHeader {
            microsec_per_frame: 41_708,
            max_bytes_per_sec: 0,
            flags: 0,
            total_frames: 0,
            initial_frames: 0,
            streams: 0,
            width: 0,
            height: 0,
        };
        let odml = OdmlInfo {
            total_frames: Some(1000),
        };
        let mut m = MediaMetadata::new("clip.avi", 0);
        finalise(Some(avih), vec![], odml, &mut m);
        assert!(m.container.properties.duration.is_some());
    }
}
