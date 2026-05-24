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

//! Native FLAC reader (`.flac` files starting with `fLaC`).
//!
//! Layout (FLAC spec §3):
//!
//! ```text
//! 4   "fLaC"
//! repeat metadata blocks:
//!   u8  is_last(1) | block_type(7)
//!   u24 length (BE)
//!   [length bytes of block body]
//! ```
//!
//! Block type 0 = STREAMINFO (mandatory, first).  Block type 4 =
//! VORBIS_COMMENT — same layout as the in-Ogg variant decoded by [`crate::media_metadata::ogg::comments`].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::tag::TagEntry;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{
    AudioCodecConfig, AudioTrackProperties,
};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::ogg::comments;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 64 * 1024;
const BLOCK_TYPE_STREAMINFO: u8 = 0;
const BLOCK_TYPE_VORBIS_COMMENT: u8 = 4;

#[derive(Debug, Clone)]
pub struct FlacStreaminfo {
    pub min_block_size: u32,
    pub max_block_size: u32,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub total_samples: u64,
    pub md5_hex: String,
}

#[derive(Debug, Default, Clone)]
pub struct FlacMetadata {
    pub streaminfo: Option<FlacStreaminfo>,
    pub vendor: Option<String>,
    pub tags: Vec<TagEntry>,
}

pub fn parse(bytes: &[u8]) -> Option<FlacMetadata> {
    if bytes.len() < 4 || &bytes[..4] != b"fLaC" {
        return None;
    }
    let mut metadata = FlacMetadata::default();
    let mut pos = 4usize;
    loop {
        if pos + 4 > bytes.len() {
            break;
        }
        let header = bytes[pos];
        let last_block = header & 0x80 != 0;
        let block_type = header & 0x7F;
        let length = (((bytes[pos + 1] as usize) << 16)
            | ((bytes[pos + 2] as usize) << 8)
            | bytes[pos + 3] as usize) as usize;
        pos += 4;
        let body_end = pos + length;
        if body_end > bytes.len() {
            break;
        }
        let body = &bytes[pos..body_end];
        match block_type {
            BLOCK_TYPE_STREAMINFO if body.len() >= 34 => {
                metadata.streaminfo = Some(decode_streaminfo(body));
            }
            BLOCK_TYPE_VORBIS_COMMENT => {
                if let Some(c) = comments::parse(body) {
                    metadata.vendor = Some(c.vendor);
                    metadata.tags = c.entries;
                }
            }
            _ => {}
        }
        pos = body_end;
        if last_block {
            break;
        }
    }
    Some(metadata)
}

fn decode_streaminfo(body: &[u8]) -> FlacStreaminfo {
    let min_block_size = u16::from_be_bytes([body[0], body[1]]) as u32;
    let max_block_size = u16::from_be_bytes([body[2], body[3]]) as u32;
    let min_frame_size = ((body[4] as u32) << 16) | ((body[5] as u32) << 8) | body[6] as u32;
    let max_frame_size = ((body[7] as u32) << 16) | ((body[8] as u32) << 8) | body[9] as u32;
    let packed = u64::from_be_bytes([
        body[10], body[11], body[12], body[13], body[14], body[15], body[16], body[17],
    ]);
    let sample_rate = ((packed >> 44) & 0xF_FFFF) as u32;
    let channels = (((packed >> 41) & 0x07) + 1) as u32;
    let bps = (((packed >> 36) & 0x1F) + 1) as u32;
    let total_samples = packed & 0x0F_FFFF_FFFF;
    let md5: [u8; 16] = body[18..34].try_into().unwrap();
    let md5_hex = md5.iter().map(|b| format!("{:02x}", b)).collect();
    FlacStreaminfo {
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        sample_rate,
        channels,
        bits_per_sample: bps,
        total_samples,
        md5_hex,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FlacReader;

impl Reader for FlacReader {
    fn name(&self) -> &'static str {
        "flac"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 4];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read == 4 && &head == b"fLaC")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut buf)?;
        let metadata = parse(&buf[..read]).ok_or(ParseError::Unrecognised)?;
        let streaminfo = metadata.streaminfo.ok_or(ParseError::Malformed {
            format: "flac",
            offset: 0,
            reason: "missing STREAMINFO block".to_string(),
        })?;

        out.container.format = ContainerFormat::Flac;
        out.container.recognized = true;
        out.container.supported = true;
        if streaminfo.sample_rate > 0 {
            let ns = (streaminfo.total_samples as u128).saturating_mul(1_000_000_000)
                / streaminfo.sample_rate as u128;
            out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
        }
        if let Some(vendor) = metadata.vendor.clone() {
            out.container.properties.muxing_app = Some(vendor);
        }

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let audio = AudioTrackProperties {
            channels: Some(streaminfo.channels),
            sampling_frequency: Some(streaminfo.sample_rate as f64),
            bit_depth: Some(streaminfo.bits_per_sample),
            codec_config: Some(AudioCodecConfig {
                flac_min_block_size: Some(streaminfo.min_block_size),
                flac_max_block_size: Some(streaminfo.max_block_size),
                flac_min_frame_size: Some(streaminfo.min_frame_size),
                flac_max_frame_size: Some(streaminfo.max_frame_size),
                flac_total_samples: if streaminfo.total_samples == 0 {
                    None
                } else {
                    Some(streaminfo.total_samples)
                },
                flac_md5_hex: Some(streaminfo.md5_hex.clone()),
                ..AudioCodecConfig::default()
            }),
            ..AudioTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_FLAC".to_string(),
                name: Some("FLAC".to_string()),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                audio: Some(audio),
                tags: metadata.tags,
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn build_flac_native(
    sample_rate: u32,
    channels: u32,
    bps: u32,
    total_samples: u64,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fLaC");
    // STREAMINFO: type 0, last flag, length 34
    bytes.push(0x80); // last_block + type 0
    bytes.extend_from_slice(&[0u8, 0u8, 34]);
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = ((sample_rate as u64) << 44)
        | (((channels - 1) as u64 & 0x7) << 41)
        | (((bps - 1) as u64 & 0x1F) << 36)
        | (total_samples & 0x0F_FFFF_FFFF);
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_extracts_streaminfo_fields() {
        let bytes = build_flac_native(48_000, 2, 24, 96_000);
        let m = parse(&bytes).unwrap();
        let si = m.streaminfo.unwrap();
        assert_eq!(si.sample_rate, 48_000);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 24);
        assert_eq!(si.total_samples, 96_000);
    }

    #[test]
    fn parse_rejects_non_native_flac() {
        let bytes = b"junk".to_vec();
        assert!(parse(&bytes).is_none());
    }

    #[test]
    fn parse_handles_truncated_block_header_gracefully() {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend_from_slice(&[0x80, 0xFF]); // truncated header
        let m = parse(&bytes).unwrap();
        assert!(m.streaminfo.is_none());
    }

    #[test]
    fn probe_accepts_flac_magic() {
        let bytes = build_flac_native(48_000, 2, 24, 1);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(FlacReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_track_and_duration() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_flac_native(48_000, 2, 24, 96_000);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.flac", 0);
        FlacReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Flac);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.bit_depth, Some(24));
        // 96_000 samples / 48_000 = 2 seconds
        assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
    }

    #[test]
    fn read_headers_returns_malformed_without_streaminfo() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = b"fLaC".to_vec();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.flac", 0);
        let err = FlacReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }
}
