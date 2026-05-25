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
    AviStreamKind, BitmapInfoHeader, StreamBuilder, StreamFormat, VideoPropertiesHeader,
    WaveFormatEx,
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
                let video = video_properties(&bmih, &header, builder.vprp.as_ref());
                // PARSER-085: surface `BITMAPINFOHEADER + extradata` as the
                // video codec_private blob.  Mirrors mkvtoolnix's
                // `r_avi.cpp:188-206` storage of the whole bmih + extra bytes.
                let private = Some(CodecPrivate::from_bytes(&bmih_codec_private(&bmih)));
                (codec_id, codec_name, private, Some(video), None, None)
            }
            (TrackType::Audio, Some(StreamFormat::Audio(wf))) => {
                // WAVE_FORMAT_EXTENSIBLE (0xFFFE) carries the real format tag in
                // the SubFormat GUID's data1 field (PARSER-059): the 18-byte
                // WAVEFORMATEX is followed by wValidBitsPerSample(2) +
                // dwChannelMask(4) + GUID, so data1 sits at extra offset 6.
                let effective_tag = if wf.format_tag == 0xFFFE && wf.extra.len() >= 10 {
                    u16::from_le_bytes([wf.extra[6], wf.extra[7]])
                } else {
                    wf.format_tag
                };
                let codec_id = format!("0x{:04X}", effective_tag);
                let codec_name = name_from_wave_tag(effective_tag);
                let private = if wf.extra.is_empty() {
                    None
                } else {
                    Some(CodecPrivate::from_bytes(&wf.extra))
                };
                let audio = audio_properties(&wf);
                (codec_id, codec_name, private, None, Some(audio), None)
            }
            (TrackType::Subtitles, fmt) => {
                // VirtualDub embeds SRT/SSA subtitles as a GAB2 block in the
                // text stream's strf (PARSER-060).
                let (codec_id, variant) = fmt
                    .as_ref()
                    .and_then(gab2_codec)
                    .map(|(id, name)| (id.to_string(), name.to_string()))
                    .unwrap_or_else(|| {
                        (fourcc_to_string(&header.fcc_handler), "AVI Text".to_string())
                    });
                let subtitle = Some(SubtitleTrackProperties {
                    text_subtitles: true,
                    encoding: None,
                    variant: Some(variant),
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

fn video_properties(
    bmih: &BitmapInfoHeader,
    header: &super::strl::StreamHeader,
    vprp: Option<&VideoPropertiesHeader>,
) -> VideoTrackProperties {
    let width = bmih.width.max(0) as u32;
    let height = bmih.height.unsigned_abs(); // height may be negative ⇒ top-down DIB
    let pixel = if width > 0 && height > 0 {
        Some(Dimensions2D { width, height })
    } else {
        None
    };
    // PARSER-084: when `vprp` carries a frame aspect ratio, derive display
    // dimensions from it instead of mirroring the pixel dimensions.  Mirrors
    // `avi_reader_c::handle_video_aspect_ratio` (`r_avi.cpp:241-273`).
    let display = match (pixel, vprp) {
        (Some(p), Some(v))
            if v.frame_aspect_ratio_x != 0 && v.frame_aspect_ratio_y != 0 && p.width != 0 && p.height != 0 =>
        {
            Some(display_dimensions_from_aspect(p, v))
        }
        _ => pixel,
    };
    let default_duration_ns = header.frame_duration_ns();
    let mut props = VideoTrackProperties {
        pixel_dimensions: pixel,
        display_dimensions: display,
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

fn display_dimensions_from_aspect(
    pixel: Dimensions2D,
    vprp: &VideoPropertiesHeader,
) -> Dimensions2D {
    let x = vprp.frame_aspect_ratio_x as u64;
    let y = vprp.frame_aspect_ratio_y as u64;
    let pw = pixel.width as u64;
    let ph = pixel.height as u64;
    // Compare aspect ratio (x/y) with pixel ratio (pw/ph) via cross multiply.
    if x * ph >= y * pw {
        Dimensions2D {
            width: ((x * ph + y / 2) / y) as u32,
            height: pixel.height,
        }
    } else {
        Dimensions2D {
            width: pixel.width,
            height: ((y * pw + x / 2) / x) as u32,
        }
    }
}

/// Serialise the BITMAPINFOHEADER + trailing extradata into a contiguous
/// 40+N-byte blob (PARSER-085).  Mirrors mkvtoolnix's
/// `r_avi.cpp:188-206` codec_private layout.
fn bmih_codec_private(bmih: &BitmapInfoHeader) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40 + bmih.extra.len());
    bytes.extend_from_slice(&bmih.size.to_le_bytes());
    bytes.extend_from_slice(&bmih.width.to_le_bytes());
    bytes.extend_from_slice(&bmih.height.to_le_bytes());
    bytes.extend_from_slice(&bmih.planes.to_le_bytes());
    bytes.extend_from_slice(&bmih.bit_count.to_le_bytes());
    bytes.extend_from_slice(&bmih.compression);
    bytes.extend_from_slice(&bmih.image_size.to_le_bytes());
    // The remaining 16 bytes of the standard BITMAPINFOHEADER (x/y ppm,
    // colors used/important) are stored as zeros — we never decode them and
    // mkvtoolnix's packetizers don't depend on the values.
    bytes.extend_from_slice(&[0u8; 16]);
    bytes.extend_from_slice(&bmih.extra);
    bytes
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

/// Detect a GAB2 subtitle block (VirtualDub) in a text stream's strf and
/// classify it as SRT or SSA/ASS (PARSER-060 / PARSER-087).  The block is
/// `"GAB2\0"` followed by `(u16 type, u32 size, data)` entries; type 4 carries
/// the subtitle file content.  PARSER-087 routes the payload through the
/// canonical SRT / SSA text probers instead of the previous hand-rolled string
/// search so character-set handling and edge cases match the standalone
/// subtitle readers.
fn gab2_codec(fmt: &StreamFormat) -> Option<(&'static str, &'static str)> {
    let StreamFormat::Other(bytes) = fmt else {
        return None;
    };
    if bytes.len() < 5 || &bytes[0..4] != b"GAB2" {
        return None;
    }
    let mut pos = 5usize; // skip "GAB2\0"
    while pos + 6 <= bytes.len() {
        let block_type = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
        let size =
            u32::from_le_bytes([bytes[pos + 2], bytes[pos + 3], bytes[pos + 4], bytes[pos + 5]])
                as usize;
        let data_start = pos + 6;
        let data_end = (data_start + size).min(bytes.len());
        if block_type == 4 {
            let data = &bytes[data_start..data_end];
            let text = crate::media_metadata::subtitles::encoding::decode_lossy(data);
            if crate::media_metadata::subtitles::ssa::classify(&text).is_some() {
                return Some(("S_TEXT/ASS", "SSA/ASS (GAB2)"));
            }
            if crate::media_metadata::subtitles::srt::has_srt_timecode_line(&text) {
                return Some(("S_TEXT/UTF8", "SRT (GAB2)"));
            }
            return Some(("S_TEXT/UTF8", "GAB2 Subtitle"));
        }
        pos = data_end;
    }
    None
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
            extra: Vec::new(),
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
            vprp: None,
        };
        let track = make_track(0, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Video);
        assert_eq!(track.codec.id, "H264");
        let v = track.properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
        assert_eq!(v.default_duration_ns, Some(41_708_333));
    }

    // ---- PARSER-084: vprp display aspect ratio ---------------------------

    #[test]
    fn vprp_sixteen_nine_aspect_expands_display_width() {
        // 1440x1080 raster with vprp 16:9 → display 1920x1080.
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1440,
            height: 1080,
            planes: 1,
            bit_count: 24,
            compression: *b"H264",
            image_size: 0,
            extra: Vec::new(),
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
            vprp: Some(VideoPropertiesHeader {
                frame_aspect_ratio_x: 16,
                frame_aspect_ratio_y: 9,
                frame_width_in_pixels: 1440,
                frame_height_in_lines: 1080,
            }),
        };
        let track = make_track(0, builder).unwrap();
        let v = track.properties.video.unwrap();
        assert_eq!(
            v.display_dimensions,
            Some(Dimensions2D { width: 1920, height: 1080 })
        );
    }

    #[test]
    fn vprp_invalid_zero_aspect_is_ignored() {
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1920,
            height: 1080,
            planes: 1,
            bit_count: 24,
            compression: *b"H264",
            image_size: 0,
            extra: Vec::new(),
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
            vprp: Some(VideoPropertiesHeader {
                frame_aspect_ratio_x: 0,
                frame_aspect_ratio_y: 0,
                frame_width_in_pixels: 0,
                frame_height_in_lines: 0,
            }),
        };
        let track = make_track(0, builder).unwrap();
        let v = track.properties.video.unwrap();
        assert_eq!(
            v.display_dimensions,
            Some(Dimensions2D { width: 1920, height: 1080 })
        );
    }

    // ---- PARSER-085: BITMAPINFOHEADER + extradata as codec_private -----

    #[test]
    fn video_extradata_is_stored_in_codec_private() {
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1920,
            height: 1080,
            planes: 1,
            bit_count: 24,
            compression: *b"XVID",
            image_size: 0,
            extra: vec![0x01, 0x02, 0x03, 0x04],
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
            vprp: None,
        };
        let track = make_track(0, builder).unwrap();
        let private = track.codec.codec_private.unwrap();
        assert_eq!(private.length, 44);
        // First 4 bytes are the BMIH `size` (40) in LE.
        assert!(private.hex.starts_with("28000000"));
        // Last 8 chars are the 4 extradata bytes.
        assert!(private.hex.ends_with("01020304"));
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
            extra: Vec::new(),
        };
        let builder = StreamBuilder {
            header: Some(dummy_video_header()),
            format: Some(StreamFormat::Video(bmih)),
            name: None,
            private: None,
            vprp: None,
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
            vprp: None,
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
            vprp: None,
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
            vprp: None,
        };
        let track = make_track(2, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Subtitles);
        let sub = track.properties.subtitle.unwrap();
        assert!(sub.text_subtitles);
        assert_eq!(track.properties.common.track_name.as_deref(), Some("English"));
    }

    // ---- PARSER-059: WAVEFORMATEXTENSIBLE resolution ---------------------

    #[test]
    fn extensible_audio_resolves_subformat_tag() {
        // wValidBitsPerSample(2) + dwChannelMask(4) + GUID(16); data1 = 0x0001.
        let mut extra = Vec::new();
        extra.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        extra.extend_from_slice(&3u32.to_le_bytes()); // channel mask
        extra.extend_from_slice(&1u32.to_le_bytes()); // GUID data1 = PCM
        extra.extend_from_slice(&[0u8; 12]); // rest of GUID
        let wf = WaveFormatEx {
            format_tag: 0xFFFE,
            channels: 2,
            samples_per_sec: 48000,
            avg_bytes_per_sec: 192000,
            block_align: 4,
            bits_per_sample: 16,
            extra,
        };
        let builder = StreamBuilder {
            header: Some(dummy_audio_header()),
            format: Some(StreamFormat::Audio(wf)),
            name: None,
            private: None,
            vprp: None,
        };
        let track = make_track(1, builder).unwrap();
        assert_eq!(track.codec.id, "0x0001");
        assert_eq!(track.codec.name.as_deref(), Some("PCM"));
    }

    // ---- PARSER-060: GAB2 embedded subtitles -----------------------------

    #[test]
    fn gab2_srt_subtitle_classified() {
        let srt = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHello\r\n";
        let mut gab2 = b"GAB2\0".to_vec();
        // filename block (type 2)
        gab2.extend_from_slice(&2u16.to_le_bytes());
        gab2.extend_from_slice(&4u32.to_le_bytes());
        gab2.extend_from_slice(b"a.sr");
        // subtitle data block (type 4)
        gab2.extend_from_slice(&4u16.to_le_bytes());
        gab2.extend_from_slice(&(srt.len() as u32).to_le_bytes());
        gab2.extend_from_slice(srt);
        let builder = StreamBuilder {
            header: Some(dummy_text_header()),
            format: Some(StreamFormat::Other(gab2)),
            name: None,
            private: None,
            vprp: None,
        };
        let track = make_track(2, builder).unwrap();
        assert_eq!(track.track_type, TrackType::Subtitles);
        assert_eq!(track.codec.id, "S_TEXT/UTF8");
        assert_eq!(
            track.properties.subtitle.unwrap().variant.as_deref(),
            Some("SRT (GAB2)")
        );
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
            vprp: None,
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
