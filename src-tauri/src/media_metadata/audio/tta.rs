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

//! TTA (The True Audio) reader.  Magic = `TTA1` per the format spec.
//!
//! Header layout (22 bytes):
//!
//! ```text
//! 4   "TTA1"
//! u16 AudioFormat (LE — 1 = PCM)
//! u16 NumChannels (LE)
//! u16 BitsPerSample (LE)
//! u32 SampleRate (LE)
//! u32 DataLength (LE — in samples)
//! u32 CRC32
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

#[derive(Debug, Clone, Copy)]
pub struct TtaHeader {
    pub channels: u32,
    pub bits_per_sample: u32,
    pub sample_rate: u32,
    pub data_length_samples: u64,
}

pub fn parse_header(bytes: &[u8]) -> Option<TtaHeader> {
    if bytes.len() < 22 || &bytes[..4] != b"TTA1" {
        return None;
    }
    let _audio_format = u16::from_le_bytes([bytes[4], bytes[5]]);
    let channels = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let bits = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    let rate = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
    let length = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]) as u64;
    Some(TtaHeader {
        channels,
        bits_per_sample: bits,
        sample_rate: rate,
        data_length_samples: length,
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TtaReader;

impl Reader for TtaReader {
    fn name(&self) -> &'static str {
        "tta"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 4];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read == 4 && &head == b"TTA1")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut bytes = vec![0u8; 22];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut bytes)?;
        let header = parse_header(&bytes[..read]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Tta;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let audio = AudioTrackProperties {
            channels: Some(header.channels),
            sampling_frequency: Some(header.sample_rate as f64),
            bit_depth: Some(header.bits_per_sample),
            ..AudioTrackProperties::default()
        };
        if header.sample_rate > 0 {
            let ns = (header.data_length_samples as u128)
                .saturating_mul(1_000_000_000)
                / header.sample_rate as u128;
            out.container.properties.duration =
                Some(crate::media_metadata::model::duration::DurationValue::from_ns(ns as u64));
        }
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_TTA1".to_string(),
                name: Some("TTA (True Audio)".to_string()),
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
pub(crate) fn build_tta1_header(
    channels: u16,
    bits: u16,
    sample_rate: u32,
    length_samples: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(22);
    bytes.extend_from_slice(b"TTA1");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&length_samples.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes()); // CRC
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_header_round_trips_basic_fields() {
        let bytes = build_tta1_header(2, 16, 44_100, 88_200);
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.channels, 2);
        assert_eq!(h.bits_per_sample, 16);
        assert_eq!(h.sample_rate, 44_100);
        assert_eq!(h.data_length_samples, 88_200);
    }

    #[test]
    fn parse_header_rejects_missing_magic() {
        let mut bytes = build_tta1_header(2, 16, 44_100, 88_200);
        bytes[0] = b'X';
        assert!(parse_header(&bytes).is_none());
    }

    #[test]
    fn parse_header_rejects_truncated() {
        assert!(parse_header(b"TTA1").is_none());
    }

    #[test]
    fn probe_accepts_tta1_magic() {
        let bytes = build_tta1_header(2, 16, 44_100, 1);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(TtaReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_track_and_duration() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_tta1_header(2, 16, 44_100, 88_200);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.tta", 0);
        TtaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Tta);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.bit_depth, Some(16));
        assert_eq!(a.channels, Some(2));
        // 88_200 samples / 44_100 Hz = 2 seconds = 2 000 000 000 ns
        assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
    }
}
