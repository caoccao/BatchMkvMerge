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

//! `esds` — MPEG-4 Elementary Stream Descriptor (ISO/IEC 14496-1).
//!
//! Used by AAC, MP3-in-MP4 and other MPEG-4-system streams.  The descriptor
//! is a nested TLV tree of MPEG-4 BER-encoded objects.  We walk just enough
//! of it to extract:
//!
//! - `objectTypeIndication` (e.g. 0x40 = AAC).
//! - `streamType` / `bufferSizeDB` / `maxBitrate` / `avgBitrate`.
//! - `DecoderSpecificInfo` (AudioSpecificConfig for AAC).
//!
//! AudioSpecificConfig is then bit-decoded to populate `AudioCodecConfig`
//! (object type, sample-rate index, channel config, SBR/PS extension flags).

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::AudioCodecConfig;

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

const MAX_PAYLOAD: u64 = 64 * 1024;
const TAG_ES_DESCRIPTOR: u8 = 0x03;
const TAG_DECODER_CONFIG: u8 = 0x04;
const TAG_DEC_SPECIFIC_INFO: u8 = 0x05;

pub fn parse(
    src: &mut FileSource,
    header: &BoxHeader,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    let payload = atom::read_payload(src, header, MAX_PAYLOAD)?;
    if payload.len() < 4 {
        return Ok(());
    }
    // 4-byte FullBox header (version + flags).
    let body = &payload[4..];
    let mut cursor = Cursor { data: body, pos: 0 };
    let mut cfg = AudioCodecConfig::default();
    cfg.raw_hex = Some(hex_encode(&payload));
    walk(&mut cursor, &mut cfg);
    builder.audio_codec_config = Some(cfg);
    builder.codec_private_hex = Some(hex_encode(&payload));
    Ok(())
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn read_u16_be(&mut self) -> Option<u16> {
        if self.pos + 2 > self.data.len() {
            return None;
        }
        let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Some(v)
    }
    fn read_u32_be(&mut self) -> Option<u32> {
        if self.pos + 4 > self.data.len() {
            return None;
        }
        let v = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Some(v)
    }
    fn slice(&self, len: usize) -> Option<&'a [u8]> {
        self.data.get(self.pos..self.pos + len)
    }
    fn skip(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.data.len());
    }
    /// MPEG-4 BER-encoded length (max 4 bytes, 7-bit chunks).
    fn read_ber_length(&mut self) -> Option<usize> {
        let mut value = 0usize;
        for _ in 0..4 {
            let b = self.read_u8()?;
            value = (value << 7) | ((b & 0x7F) as usize);
            if b & 0x80 == 0 {
                return Some(value);
            }
        }
        Some(value)
    }
}

fn walk(cursor: &mut Cursor, cfg: &mut AudioCodecConfig) {
    while let Some(tag) = cursor.read_u8() {
        let len = match cursor.read_ber_length() {
            Some(l) => l,
            None => return,
        };
        let body_end = (cursor.pos + len).min(cursor.data.len());
        match tag {
            TAG_ES_DESCRIPTOR => {
                let _esid = cursor.read_u16_be();
                let flags = cursor.read_u8().unwrap_or(0);
                // streamDependenceFlag(1) | URL_Flag(1) | OCRStreamFlag(1) | streamPriority(5)
                if flags & 0x80 != 0 {
                    cursor.skip(2); // dependsOn_ES_ID
                }
                if flags & 0x40 != 0 {
                    if let Some(url_len) = cursor.read_u8() {
                        cursor.skip(url_len as usize);
                    }
                }
                if flags & 0x20 != 0 {
                    cursor.skip(2); // OCR_ES_Id
                }
                // recurse into the rest of the ES descriptor
                let mut inner = Cursor {
                    data: &cursor.data[cursor.pos..body_end],
                    pos: 0,
                };
                walk(&mut inner, cfg);
                cursor.pos = body_end;
            }
            TAG_DECODER_CONFIG => {
                let object_type = cursor.read_u8().unwrap_or(0);
                let _stream_type = cursor.read_u8();
                let _buffer = cursor.read_u8().is_some()
                    && cursor.read_u8().is_some()
                    && cursor.read_u8().is_some();
                let max_bitrate = cursor.read_u32_be();
                let avg_bitrate = cursor.read_u32_be();
                cfg.profile_name = Some(format_object_type(object_type).to_string());
                let _ = (max_bitrate, avg_bitrate); // identification doesn't expose bitrates
                // recurse to pick up the nested DecSpecificInfo
                let mut inner = Cursor {
                    data: &cursor.data[cursor.pos..body_end],
                    pos: 0,
                };
                walk(&mut inner, cfg);
                cursor.pos = body_end;
            }
            TAG_DEC_SPECIFIC_INFO => {
                if let Some(slice) = cursor.slice(len) {
                    parse_audio_specific_config(slice, cfg);
                }
                cursor.pos = body_end;
            }
            _ => {
                cursor.pos = body_end;
            }
        }
    }
}

