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

//! FlvReader — walks tags until each declared stream has identification
//! state filled in (or until the 1 MiB detection window is exhausted).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::header::{FlvHeader, HEADER_LEN};
use super::script_data;
use super::tag::{AudioTagFlags, FlvTagHeader, VideoCodecId};

const DETECT_WINDOW: u64 = 1024 * 1024;

#[derive(Debug, Default, Clone)]
struct VideoState {
    codec: Option<VideoCodecId>,
    width: Option<u32>,
    height: Option<u32>,
    frame_rate: Option<f64>,
}

#[derive(Debug, Default, Clone)]
struct AudioState {
    codec_id: Option<&'static str>,
    codec_name: Option<&'static str>,
    sample_rate: Option<u32>,
    channels: Option<u32>,
    bit_depth: Option<u32>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FlvReader;

impl Reader for FlvReader {
    fn name(&self) -> &'static str {
        "flv"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = [0u8; HEADER_LEN];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        if read < HEADER_LEN {
            return Ok(false);
        }
        Ok(FlvHeader::parse(&buf).is_some())
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut buf = [0u8; HEADER_LEN];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut buf)?;
        if read < HEADER_LEN {
            return Err(ParseError::Unrecognised);
        }
        let header = FlvHeader::parse(&buf).ok_or(ParseError::Unrecognised)?;
        src.seek_to(header.data_offset as u64)?;

        out.container.format = ContainerFormat::Flv;
        out.container.recognized = true;
        out.container.supported = true;

        let mut video = VideoState::default();
        let mut audio = AudioState::default();
        let mut video_seen = false;
        let mut audio_seen = false;

        loop {
            deadline.check("flv-tag")?;
            let pos = src.position();
            if pos >= DETECT_WINDOW {
                break;
            }
            // Try to read the next tag header.  EOF here is a clean stop.
            let mut header_buf = [0u8; FlvTagHeader::TOTAL_LEN];
            match src.read_at_most(&mut header_buf)? {
                n if n < FlvTagHeader::TOTAL_LEN => break,
                _ => {}
            }
            let tag = match FlvTagHeader::parse(&header_buf) {
                Some(t) => t,
                None => break,
            };
            let payload_pos = src.position();
            if tag.is_audio() {
                audio_seen = true;
                read_audio_payload(src, tag.data_size, &mut audio)?;
            } else if tag.is_video() {
                video_seen = true;
                read_video_payload(src, tag.data_size, &mut video)?;
            } else if tag.is_script() {
                read_script_payload(src, tag.data_size, &mut video)?;
            }
            // Always seek to the byte just past this tag's payload.
            src.seek_to(payload_pos + tag.data_size as u64)?;
            // Stop early if both kinds have been observed and both have a codec.
            let video_done = !header.has_video() || video.codec.is_some();
            let audio_done = !header.has_audio() || audio.codec_id.is_some();
            if video_done && audio_done && (video_seen || audio_seen) {
                break;
            }
        }

        let mut track_id: i64 = 0;
        if header.has_video() && (video.codec.is_some() || video.width.is_some()) {
            let codec_info = match video.codec {
                Some(c) => CodecInfo {
                    id: c.codec_id().to_string(),
                    name: Some(c.display_name().to_string()),
                    codec_private: None,
                },
                None => CodecInfo {
                    id: "V_UNKNOWN".to_string(),
                    name: Some("Unknown".to_string()),
                    codec_private: None,
                },
            };
            let mut common = CommonTrackProperties::default();
            common.number = Some(1);
            let dims = video.width.zip(video.height).map(|(w, h)| Dimensions2D {
                width: w,
                height: h,
            });
            let mut vp = VideoTrackProperties {
                pixel_dimensions: dims,
                display_dimensions: dims,
                ..VideoTrackProperties::default()
            };
            if let Some(fps) = video.frame_rate {
                if fps > 0.0 {
                    vp.default_duration_ns =
                        Some((1_000_000_000.0 / fps).round() as u64);
                }
            }
            out.tracks.push(Track {
                id: track_id,
                track_type: TrackType::Video,
                codec: codec_info,
                properties: TrackProperties {
                    common,
                    video: Some(vp),
                    ..TrackProperties::default()
                },
            });
            track_id += 1;
        }

