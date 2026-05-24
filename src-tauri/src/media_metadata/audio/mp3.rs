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

//! MP3 reader (MPEG-1/2/2.5 Audio Layer I/II/III).
//!
//! Frame header layout (ISO/IEC 11172-3 §2.4.1.3 + 13818-3 extensions):
//!
//! ```text
//! 11 bits frame sync         (1111_1111_111)
//! 2  bits MPEG version       (00=2.5, 10=2, 11=1)
//! 2  bits layer              (01=III, 10=II, 11=I)
//! 1  bit  protection
//! 4  bits bitrate index
//! 2  bits sampling rate index
//! 1  bit  padding
//! 1  bit  private
//! 2  bits channel mode       (00=stereo, 01=joint, 10=dual, 11=mono)
//! ...
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;
const MIN_CONFIRM_FRAMES: usize = 8;

const BITRATE_TABLE_V1_LAYER3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BITRATE_TABLE_V2_LAYER3: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];
const SAMPLE_RATE_TABLE_V1: [u32; 4] = [44_100, 48_000, 32_000, 0];
const SAMPLE_RATE_TABLE_V2: [u32; 4] = [22_050, 24_000, 16_000, 0];
const SAMPLE_RATE_TABLE_V25: [u32; 4] = [11_025, 12_000, 8_000, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MpegVersion {
    V1,
    V2,
    V25,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    L1,
    L2,
    L3,
}

#[derive(Debug, Clone, Copy)]
pub struct Mp3Frame {
    pub version: MpegVersion,
    pub layer: Layer,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate_kbps: u32,
    pub frame_length: usize,
}

/// Decode the 4-byte frame header at `bytes[..4]`.  Returns `None` if the
/// sync bits / fields are inconsistent.
pub fn decode_frame(bytes: &[u8]) -> Option<Mp3Frame> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes[0] != 0xFF || (bytes[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version = match (bytes[1] >> 3) & 0x03 {
        0b00 => MpegVersion::V25,
        0b10 => MpegVersion::V2,
        0b11 => MpegVersion::V1,
        _ => return None,
    };
    let layer = match (bytes[1] >> 1) & 0x03 {
        0b11 => Layer::L1,
        0b10 => Layer::L2,
        0b01 => Layer::L3,
        _ => return None,
    };
    let bitrate_index = (bytes[2] >> 4) & 0x0F;
    let sample_rate_index = ((bytes[2] >> 2) & 0x03) as usize;
    let padding = (bytes[2] >> 1) & 0x01;
    let channel_mode = (bytes[3] >> 6) & 0x03;

    if !matches!(layer, Layer::L3) {
        // Phase 7 focuses on Layer III; reject Layers I/II for confirmation.
        return None;
    }

    let bitrate_table = match version {
        MpegVersion::V1 => &BITRATE_TABLE_V1_LAYER3,
        MpegVersion::V2 | MpegVersion::V25 => &BITRATE_TABLE_V2_LAYER3,
    };
    let bitrate_kbps = bitrate_table[bitrate_index as usize];
    if bitrate_kbps == 0 {
        return None;
    }
    let sr_table = match version {
        MpegVersion::V1 => &SAMPLE_RATE_TABLE_V1,
        MpegVersion::V2 => &SAMPLE_RATE_TABLE_V2,
        MpegVersion::V25 => &SAMPLE_RATE_TABLE_V25,
    };
    let sample_rate = sr_table[sample_rate_index];
    if sample_rate == 0 {
        return None;
    }
    let channels = if channel_mode == 0b11 { 1 } else { 2 };
    let frame_length = if matches!(version, MpegVersion::V1) {
        144 * bitrate_kbps as usize * 1000 / sample_rate as usize + padding as usize
    } else {
        72 * bitrate_kbps as usize * 1000 / sample_rate as usize + padding as usize
    };
    Some(Mp3Frame {
        version,
        layer,
        sample_rate,
        channels,
        bitrate_kbps,
        frame_length,
    })
}

/// Scan `bytes` for [`MIN_CONFIRM_FRAMES`] consecutive valid MP3 frames.
/// Returns the offset of the first frame on success.
pub fn find_frame_sync(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if let Some(frame) = decode_frame(&bytes[i..]) {
            let mut hits = 1usize;
            let mut next = i + frame.frame_length.max(4);
            while hits < MIN_CONFIRM_FRAMES && next + 4 <= bytes.len() {
                let Some(next_frame) = decode_frame(&bytes[next..]) else {
                    break;
                };
                hits += 1;
                next += next_frame.frame_length.max(4);
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
pub struct Mp3Reader;

impl Reader for Mp3Reader {
    fn name(&self) -> &'static str {
        "mp3"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < 4 {
            return Ok(false);
        }
        let (start, _end) = id3v2::payload_bounds(&probe[..read]);
        Ok(find_frame_sync(&probe[start..read]).is_some())
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
        let offset = find_frame_sync(bytes).ok_or(ParseError::Unrecognised)?;
        let frame = decode_frame(&bytes[offset..]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Mp3;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let audio = AudioTrackProperties {
            channels: Some(frame.channels),
            sampling_frequency: Some(frame.sample_rate as f64),
            ..AudioTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_MPEG/L3".to_string(),
                name: Some("MP3".to_string()),
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

#[cfg(test)]
pub(crate) fn build_mp3_frame_v1(bitrate_kbps: u32, sample_rate: u32, mono: bool) -> Vec<u8> {
    let bitrate_index = BITRATE_TABLE_V1_LAYER3
        .iter()
        .position(|&b| b == bitrate_kbps)
        .unwrap() as u8;
    let sr_index = SAMPLE_RATE_TABLE_V1
        .iter()
        .position(|&s| s == sample_rate)
        .unwrap() as u8;
    let mut header = [0u8; 4];
    header[0] = 0xFF;
    header[1] = 0xFB; // version 1 + layer III + protection bit (no CRC)
    header[2] = (bitrate_index << 4) | (sr_index << 2);
    header[3] = if mono { 0xC0 } else { 0x00 };
    let frame = decode_frame(&header).unwrap();
    let mut bytes = Vec::with_capacity(frame.frame_length);
    bytes.extend_from_slice(&header);
    bytes.resize(frame.frame_length, 0);
    bytes
}

#[cfg(test)]
pub(crate) fn build_mp3_stream(frames: usize, bitrate: u32, sample_rate: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..frames {
        bytes.extend(build_mp3_frame_v1(bitrate, sample_rate, false));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decode_frame_handles_mpeg1_layer3_128kbps_44100() {
        let frame = build_mp3_frame_v1(128, 44_100, false);
        let f = decode_frame(&frame).unwrap();
        assert_eq!(f.version, MpegVersion::V1);
        assert_eq!(f.layer, Layer::L3);
        assert_eq!(f.bitrate_kbps, 128);
        assert_eq!(f.sample_rate, 44_100);
        assert_eq!(f.channels, 2);
        assert_eq!(f.frame_length, 417);
    }

    #[test]
    fn decode_frame_rejects_layer1_and_2() {
        let mut header = [0xFFu8, 0xFB, 0x90, 0x00];
        header[1] = 0xFF; // layer = 11 → Layer I
        assert!(decode_frame(&header).is_none());
    }

    #[test]
    fn decode_frame_rejects_invalid_bitrate_index() {
        let mut header = [0xFFu8, 0xFB, 0xF0, 0x00]; // bitrate index 15 = invalid
        header[2] = 0xF0;
        assert!(decode_frame(&header).is_none());
    }

    #[test]
    fn decode_frame_rejects_invalid_sample_rate_index() {
        let mut header = build_mp3_frame_v1(128, 44_100, false);
        header[2] |= 0x0C; // set sample_rate_index to 3 = invalid
        assert!(decode_frame(&header).is_none());
    }

    #[test]
    fn decode_frame_handles_mono_channel_mode() {
        let frame = build_mp3_frame_v1(96, 48_000, true);
        let f = decode_frame(&frame).unwrap();
        assert_eq!(f.channels, 1);
    }

    #[test]
    fn find_frame_sync_requires_eight_consecutive_frames() {
        let bytes = build_mp3_stream(8, 128, 44_100);
        assert_eq!(find_frame_sync(&bytes), Some(0));
    }

    #[test]
    fn find_frame_sync_skips_garbage_prefix() {
        let mut bytes = vec![0xAAu8; 16];
        bytes.extend(build_mp3_stream(8, 128, 44_100));
        assert_eq!(find_frame_sync(&bytes), Some(16));
    }

    #[test]
    fn find_frame_sync_returns_none_for_single_frame() {
        let bytes = build_mp3_frame_v1(128, 44_100, false);
        assert!(find_frame_sync(&bytes).is_none());
    }

    #[test]
    fn probe_accepts_clean_mp3_stream() {
        let bytes = build_mp3_stream(10, 128, 44_100);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(Mp3Reader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_accepts_mp3_after_id3v2_header() {
        let mut bytes = id3v2::build_id3v2_tag(false, 64);
        bytes.extend(build_mp3_stream(10, 128, 44_100));
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(Mp3Reader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
        assert!(!Mp3Reader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_audio_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_mp3_stream(10, 128, 44_100);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mp3", 0);
        Mp3Reader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Mp3);
        assert_eq!(out.tracks.len(), 1);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.sampling_frequency, Some(44_100.0));
    }
}