fn parse_audio_specific_config(bytes: &[u8], cfg: &mut AudioCodecConfig) {
    if bytes.is_empty() {
        return;
    }
    let mut reader = BitCursor { data: bytes, pos: 0 };
    let aot = read_audio_object_type(&mut reader);
    let sample_rate_index = reader.read_bits(4) as u32;
    let _sample_rate = if sample_rate_index == 0xF {
        reader.read_bits(24)
    } else {
        sample_rate_from_index(sample_rate_index as u8)
    };
    let channel_config = reader.read_bits(4) as u32;
    cfg.aac_object_type = Some(aot);
    cfg.aac_frame_length = Some(match aot {
        3 => 768,
        _ => 1024,
    });
    let _ = channel_config; // channels already filled from sample entry

    // Extension: object type 5 (SBR) or 29 (PS) signal presence.
    let sbr_present = matches!(aot, 5 | 29);
    let ps_present = aot == 29;
    cfg.aac_sbr_present = Some(sbr_present);
    cfg.aac_ps_present = Some(ps_present);
}

fn read_audio_object_type(cursor: &mut BitCursor) -> u32 {
    let mut aot = cursor.read_bits(5) as u32;
    if aot == 31 {
        aot = 32 + cursor.read_bits(6) as u32;
    }
    aot
}

struct BitCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitCursor<'a> {
    fn read_bits(&mut self, n: u32) -> u64 {
        let mut value: u64 = 0;
        for _ in 0..n {
            let byte_idx = self.pos / 8;
            let bit_idx = 7 - (self.pos % 8) as u32;
            let bit = if byte_idx < self.data.len() {
                ((self.data[byte_idx] >> bit_idx) & 0x01) as u64
            } else {
                0
            };
            value = (value << 1) | bit;
            self.pos += 1;
        }
        value
    }
}

fn sample_rate_from_index(idx: u8) -> u64 {
    const TABLE: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    TABLE.get(idx as usize).copied().unwrap_or(0) as u64
}

fn format_object_type(idc: u8) -> &'static str {
    match idc {
        0x40 => "AAC",
        0x41 => "AAC main",
        0x6B => "MP3 (MPEG-1 Layer III)",
        0x69 => "MP3 (MPEG-2 Layer III)",
        0x67 => "MPEG-2 AAC",
        0xA5 => "AC-3",
        0xA6 => "E-AC-3",
        0xA9 => "DTS",
        0xDD => "Vorbis",
        _ => "Unknown",
    }
}

