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

//! AC-3 / E-AC-3 reader.
//!
//! Frame sync = `0x0B 0x77` (ATSC A/52 §4.4.1).  After sync:
//!
//! ```text
//! u16 CRC1
//! 2 bits fscod (sample rate code)
//! 6 bits frmsizecod (frame-size index)
//! 5 bits bsid                          (== 8 for AC-3, ≥ 11 for E-AC-3)
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

const SAMPLE_RATES: [u32; 3] = [48_000, 44_100, 32_000];

const FRAME_SIZES: [[u16; 3]; 38] = [
    [64, 69, 96], [64, 70, 96], [80, 87, 120], [80, 88, 120],
    [96, 104, 144], [96, 105, 144], [112, 121, 168], [112, 122, 168],
    [128, 139, 192], [128, 140, 192], [160, 174, 240], [160, 175, 240],
    [192, 208, 288], [192, 209, 288], [224, 243, 336], [224, 244, 336],
    [256, 278, 384], [256, 279, 384], [320, 348, 480], [320, 349, 480],
    [384, 417, 576], [384, 418, 576], [448, 487, 672], [448, 488, 672],
    [512, 557, 768], [512, 558, 768], [640, 696, 960], [640, 697, 960],
    [768, 835, 1152], [768, 836, 1152], [896, 975, 1344], [896, 976, 1344],
    [1024, 1114, 1536], [1024, 1115, 1536], [1152, 1253, 1728], [1152, 1254, 1728],
    [1280, 1393, 1920], [1280, 1394, 1920],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ac3Variant {
    Ac3,
    Eac3,
}

#[derive(Debug, Clone, Copy)]
pub struct Ac3Frame {
    pub variant: Ac3Variant,
    pub sample_rate: u32,
    pub frame_length: usize,
    pub channels: u32,
    pub bsid: u8,
}

pub fn decode_frame(bytes: &[u8]) -> Option<Ac3Frame> {
    if bytes.len() < 6 {
        return None;
    }
    if bytes[0] != 0x0B || bytes[1] != 0x77 {
        return None;
    }
    let bsid = bytes[5] >> 3;
    if bsid <= 10 {
        decode_ac3_frame(bytes, bsid)
    } else {
        decode_eac3_frame(bytes, bsid)
    }
}

fn decode_ac3_frame(bytes: &[u8], bsid: u8) -> Option<Ac3Frame> {
    let fscod = (bytes[4] >> 6) & 0x03;
    let frmsizecod = (bytes[4] & 0x3F) as usize;
    if fscod == 3 || frmsizecod >= FRAME_SIZES.len() {
        return None;
    }
    let acmod = (bytes[6] >> 5) & 0x07;
    let lfeon = (channels_with_lfe(bytes, acmod)) as u32;
    let sample_rate = SAMPLE_RATES[fscod as usize];
    let frame_length = (FRAME_SIZES[frmsizecod][fscod as usize] as usize) * 2;
    Some(Ac3Frame {
        variant: Ac3Variant::Ac3,
        sample_rate,
        frame_length,
        channels: channels_from_acmod(acmod) + lfeon,
        bsid,
    })
}

fn channels_with_lfe(bytes: &[u8], acmod: u8) -> u8 {
    let bsi_offset_bits = 5 + 3 + match acmod {
        0 => 2 + 2, // 1+1 → 2x dialnorm
        2 => 2,      // 2/0
        _ => 0,
    };
    let byte_index = 6 + bsi_offset_bits / 8;
    let bit_index = 7 - (bsi_offset_bits % 8) as u8;
    if byte_index >= bytes.len() {
        return 0;
    }
    if (bytes[byte_index] >> bit_index) & 0x01 != 0 {
        1
    } else {
        0
    }
}

fn channels_from_acmod(acmod: u8) -> u32 {
    match acmod {
        0 => 2,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 3,
        5 => 4,
        6 => 4,
        7 => 5,
        _ => 0,
    }
}

fn decode_eac3_frame(bytes: &[u8], bsid: u8) -> Option<Ac3Frame> {
    // E-AC-3 layout per ATSC A/52 Annex E:
    // - 2 bits strmtyp
    // - 3 bits substreamid
    // - 11 bits frmsiz (in 16-bit words minus 1)
    // - 2 bits fscod
    // - 2 bits numblkscod (or fscod2)
    // - 3 bits acmod
    // - 1 bit  lfeon
    // - 5 bits bsid (we already know this)
    let frmsiz = (((bytes[2] as u16) & 0x07) << 8) | bytes[3] as u16;
    let frame_length = ((frmsiz as usize) + 1) * 2;
    let fscod = (bytes[4] >> 6) & 0x03;
    let fscod2 = (bytes[4] >> 4) & 0x03;
    let sample_rate = if fscod == 3 {
        match fscod2 {
            0 => 24_000,
            1 => 22_050,
            2 => 16_000,
            _ => return None,
        }
    } else {
        SAMPLE_RATES[fscod as usize]
    };
    let acmod = (bytes[4] >> 1) & 0x07;
    let lfeon = (bytes[4] & 0x01) as u32;
    Some(Ac3Frame {
        variant: Ac3Variant::Eac3,
        sample_rate,
        frame_length,
        channels: channels_from_acmod(acmod) + lfeon,
        bsid,
    })
}

pub fn find_frame_sync(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + 6 <= bytes.len() {
        if let Some(frame) = decode_frame(&bytes[i..]) {
            let mut hits = 1usize;
            let mut next = i + frame.frame_length.max(6);
            while hits < MIN_CONFIRM_FRAMES && next + 6 <= bytes.len() {
                let Some(next_frame) = decode_frame(&bytes[next..]) else {
                    break;
                };
                hits += 1;
                next += next_frame.frame_length.max(6);
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
pub struct Ac3Reader;

impl Reader for Ac3Reader {
    fn name(&self) -> &'static str {
        "ac3"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < 6 {
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

        let (codec_id, codec_name, format) = match frame.variant {
            Ac3Variant::Ac3 => ("A_AC3", "AC-3", ContainerFormat::Ac3),
            Ac3Variant::Eac3 => ("A_EAC3", "E-AC-3", ContainerFormat::Eac3),
        };
        out.container.format = format;
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
                id: codec_id.to_string(),
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
pub(crate) fn build_ac3_frame(fscod: u8, frmsizecod: u8) -> Vec<u8> {
    let len = (FRAME_SIZES[frmsizecod as usize][fscod as usize] as usize) * 2;
    let mut bytes = vec![0u8; len];
    bytes[0] = 0x0B;
    bytes[1] = 0x77;
    bytes[4] = (fscod << 6) | (frmsizecod & 0x3F);
    bytes[5] = 8 << 3; // bsid = 8 (AC-3)
    bytes[6] = (2 & 0x07) << 5; // acmod = 2 (stereo)
    bytes
}

#[cfg(test)]
pub(crate) fn build_ac3_stream(frames: usize, fscod: u8, frmsizecod: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..frames {
        bytes.extend(build_ac3_frame(fscod, frmsizecod));
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_ac3_stereo_48k_192kbps() {
        let frame = build_ac3_frame(0, 8);
        let f = decode_frame(&frame).unwrap();
        assert_eq!(f.variant, Ac3Variant::Ac3);
        assert_eq!(f.sample_rate, 48_000);
        assert_eq!(f.channels, 2);
    }

    #[test]
    fn channels_from_acmod_full_table() {
        let pairs = [(0, 2), (1, 1), (2, 2), (3, 3), (4, 3), (5, 4), (6, 4), (7, 5)];
        for (acmod, expected) in pairs {
            assert_eq!(channels_from_acmod(acmod), expected);
        }
    }

    #[test]
    fn rejects_invalid_sync() {
        let mut frame = build_ac3_frame(0, 8);
        frame[0] = 0x00;
        assert!(decode_frame(&frame).is_none());
    }

    #[test]
    fn rejects_invalid_fscod() {
        let mut frame = build_ac3_frame(0, 8);
        frame[4] = (3 << 6) | 8; // fscod = 3 reserved
        assert!(decode_frame(&frame).is_none());
    }

    #[test]
    fn find_frame_sync_requires_eight() {
        let bytes = build_ac3_stream(8, 0, 8);
        assert_eq!(find_frame_sync(&bytes), Some(0));
    }

    #[test]
    fn probe_accepts_ac3_stream() {
        let bytes = build_ac3_stream(10, 0, 8);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(Ac3Reader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_ac3_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_ac3_stream(10, 0, 8);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.ac3", 0);
        Ac3Reader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Ac3);
        assert_eq!(out.tracks[0].codec.id, "A_AC3");
    }

    #[test]
    fn eac3_bsid_branch_decodes_separately() {
        let mut frame = vec![0u8; 32];
        frame[0] = 0x0B;
        frame[1] = 0x77;
        // strmtyp + substreamid don't matter for this test
        frame[2] = 0x00;
        frame[3] = 0x07; // frmsiz low bits → frame_length = (7+1)*2 = 16
        frame[4] = 0x00 << 6; // fscod = 0 → 48 kHz
        frame[5] = 12 << 3; // bsid = 12 → E-AC-3
        let f = decode_frame(&frame).unwrap();
        assert_eq!(f.variant, Ac3Variant::Eac3);
        assert_eq!(f.sample_rate, 48_000);
        assert_eq!(f.frame_length, 16);
    }

    #[test]
    fn eac3_fscod_3_uses_fscod2_for_sample_rate() {
        let mut frame = vec![0u8; 32];
        frame[0] = 0x0B;
        frame[1] = 0x77;
        frame[3] = 0x07;
        frame[4] = 0b11_00_0000; // fscod = 3, fscod2 = 0 → 24 kHz
        frame[5] = 12 << 3;
        let f = decode_frame(&frame).unwrap();
        assert_eq!(f.sample_rate, 24_000);
    }
}
