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

//! DTS reader.  Sync words (ETSI TS 102 114):
//!
//! - `0x7FFE8001` — 16-bit big-endian (canonical .dts)
//! - `0xFE7F0180` — 16-bit little-endian
//! - `0x1FFFE800` — 14-bit big-endian
//! - `0xFF1F00E8` — 14-bit little-endian
//! - `0x64582025` — DTS-HD extension substream (HD-MA, HD-HRA)
//!
//! For identification we only sniff the sync and report DTS / DTS-HD.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtsSync {
    Be16,
    Le16,
    Be14,
    Le14,
    HdExtension,
}

const SAMPLE_RATE_TABLE: [u32; 16] = [
    0, 8_000, 16_000, 32_000, 0, 0, 11_025, 22_050, 44_100, 0, 0, 12_000, 24_000, 48_000, 96_000,
    192_000,
];

/// Find a DTS sync word inside the buffer.
pub fn find_sync(bytes: &[u8]) -> Option<(usize, DtsSync)> {
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if let Some(sync) = classify_sync(&bytes[i..i + 4]) {
            return Some((i, sync));
        }
        i += 1;
    }
    None
}

fn classify_sync(b: &[u8]) -> Option<DtsSync> {
    match (b[0], b[1], b[2], b[3]) {
        (0x7F, 0xFE, 0x80, 0x01) => Some(DtsSync::Be16),
        (0xFE, 0x7F, 0x01, 0x80) => Some(DtsSync::Le16),
        (0x1F, 0xFF, 0xE8, 0x00) => Some(DtsSync::Be14),
        (0xFF, 0x1F, 0x00, 0xE8) => Some(DtsSync::Le14),
        (0x64, 0x58, 0x20, 0x25) => Some(DtsSync::HdExtension),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CoreFrameHeader {
    pub sample_rate: u32,
    pub channels: u32,
    pub is_hd: bool,
}

pub fn decode_core_header(bytes: &[u8]) -> Option<CoreFrameHeader> {
    let (offset, sync) = find_sync(bytes)?;
    let is_hd = matches!(sync, DtsSync::HdExtension);
    if is_hd {
        // For HD-only streams we report a placeholder rate; the inner core
        // frame would expose the real one.
        return Some(CoreFrameHeader {
            sample_rate: 0,
            channels: 0,
            is_hd: true,
        });
    }
    if !matches!(sync, DtsSync::Be16) {
        // Only the canonical 16-bit BE form carries the simple sample-rate
        // index we can read without bit-stream conversion.
        return Some(CoreFrameHeader {
            sample_rate: 0,
            channels: 0,
            is_hd: false,
        });
    }
    if offset + 13 > bytes.len() {
        return Some(CoreFrameHeader {
            sample_rate: 0,
            channels: 0,
            is_hd: false,
        });
    }
    // Reading frame header bits per ETSI TS 102 114 §5.3:
    let p = &bytes[offset + 4..];
    if p.len() < 9 {
        return None;
    }
    // 1 bit FTYPE, 5 bits SHORT, 1 bit CPF, 7 bits NBLKS, 14 bits FSIZE, 6 bits AMODE,
    // 4 bits SFREQ, ...
    let amode = ((p[3] & 0x0F) << 2) | (p[4] >> 6);
    let sfreq = ((p[4] >> 2) & 0x0F) as usize;
    let sample_rate = SAMPLE_RATE_TABLE[sfreq];
    let channels = channels_from_amode(amode);
    Some(CoreFrameHeader {
        sample_rate,
        channels,
        is_hd: false,
    })
}

fn channels_from_amode(amode: u8) -> u32 {
    match amode {
        0 => 1,
        1 | 2 | 3 | 4 => 2,
        5 | 6 => 3,
        7 | 8 => 4,
        9 => 5,
        10..=15 => 6,
        _ => 0,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DtsReader;

impl Reader for DtsReader {
    fn name(&self) -> &'static str {
        "dts"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < 4 {
            return Ok(false);
        }
        let (start, _end) = id3v2::payload_bounds(&probe[..read]);
        Ok(find_sync(&probe[start..read]).is_some())
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
        let header = decode_core_header(bytes).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Dts;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let mut audio = AudioTrackProperties::default();
        if header.sample_rate > 0 {
            audio.sampling_frequency = Some(header.sample_rate as f64);
        }
        if header.channels > 0 {
            audio.channels = Some(header.channels);
        }
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: "A_DTS".to_string(),
                name: Some(if header.is_hd { "DTS-HD" } else { "DTS" }.to_string()),
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
pub(crate) fn build_dts_be16_frame() -> Vec<u8> {
    // Header (4-byte sync + 9-byte frame header) + dummy payload
    let mut bytes = vec![0u8; 64];
    bytes[0] = 0x7F;
    bytes[1] = 0xFE;
    bytes[2] = 0x80;
    bytes[3] = 0x01;
    // amode = 6, sfreq = 13 (48 kHz) — span bytes 7..9 of the header (per
    // amode = upper 4 bits of p[3] (0..3) and lower 2 of p[4] (4..5)).
    bytes[7] = 0x00 | (6 >> 2); // upper bits of amode in p[3]
    // amode bits: shift so (p[3] & 0x0F) << 2 | (p[4] >> 6) == 6
    // Use amode=6 → p[3]=0b0000_0001, p[4]=0b1000_0000  ⇒ (0x01<<2)|0b10 = 6
    bytes[7] = 0x01;
    bytes[8] = 0b1011_0100; // top 2 bits = 10 (=2), then sfreq 4 bits = 1101 = 13
    // amode = (0x01<<2) | 0b10 = 6
    // sfreq = ((0b10110100 >> 2) & 0x0F) = 0b1101 = 13
    bytes
}

#[cfg(test)]
pub(crate) fn build_dts_hd_extension() -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    bytes[0] = 0x64;
    bytes[1] = 0x58;
    bytes[2] = 0x20;
    bytes[3] = 0x25;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn classifies_all_four_dts_syncs() {
        assert_eq!(classify_sync(&[0x7F, 0xFE, 0x80, 0x01]), Some(DtsSync::Be16));
        assert_eq!(classify_sync(&[0xFE, 0x7F, 0x01, 0x80]), Some(DtsSync::Le16));
        assert_eq!(classify_sync(&[0x1F, 0xFF, 0xE8, 0x00]), Some(DtsSync::Be14));
        assert_eq!(classify_sync(&[0xFF, 0x1F, 0x00, 0xE8]), Some(DtsSync::Le14));
        assert_eq!(classify_sync(&[0x64, 0x58, 0x20, 0x25]), Some(DtsSync::HdExtension));
        assert_eq!(classify_sync(&[0x00, 0x00, 0x00, 0x00]), None);
    }

    #[test]
    fn find_sync_skips_garbage_prefix() {
        let mut bytes = vec![0xAAu8; 32];
        bytes.extend([0x7F, 0xFE, 0x80, 0x01]);
        let (pos, sync) = find_sync(&bytes).unwrap();
        assert_eq!(pos, 32);
        assert_eq!(sync, DtsSync::Be16);
    }

    #[test]
    fn find_sync_returns_none_for_garbage() {
        assert!(find_sync(&[0xAAu8; 64]).is_none());
    }

    #[test]
    fn decode_core_header_extracts_sample_rate_and_channels() {
        let frame = build_dts_be16_frame();
        let h = decode_core_header(&frame).unwrap();
        assert_eq!(h.sample_rate, 48_000);
        // amode = 6 (L+R+S) = 3 channels per ETSI TS 102 114 Table 5.3.
        assert_eq!(h.channels, 3);
        assert!(!h.is_hd);
    }

    #[test]
    fn decode_core_header_flags_hd_extension() {
        let frame = build_dts_hd_extension();
        let h = decode_core_header(&frame).unwrap();
        assert!(h.is_hd);
    }

    #[test]
    fn probe_accepts_dts_be16_stream() {
        let bytes = build_dts_be16_frame();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(DtsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_dts_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_dts_be16_frame();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.dts", 0);
        DtsReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Dts);
        assert_eq!(out.tracks[0].codec.id, "A_DTS");
        assert_eq!(out.tracks[0].codec.name.as_deref(), Some("DTS"));
    }

    #[test]
    fn read_headers_emits_dts_hd_codec_name() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_dts_hd_extension();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.dtshd", 0);
        DtsReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.name.as_deref(), Some("DTS-HD"));
    }

    #[test]
    fn channels_from_amode_table() {
        assert_eq!(channels_from_amode(0), 1);
        assert_eq!(channels_from_amode(1), 2);
        assert_eq!(channels_from_amode(5), 3);
        assert_eq!(channels_from_amode(7), 4);
        assert_eq!(channels_from_amode(9), 5);
        assert_eq!(channels_from_amode(14), 6);
    }
}