#[cfg(test)]
pub(crate) fn build_esds_payload(object_type: u8, audio_specific_config: &[u8]) -> Vec<u8> {
    // FullBox header
    let mut p = vec![0u8; 4];
    // ES descriptor:  ES_ID(2) + flags(1) = 3 bytes header + DecoderConfig inline
    let dec_specific = {
        let mut v = vec![TAG_DEC_SPECIFIC_INFO];
        v.push(audio_specific_config.len() as u8);
        v.extend_from_slice(audio_specific_config);
        v
    };
    let dec_config = {
        let mut v = vec![TAG_DECODER_CONFIG];
        let body_len = 13 + dec_specific.len();
        v.push(body_len as u8); // 1-byte BER length
        v.push(object_type);
        v.push(0x15); // streamType + flags
        v.extend_from_slice(&[0u8; 3]); // bufferSizeDB
        v.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate
        v.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate
        v.extend_from_slice(&dec_specific);
        v
    };
    let es_descriptor = {
        let mut v = vec![TAG_ES_DESCRIPTOR];
        let body_len = 3 + dec_config.len();
        v.push(body_len as u8);
        v.extend_from_slice(&[0u8; 2]); // ES_ID
        v.push(0); // flags
        v.extend_from_slice(&dec_config);
        v
    };
    p.extend_from_slice(&es_descriptor);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mp4::atom::encode_box;
    use std::io::Cursor as StdCursor;

    fn run(payload: Vec<u8>) -> TrackBuilder {
        let bytes = encode_box(b"esds", &payload);
        let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        parse(&mut s, &h, &mut b).unwrap();
        b
    }

    fn aac_lc_specific_config(sample_rate_idx: u8, channels: u8) -> Vec<u8> {
        // 5 bits AOT (2 = AAC LC) + 4 bits sample_rate_idx + 4 bits channels
        let aot = 2u16;
        let value = (aot << 11) | ((sample_rate_idx as u16) << 7) | ((channels as u16) << 3);
        vec![(value >> 8) as u8, (value & 0xFF) as u8]
    }

    #[test]
    fn aac_object_type_decoded() {
        let asc = aac_lc_specific_config(4, 2); // 44.1k stereo
        let payload = build_esds_payload(0x40, &asc);
        let b = run(payload);
        let cfg = b.audio_codec_config.unwrap();
        assert_eq!(cfg.aac_object_type, Some(2));
        assert_eq!(cfg.aac_frame_length, Some(1024));
        assert_eq!(cfg.profile_name.as_deref(), Some("AAC"));
    }

    #[test]
    fn aac_sbr_extension_detected() {
        // AOT 5 = SBR
        let value = (5u16 << 11) | (4u16 << 7) | (2u16 << 3);
        let asc = vec![(value >> 8) as u8, (value & 0xFF) as u8];
        let payload = build_esds_payload(0x40, &asc);
        let b = run(payload);
        let cfg = b.audio_codec_config.unwrap();
        assert_eq!(cfg.aac_sbr_present, Some(true));
        assert_eq!(cfg.aac_ps_present, Some(false));
    }

    #[test]
    fn aac_ps_extension_detected() {
        // AOT 29 = PS
        let value = (29u16 << 11) | (4u16 << 7) | (2u16 << 3);
        let asc = vec![(value >> 8) as u8, (value & 0xFF) as u8];
        let payload = build_esds_payload(0x40, &asc);
        let b = run(payload);
        let cfg = b.audio_codec_config.unwrap();
        assert_eq!(cfg.aac_ps_present, Some(true));
    }

    #[test]
    fn raw_hex_round_trips() {
        let asc = aac_lc_specific_config(4, 2);
        let payload = build_esds_payload(0x40, &asc);
        let b = run(payload.clone());
        let raw = b.audio_codec_config.unwrap().raw_hex.unwrap();
        let decoded: Vec<u8> = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn empty_payload_is_noop() {
        // Box with only 4-byte FullBox header
        let bytes = encode_box(b"esds", &[0u8; 4]);
        let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        parse(&mut s, &h, &mut b).unwrap();
        assert!(b.audio_codec_config.is_some()); // raw_hex populated, others None
        let cfg = b.audio_codec_config.unwrap();
        assert!(cfg.aac_object_type.is_none());
    }

    #[test]
    fn ber_length_handles_multibyte() {
        // 0x81 0x01 = 7-bit length = 129 ... ensure no panic
        let mut cur = Cursor { data: &[0x81, 0x01, 0x00], pos: 0 };
        assert_eq!(cur.read_ber_length(), Some(129));
    }

    #[test]
    fn sample_rate_table_known_values() {
        assert_eq!(sample_rate_from_index(0), 96000);
        assert_eq!(sample_rate_from_index(3), 48000);
        assert_eq!(sample_rate_from_index(12), 7350);
        assert_eq!(sample_rate_from_index(15), 0); // out of range
    }

    #[test]
    fn object_type_table() {
        assert_eq!(format_object_type(0x40), "AAC");
        assert_eq!(format_object_type(0xA5), "AC-3");
        assert_eq!(format_object_type(0xDD), "Vorbis");
        assert_eq!(format_object_type(0xFE), "Unknown");
    }
}