        if header.has_audio() && audio.codec_id.is_some() {
            let mut common = CommonTrackProperties::default();
            common.number = Some((track_id as u64) + 1);
            let ap = AudioTrackProperties {
                sampling_frequency: audio.sample_rate.map(|r| r as f64),
                channels: audio.channels,
                bit_depth: audio.bit_depth,
                ..AudioTrackProperties::default()
            };
            out.tracks.push(Track {
                id: track_id,
                track_type: TrackType::Audio,
                codec: CodecInfo {
                    id: audio.codec_id.unwrap_or("A_UNKNOWN").to_string(),
                    name: audio.codec_name.map(str::to_owned),
                    codec_private: None,
                },
                properties: TrackProperties {
                    common,
                    audio: Some(ap),
                    ..TrackProperties::default()
                },
            });
        }

        Ok(())
    }
}

fn read_audio_payload(
    src: &mut FileSource,
    data_size: u32,
    state: &mut AudioState,
) -> Result<(), ParseError> {
    if data_size == 0 {
        return Ok(());
    }
    let byte = src.read_u8()?;
    let flags = AudioTagFlags::parse(byte);
    if let Some((id, name)) = flags.codec() {
        state.codec_id.get_or_insert(audio_codec_matroska_id(id));
        state.codec_name.get_or_insert(name);
    }
    if state.sample_rate.is_none() {
        if let Some(rate) = flags.sample_rate() {
            state.sample_rate = Some(rate);
        }
    }
    if state.channels.is_none() {
        state.channels = Some(flags.channels());
    }
    if state.bit_depth.is_none() {
        state.bit_depth = Some(flags.bits_per_sample() as u32);
    }
    Ok(())
}

