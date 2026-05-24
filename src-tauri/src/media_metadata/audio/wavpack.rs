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

//! WAVPACK v4 reader.  Frame layout (WavPack 4.0 spec):
//!
//! ```text
//! 4   "wvpk"
//! u32 block_size (LE — excluding 8-byte header)
//! u16 version (LE)
//! u8  track_number
//! u8  index_number
//! u32 total_samples (LE — `(u32)-1` means unknown)
//! u32 block_index   (LE)
//! u32 block_samples (LE)
//! u32 flags         (LE — bit field with sample rate index + bits/sample)
//! u32 crc           (LE)
//! ```
//!
//! Sample-rate index is bits 23..27 of `flags`.  Bits 0..1 hold
//! `bytes_per_sample - 1`.  Bit 2 is mono flag.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const SAMPLE_RATES: [u32; 16] = [
    6_000, 8_000, 9_600, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 64_000,
    88_200, 96_000, 192_000, 0,
];

#[derive(Debug, Clone, Copy)]
pub struct WavpackHeader {
    pub block_size: u32,
    pub version: u16,
    pub total_samples: u32,
    pub block_samples: u32,
    pub sample_rate: u32,
    pub bits_per_sample: u32,
    pub channels: u32,
}

pub fn parse_header(bytes: &[u8]) -> Option<WavpackHeader> {
    if bytes.len() < 32 || &bytes[..4] != b"wvpk" {
        return None;
    }
    let block_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let total_samples = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let block_samples = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    let flags = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);

    let sr_index = ((flags >> 23) & 0x0F) as usize;
    let sample_rate = SAMPLE_RATES[sr_index];
    let bps_index = (flags & 0x03) as u32;
    let bits_per_sample = (bps_index + 1) * 8;
    let channels = if flags & 0x04 != 0 { 1 } else { 2 };
    Some(WavpackHeader {
        block_size,
        version,
        total_samples,
        block_samples,
        sample_rate,
        bits_per_sample,
        channels,
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WavpackReader;

impl Reader for WavpackReader {
    fn name(&self) -> &'static str {
        "wavpack"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 4];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read == 4 && &head == b"wvpk")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut buf = vec![0u8; 32];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut buf)?;
        let header = parse_header(&buf[..read]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Wavpack;
        out.container.recognized = true;
        out.container.supported = true;
        if header.sample_rate > 0 && header.total_samples != u32::MAX {
            let ns = (header.total_samples as u128) * 1_000_000_000 / header.sample_rate as u128;
            out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
        }

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let audio = AudioTrackProperties {
            channels: Some(header.channels),
            sampling_frequency: if header.sample_rate == 0 {
                None
            } else {
                Some(header.sample_rate as f64)
            },
            bit_depth: Some(header.bits_per_sample),
            ..AudioTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_WAVPACK4".to_string(),
                name: Some(format!("WavPack {}", header.version)),
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
pub(crate) fn build_wavpack_header(
    sample_rate: u32,
    bps_index: u8,
    mono: bool,
    total_samples: u32,
    block_samples: u32,
) -> Vec<u8> {
    let sr_index = SAMPLE_RATES
        .iter()
        .position(|&s| s == sample_rate)
        .unwrap_or(15) as u32;
    let flags = (sr_index << 23) | ((bps_index as u32) & 0x03) | (if mono { 0x04 } else { 0 });
    let mut bytes = vec![0u8; 32];
    bytes[..4].copy_from_slice(b"wvpk");
    bytes[4..8].copy_from_slice(&100u32.to_le_bytes()); // block_size
    bytes[8..10].copy_from_slice(&0x0407u16.to_le_bytes()); // version
    bytes[12..16].copy_from_slice(&total_samples.to_le_bytes());
    bytes[20..24].copy_from_slice(&block_samples.to_le_bytes());
    bytes[24..28].copy_from_slice(&flags.to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_header_decodes_44100_24bit_stereo() {
        let bytes = build_wavpack_header(44_100, 2, false, 88_200, 1024);
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.sample_rate, 44_100);
        assert_eq!(h.bits_per_sample, 24);
        assert_eq!(h.channels, 2);
        assert_eq!(h.total_samples, 88_200);
    }

    #[test]
    fn parse_header_handles_mono_flag() {
        let bytes = build_wavpack_header(48_000, 1, true, 96_000, 1024);
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.channels, 1);
    }

    #[test]
    fn parse_header_rejects_non_wvpk() {
        let mut bytes = build_wavpack_header(48_000, 1, false, 1, 1);
        bytes[0] = b'X';
        assert!(parse_header(&bytes).is_none());
    }

    #[test]
    fn parse_header_rejects_truncated() {
        assert!(parse_header(b"wvpk").is_none());
    }

    #[test]
    fn probe_accepts_wvpk_magic() {
        let bytes = build_wavpack_header(44_100, 2, false, 1, 1);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(WavpackReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_track_and_duration() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_wavpack_header(44_100, 1, false, 88_200, 1024);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.wv", 0);
        WavpackReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.bit_depth, Some(16));
        assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
    }

    #[test]
    fn read_headers_handles_unknown_total_samples() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_wavpack_header(44_100, 1, false, u32::MAX, 1024);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.wv", 0);
        WavpackReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert!(out.container.properties.duration.is_none());
    }
}
