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

//! AAC ADTS reader.
//!
//! ADTS frame header (ISO/IEC 13818-7 §6.2):
//!
//! ```text
//! 12 bits sync (FFF)
//! 1  bit  MPEG version (0=MPEG-4, 1=MPEG-2)
//! 2  bits layer (== 00)
//! 1  bit  protection_absent
//! 2  bits profile (object_type - 1)
//! 4  bits sampling_frequency_index
//! 1  bit  private
//! 3  bits channel_configuration
//! 1  bit  original_copy
//! 1  bit  home
//! 1  bit  copyright_identification_bit
//! 1  bit  copyright_identification_start
//! 13 bits frame_length (including header)
//! 11 bits buffer_fullness
//! 2  bits number_of_raw_data_blocks
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;
const MIN_CONFIRM_FRAMES: usize = 8;

const SAMPLE_RATE_TABLE: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

#[derive(Debug, Clone, Copy)]
pub struct AdtsHeader {
    pub mpeg_version: u8, // 0 = MPEG-4, 1 = MPEG-2
    pub profile: u8,
    pub sample_rate: u32,
    pub channels: u32,
    pub frame_length: usize,
}

pub fn decode_adts(bytes: &[u8]) -> Option<AdtsHeader> {
    if bytes.len() < 7 {
        return None;
    }
    if bytes[0] != 0xFF || (bytes[1] & 0xF0) != 0xF0 {
        return None;
    }
    if (bytes[1] & 0x06) != 0 {
        return None; // layer must be 00
    }
    let mpeg_version = (bytes[1] >> 3) & 0x01;
    let profile = (bytes[2] >> 6) & 0x03;
    let sr_index = ((bytes[2] >> 2) & 0x0F) as usize;
    if sr_index >= SAMPLE_RATE_TABLE.len() {
        return None;
    }
    let channel_config = (((bytes[2] & 0x01) << 2) | ((bytes[3] >> 6) & 0x03)) as u32;
    if channel_config == 0 {
        return None;
    }
    let frame_length = (((bytes[3] as usize) & 0x03) << 11)
        | ((bytes[4] as usize) << 3)
        | ((bytes[5] as usize) >> 5);
    if frame_length < 7 {
        return None;
    }
    let channels = match channel_config {
        7 => 8,
        c => c,
    };
    Some(AdtsHeader {
        mpeg_version,
        profile,
        sample_rate: SAMPLE_RATE_TABLE[sr_index],
        channels,
        frame_length,
    })
}