fn audio_codec_matroska_id(fourcc: &'static str) -> &'static str {
    match fourcc {
        "AAC " => "A_AAC",
        "MP3 " => "A_MPEG/L3",
        "SPEX" => "A_VORBIS", // closest match for Speex when no Matroska id exists
        "LPCM" | "PCMP" => "A_PCM/INT/LIT",
        _ => "A_UNKNOWN",
    }
}

fn read_video_payload(
    src: &mut FileSource,
    data_size: u32,
    state: &mut VideoState,
) -> Result<(), ParseError> {
    if data_size == 0 {
        return Ok(());
    }
    let header_byte = src.read_u8()?;
    let codec_id = VideoCodecId::from_byte(header_byte & 0x0F);
    if let Some(cid) = codec_id {
        state.codec.get_or_insert(cid);
    }
    Ok(())
}

fn read_script_payload(
    src: &mut FileSource,
    data_size: u32,
    state: &mut VideoState,
) -> Result<(), ParseError> {
    if data_size == 0 {
        return Ok(());
    }
    let mut bytes = vec![0u8; data_size as usize];
    src.read_exact(&mut bytes)?;
    let meta = script_data::parse(&bytes);
    if let Some(w) = meta.number("width") {
        state.width.get_or_insert(w as u32);
    }
    if let Some(h) = meta.number("height") {
        state.height.get_or_insert(h as u32);
    }
    if let Some(fps) = meta.number("framerate") {
        if state.frame_rate.is_none() && fps > 0.0 {
            state.frame_rate = Some(fps);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::flv::header::{build_header, TYPE_FLAG_AUDIO, TYPE_FLAG_VIDEO};
    use crate::media_metadata::flv::script_data::{build_on_meta_data, AmfValue};
    use crate::media_metadata::flv::tag::{TAG_AUDIO, TAG_SCRIPT, TAG_VIDEO};
    use std::io::Cursor;

    fn build_tag(tag_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes()); // previous_tag_size
        buf.push(tag_type);
        let len = payload.len() as u32;
        buf.push(((len >> 16) & 0xFF) as u8);
        buf.push(((len >> 8) & 0xFF) as u8);
        buf.push((len & 0xFF) as u8);
        buf.extend_from_slice(&[0u8; 3]); // timestamp
        buf.push(0u8); // timestamp_ext
        buf.extend_from_slice(&[0u8; 3]); // stream id
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn probe_accepts_minimal_flv_header() {
        let blob = build_header(1, TYPE_FLAG_VIDEO | TYPE_FLAG_AUDIO);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        assert!(FlvReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_other_magic() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"XYZ\x01\x05\x00\x00\x00\x09".to_vec()));
        assert!(!FlvReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_video_and_audio_tracks() {
        // FLV with one video tag (H.264) + one audio tag (AAC, 44.1k stereo)
        // + a script tag declaring dims and framerate.
        let mut blob = build_header(1, TYPE_FLAG_VIDEO | TYPE_FLAG_AUDIO);
        // Script tag with onMetaData
        let script_payload =
            build_on_meta_data(&[
                ("width", AmfValue::Number(1920.0)),
                ("height", AmfValue::Number(1080.0)),
                ("framerate", AmfValue::Number(30.0)),
            ]);
        blob.extend(build_tag(TAG_SCRIPT, &script_payload));
        // Video tag: byte = (key_frame<<4) | codec_id (7 = H.264)
        blob.extend(build_tag(TAG_VIDEO, &[(1 << 4) | 7, 0, 0, 0, 0]));
        // Audio tag: byte = (AAC<<4) | (44.1k<<2) | (16b<<1) | stereo
        let audio_byte = (10 << 4) | (3 << 2) | (1 << 1) | 1;
        blob.extend(build_tag(TAG_AUDIO, &[audio_byte, 0]));

        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.flv", 0);
        FlvReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .unwrap();
        assert_eq!(out.container.format, ContainerFormat::Flv);
        assert_eq!(out.tracks.len(), 2);
        let v = out.tracks.iter().find(|t| t.track_type == TrackType::Video).unwrap();
        assert_eq!(v.codec.id, "V_MPEG4/ISO/AVC");
        let vp = v.properties.video.as_ref().unwrap();
        assert_eq!(vp.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
        assert_eq!(vp.default_duration_ns, Some(33_333_333));

        let a = out.tracks.iter().find(|t| t.track_type == TrackType::Audio).unwrap();
        assert_eq!(a.codec.id, "A_AAC");
        let ap = a.properties.audio.as_ref().unwrap();
        assert_eq!(ap.sampling_frequency, Some(44_100.0));
        assert_eq!(ap.channels, Some(2));
        assert_eq!(ap.bit_depth, Some(16));
    }

    #[test]
    fn read_headers_handles_audio_only_files() {
        let mut blob = build_header(1, TYPE_FLAG_AUDIO);
        let audio_byte = (2 << 4) | (3 << 2);
        blob.extend(build_tag(TAG_AUDIO, &[audio_byte, 0]));
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("audio.flv", 0);
        FlvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].codec.id, "A_MPEG/L3");
    }

    #[test]
    fn read_headers_returns_no_tracks_when_payload_is_empty() {
        let blob = build_header(1, 0); // neither flag set
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("empty.flv", 0);
        FlvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Flv);
        assert!(out.tracks.is_empty());
    }

    #[test]
    fn read_headers_rejects_invalid_header() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 32]));
        let mut out = MediaMetadata::new("not-flv", 0);
        let err = FlvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn read_headers_recognises_h265_video_tag() {
        let mut blob = build_header(1, TYPE_FLAG_VIDEO);
        // Video tag: byte = (key_frame<<4) | codec_id (12 = H.265)
        blob.extend(build_tag(TAG_VIDEO, &[(1 << 4) | 12, 0, 0, 0, 0]));
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.flv", 0);
        FlvReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        let v = out.tracks.iter().find(|t| t.track_type == TrackType::Video).unwrap();
        assert_eq!(v.codec.id, "V_MPEGH/ISO/HEVC");
    }
}
