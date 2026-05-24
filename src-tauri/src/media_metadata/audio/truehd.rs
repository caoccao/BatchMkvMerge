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

//! Dolby TrueHD reader.  Sync words:
//!
//! - MLP (TrueHD predecessor) major sync: `0xF8 0x72 0x6F 0xBA` after a
//!   2-byte access-unit header.
//! - TrueHD major sync: `0xF8 0x72 0x6F 0xBB`.
//!
//! Per ATSC A/85 + Dolby TrueHD format spec, the major-sync packet at offset
//! 4 of the access unit carries the sample-rate + channel-count nibble.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 256 * 1024;
const SAMPLE_RATES: [u32; 16] = [
    48_000, 96_000, 192_000, 0, 0, 0, 0, 0, 44_100, 88_200, 176_400, 0, 0, 0, 0, 0,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrueHdVariant {
    Mlp,
    TrueHd,
}

#[derive(Debug, Clone, Copy)]
pub struct MajorSync {
    pub variant: TrueHdVariant,
    pub sample_rate: u32,
    pub channels: u32,
}

pub fn find_major_sync(bytes: &[u8]) -> Option<(usize, MajorSync)> {
    let mut i = 0usize;
    while i + 9 <= bytes.len() {
        if bytes[i] == 0xF8 && bytes[i + 1] == 0x72 && bytes[i + 2] == 0x6F {
            let variant = match bytes[i + 3] {
                0xBA => TrueHdVariant::Mlp,
                0xBB => TrueHdVariant::TrueHd,
                _ => {
                    i += 1;
                    continue;
                }
            };
            // sample_rate nibble is the high 4 bits of byte i+8
            let sr_nibble = (bytes[i + 8] >> 4) as usize;
            let sample_rate = SAMPLE_RATES.get(sr_nibble).copied().unwrap_or(0);
            // Channel count at byte i+9 (when present)
            let channels = if i + 10 <= bytes.len() {
                channels_from_nibbles(bytes[i + 9])
            } else {
                0
            };
            return Some((
                i,
                MajorSync {
                    variant,
                    sample_rate,
                    channels,
                },
            ));
        }
        i += 1;
    }
    None
}

fn channels_from_nibbles(b: u8) -> u32 {
    // Channel count layout: bits 7..4 hold 2-channel pairs.  We approximate
    // by popcount + 2 for 5.1 / 7.1 streams.
    let low = (b & 0x1F).count_ones();
    low.max(2)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TrueHdReader;

impl Reader for TrueHdReader {
    fn name(&self) -> &'static str {
        "truehd"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < 9 {
            return Ok(false);
        }
        Ok(find_major_sync(&probe[..read]).is_some())
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
        let (_offset, ms) = find_major_sync(&probe[..read]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::TrueHd;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let mut audio = AudioTrackProperties::default();
        if ms.sample_rate > 0 {
            audio.sampling_frequency = Some(ms.sample_rate as f64);
        }
        if ms.channels > 0 {
            audio.channels = Some(ms.channels);
        }
        let codec_name = match ms.variant {
            TrueHdVariant::Mlp => "Dolby MLP",
            TrueHdVariant::TrueHd => "Dolby TrueHD",
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_TRUEHD".to_string(),
                name: Some(codec_name.to_string()),
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
pub(crate) fn build_truehd_major_sync(sr_nibble: u8, channels: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    bytes[0] = 0xF8;
    bytes[1] = 0x72;
    bytes[2] = 0x6F;
    bytes[3] = 0xBB;
    bytes[8] = sr_nibble << 4;
    bytes[9] = (channels as u8) & 0x1F;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn finds_truehd_major_sync_at_offset_zero() {
        let bytes = build_truehd_major_sync(1, 6);
        let (pos, ms) = find_major_sync(&bytes).unwrap();
        assert_eq!(pos, 0);
        assert_eq!(ms.variant, TrueHdVariant::TrueHd);
        assert_eq!(ms.sample_rate, 96_000);
    }

    #[test]
    fn finds_mlp_variant() {
        let mut bytes = build_truehd_major_sync(0, 2);
        bytes[3] = 0xBA;
        let (_, ms) = find_major_sync(&bytes).unwrap();
        assert_eq!(ms.variant, TrueHdVariant::Mlp);
    }

    #[test]
    fn skips_prefix_garbage() {
        let mut bytes = vec![0xAAu8; 16];
        bytes.extend(build_truehd_major_sync(8, 6));
        let (pos, ms) = find_major_sync(&bytes).unwrap();
        assert_eq!(pos, 16);
        assert_eq!(ms.sample_rate, 44_100);
    }

    #[test]
    fn returns_none_for_garbage() {
        assert!(find_major_sync(&[0xAAu8; 64]).is_none());
    }

    #[test]
    fn probe_accepts_truehd_stream() {
        let bytes = build_truehd_major_sync(1, 6);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(TrueHdReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_truehd_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_truehd_major_sync(2, 6);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.thd", 0);
        TrueHdReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::TrueHd);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.sampling_frequency, Some(192_000.0));
    }

    #[test]
    fn channels_from_nibbles_falls_back_to_two() {
        assert_eq!(channels_from_nibbles(0x00), 2);
        assert_eq!(channels_from_nibbles(0x1F), 5);
    }

    #[test]
    fn unknown_4th_byte_does_not_match() {
        let mut bytes = build_truehd_major_sync(1, 6);
        bytes[3] = 0xAB;
        assert!(find_major_sync(&bytes).is_none());
    }
}