pub fn find_adts_sync(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 7 <= bytes.len() {
        if let Some(h) = decode_adts(&bytes[i..]) {
            let mut hits = 1usize;
            let mut next = i + h.frame_length.max(7);
            while hits < MIN_CONFIRM_FRAMES && next + 7 <= bytes.len() {
                let Some(nh) = decode_adts(&bytes[next..]) else {
                    break;
                };
                hits += 1;
                next += nh.frame_length.max(7);
            }
            if hits >= MIN_CONFIRM_FRAMES {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AacReader;

impl Reader for AacReader {
    fn name(&self) -> &'static str {
        "aac"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < 7 {
            return Ok(false);
        }
        let (start, _end) = id3v2::payload_bounds(&probe[..read]);
        Ok(find_adts_sync(&probe[start..read]).is_some())
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut probe)?;
        let (start, _end) = id3v2::payload_bounds(&probe[..read]);
        let bytes = &probe[start..read];
        let offset = find_adts_sync(bytes).ok_or(ParseError::Unrecognised)?;
        let header = decode_adts(&bytes[offset..]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Aac;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let codec_config = AudioCodecConfig {
            aac_object_type: Some(header.profile as u32 + 1),
            aac_frame_length: Some(1024),
            ..AudioCodecConfig::default()
        };
        let audio = AudioTrackProperties {
            channels: Some(header.channels),
            sampling_frequency: Some(header.sample_rate as f64),
            codec_config: Some(codec_config),
            ..AudioTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_AAC".to_string(),
                name: Some(format_aac_profile(header.profile)),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                audio: Some(audio),
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

fn format_aac_profile(profile: u8) -> String {
    match profile {
        0 => "AAC Main",
        1 => "AAC LC",
        2 => "AAC SSR",
        3 => "AAC LTP",
        _ => "AAC",
    }
    .to_string()
}

#[cfg(test)]
pub(crate) fn build_adts_frame(profile: u8, sr_index: u8, channel_config: u8) -> Vec<u8> {
    // 7-byte ADTS header + 1 byte body so frame_length = 8
    let frame_length: u16 = 8;
    let mut bytes = vec![0u8; 8];
    bytes[0] = 0xFF;
    bytes[1] = 0xF1; // sync + MPEG-4 + layer 0 + protection_absent
    bytes[2] = (profile << 6) | (sr_index << 2) | ((channel_config >> 2) & 0x01);
    bytes[3] = ((channel_config & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03);
    bytes[4] = ((frame_length >> 3) & 0xFF) as u8;
    bytes[5] = (((frame_length & 0x07) << 5) | 0x1F) as u8;
    bytes[6] = 0xFC;
    bytes
}

#[cfg(test)]
pub(crate) fn build_adts_stream(frames: usize, profile: u8, sr_index: u8, ch: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..frames {
        bytes.extend(build_adts_frame(profile, sr_index, ch));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decode_adts_handles_lc_48k_stereo() {
        let frame = build_adts_frame(1, 3, 2);
        let h = decode_adts(&frame).unwrap();
        assert_eq!(h.profile, 1);
        assert_eq!(h.sample_rate, 48_000);
        assert_eq!(h.channels, 2);
        assert_eq!(h.frame_length, 8);
    }

    #[test]
    fn decode_adts_handles_71_layout_via_channel_config_7() {
        let frame = build_adts_frame(1, 3, 7);
        let h = decode_adts(&frame).unwrap();
        assert_eq!(h.channels, 8);
    }

    #[test]
    fn decode_adts_rejects_invalid_sync() {
        let mut frame = build_adts_frame(1, 3, 2);
        frame[0] = 0xFE;
        assert!(decode_adts(&frame).is_none());
    }

    #[test]
    fn decode_adts_rejects_layer_nonzero() {
        let mut frame = build_adts_frame(1, 3, 2);
        frame[1] |= 0x04; // set layer bit
        assert!(decode_adts(&frame).is_none());
    }

    #[test]
    fn decode_adts_rejects_invalid_sr_index() {
        let mut frame = build_adts_frame(1, 3, 2);
        frame[2] |= 0b0011_1100; // sr_index 13..15 invalid
        assert!(decode_adts(&frame).is_none());
    }

    #[test]
    fn decode_adts_rejects_channel_config_zero() {
        let frame = build_adts_frame(1, 3, 0);
        assert!(decode_adts(&frame).is_none());
    }

    #[test]
    fn find_adts_sync_requires_eight_frames() {
        let bytes = build_adts_stream(8, 1, 3, 2);
        assert_eq!(find_adts_sync(&bytes), Some(0));
    }

    #[test]
    fn find_adts_sync_skips_prefix_garbage() {
        let mut bytes = vec![0xAAu8; 16];
        bytes.extend(build_adts_stream(8, 1, 3, 2));
        assert_eq!(find_adts_sync(&bytes), Some(16));
    }

    #[test]
    fn find_adts_sync_returns_none_for_one_frame() {
        let bytes = build_adts_frame(1, 3, 2);
        assert!(find_adts_sync(&bytes).is_none());
    }

    #[test]
    fn probe_accepts_aac_stream() {
        let bytes = build_adts_stream(10, 1, 3, 2);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(AacReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_adts_stream(10, 1, 3, 2);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.aac", 0);
        AacReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Aac);
        let cfg = out.tracks[0].properties.audio.as_ref().unwrap().codec_config.as_ref().unwrap();
        assert_eq!(cfg.aac_object_type, Some(2)); // LC = profile 1 + 1
    }

    #[test]
    fn format_aac_profile_table() {
        assert_eq!(format_aac_profile(0), "AAC Main");
        assert_eq!(format_aac_profile(1), "AAC LC");
        assert_eq!(format_aac_profile(2), "AAC SSR");
        assert_eq!(format_aac_profile(3), "AAC LTP");
        assert_eq!(format_aac_profile(7), "AAC");
    }
}
