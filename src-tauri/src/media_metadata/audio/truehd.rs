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

//! Dolby TrueHD / MLP reader. Pure-Rust port of
//! `mkvtoolnix/src/common/truehd.cpp` + `src/input/r_truehd.cpp`.
//!
//! The major-sync word lives four bytes into the access unit (after the
//! 16-bit access-unit length + 16-bit input timing): `0xF8726FBA` is TrueHD,
//! `0xF8726FBB` is MLP. Sample rate and channels are decoded from the proper
//! bit fields (`decode_rate_bits` / `decode_channel_map` / the MLP channel
//! table) rather than fixed byte offsets (PARSER-025). The probe requires two
//! valid sync frames found by walking the frame chain (PARSER-026), a leading
//! ID3v2 tag is skipped (PARSER-028), and an interleaved AC-3 substream is
//! surfaced as a second track (PARSER-027).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::endian::{get_u16_be, get_u32_be};
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::{ac3, id3v2};

const PROBE_BYTES: usize = 256 * 1024;
const MIN_HEADER_SIZE: usize = 12;
const TRUEHD_SYNC_WORD: u32 = 0xf872_6fba;
const MLP_SYNC_WORD: u32 = 0xf872_6fbb;
const AC3_SYNC_WORD: u16 = 0x0b77;
const MAX_FRAMES: usize = 100_000;

/// `frame_t::ms_mlp_channels` — MLP channel-arrangement → channel count.
const MLP_CHANNELS: [u8; 32] = [
    1, 2, 3, 4, 3, 4, 5, 3, 4, 5, 4, 5, 6, 4, 5, 4, 5, 6, 5, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// `decode_channel_map`'s per-bit channel counts (ffmpeg mlp_parser).
const CHANNEL_COUNT: [u32; 13] = [2, 1, 1, 2, 2, 2, 2, 1, 1, 2, 2, 1, 1];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    TrueHd,
    Mlp,
    Ac3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Normal,
    Sync,
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub codec: Codec,
    pub frame_type: FrameType,
    pub size: usize,
    pub sampling_rate: u32,
    pub channels: u32,
}

impl Frame {
    fn is_sync(&self) -> bool {
        self.frame_type == FrameType::Sync
    }
}

/// Port of `frame_t::decode_rate_bits`.
fn decode_rate_bits(rate_bits: u32) -> u32 {
    if rate_bits == 0xf {
        return 0;
    }
    (if rate_bits & 8 != 0 { 44100 } else { 48000 }) << (rate_bits & 7)
}

/// Port of `frame_t::decode_channel_map`.
fn decode_channel_map(channel_map: u32) -> u32 {
    let mut channels = 0;
    for (i, &count) in CHANNEL_COUNT.iter().enumerate() {
        channels += count * ((channel_map >> i) & 1);
    }
    channels
}

/// Port of `frame_t::parse_header`.
pub fn parse_header(data: &[u8]) -> Option<Frame> {
    if data.len() < MIN_HEADER_SIZE {
        return None;
    }
    let first_word = get_u16_be(&data[0..]);
    let sync_word = get_u32_be(&data[4..]);
    let is_thd_or_mlp = sync_word == TRUEHD_SYNC_WORD || sync_word == MLP_SYNC_WORD;

    if !is_thd_or_mlp && first_word == AC3_SYNC_WORD {
        return parse_ac3_header(data);
    }

    // 0xF8726FBA = TrueHD, 0xF8726FBB = MLP. A frame without a sync word is a
    // "normal" (continuation) frame and is treated as TrueHD.
    let codec = if !is_thd_or_mlp || sync_word == TRUEHD_SYNC_WORD {
        Codec::TrueHd
    } else {
        Codec::Mlp
    };
    let frame_type = if is_thd_or_mlp {
        FrameType::Sync
    } else {
        FrameType::Normal
    };
    let size = ((first_word & 0xfff) as usize) * 2;

    if frame_type == FrameType::Normal {
        return Some(Frame {
            codec,
            frame_type,
            size,
            sampling_rate: 0,
            channels: 0,
        });
    }

    if codec == Codec::TrueHd {
        parse_truehd_header(data, size)
    } else {
        parse_mlp_header(data, size)
    }
}

fn parse_ac3_header(data: &[u8]) -> Option<Frame> {
    let frame = ac3::decode_frame(data)?;
    Some(Frame {
        codec: Codec::Ac3,
        frame_type: FrameType::Sync,
        size: frame.frame_length,
        sampling_rate: frame.sample_rate,
        channels: frame.channels,
    })
}

/// Port of `frame_t::parse_mlp_header`.
fn parse_mlp_header(data: &[u8], size: usize) -> Option<Frame> {
    if data.len() < 12 {
        return None;
    }
    let sampling_rate = decode_rate_bits((data[9] >> 4) as u32);
    let channels = MLP_CHANNELS[(data[11] & 0x1f) as usize] as u32;
    Some(Frame {
        codec: Codec::Mlp,
        frame_type: FrameType::Sync,
        size,
        sampling_rate,
        channels,
    })
}

/// Port of `frame_t::parse_truehd_header` (channel/sample-rate portion).
fn parse_truehd_header(data: &[u8], size: usize) -> Option<Frame> {
    let mut r = BitReader::new(data);
    let result = (|| -> Result<(u32, u32), ParseError> {
        r.skip_bits(4 + 12)?; // check_nibble + access_unit_length
        r.read_bits(16)?; // input_timing
        r.skip_bits(32)?; // format_sync

        let rate_bits = r.read_bits(4)? as u32;
        let sampling_rate = decode_rate_bits(rate_bits);
        r.skip_bits(4)?;

        r.skip_bits(4)?;
        let chanmap_substream_1 = r.read_bits(5)? as u32;
        r.skip_bits(2)?;
        let chanmap_substream_2 = r.read_bits(13)? as u32;
        let channels = decode_channel_map(if chanmap_substream_2 != 0 {
            chanmap_substream_2
        } else {
            chanmap_substream_1
        });
        Ok((sampling_rate, channels))
    })();

    let (sampling_rate, channels) = result.ok()?;
    Some(Frame {
        codec: Codec::TrueHd,
        frame_type: FrameType::Sync,
        size,
        sampling_rate,
        channels,
    })
}

/// Port of `parser_c::resync`. Returns the frame-start offset of the next
/// position whose header decodes, or `None`.
fn resync(data: &[u8], start: usize) -> Option<usize> {
    let size = data.len();
    let mut offset = start + 4;
    while offset + 4 < size {
        let sync_word = get_u32_be(&data[offset..]);
        let ac3_here = offset >= 4 && get_u16_be(&data[offset - 4..]) == AC3_SYNC_WORD;
        if (sync_word == TRUEHD_SYNC_WORD || sync_word == MLP_SYNC_WORD || ac3_here)
            && offset >= 4
            && parse_header(&data[offset - 4..]).is_some()
        {
            return Some(offset - 4);
        }
        offset += 1;
    }
    None
}

/// Port of `parser_c::parse`: resync, then walk the frame chain, propagating
/// the sync codec onto subsequent normal frames.
pub fn parse_frames(data: &[u8]) -> Vec<Frame> {
    let mut frames = Vec::new();
    let size = data.len();
    if size < MIN_HEADER_SIZE {
        return frames;
    }
    let Some(mut offset) = resync(data, 0) else {
        return frames;
    };
    let mut sync_codec = Codec::TrueHd;

    while size - offset >= MIN_HEADER_SIZE && frames.len() < MAX_FRAMES {
        let Some(mut frame) = parse_header(&data[offset..]) else {
            break;
        };

        if frame.size < 8 {
            match resync(data, offset + 1) {
                Some(o) => {
                    offset = o;
                    continue;
                }
                None => break,
            }
        }

        if frame.size + offset > size {
            break;
        }

        if matches!(frame.codec, Codec::TrueHd | Codec::Mlp) {
            if frame.is_sync() {
                sync_codec = frame.codec;
            } else {
                frame.codec = sync_codec;
            }
        }

        offset += frame.size;
        frames.push(frame);
    }

    frames
}

/// Byte offset where the stream starts, skipping a leading ID3v2 tag
/// (PARSER-028).
fn payload_start(src: &mut FileSource) -> Result<u64, ParseError> {
    let mut head = [0u8; 10];
    let n = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if n == 10 {
        Ok(id3v2::skip_id3v2(&head).unwrap_or(0) as u64)
    } else {
        Ok(0)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TrueHdReader;

impl Reader for TrueHdReader {
    fn name(&self) -> &'static str {
        "truehd"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let start = payload_start(src)?;
        src.seek_to(start)?;
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        src.seek_to(0)?;
        if read < MIN_HEADER_SIZE {
            return Ok(false);
        }
        // find_valid_headers(.., 2): count non-AC-3 sync frames.
        let sync_frames = parse_frames(&probe[..read])
            .into_iter()
            .filter(|f| f.is_sync() && f.codec != Codec::Ac3)
            .count();
        Ok(sync_frames >= 2)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let start = payload_start(src)?;
        src.seek_to(start)?;
        let mut probe = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut probe)?;
        let frames = parse_frames(&probe[..read]);

        // First non-AC-3 sync frame is the authoritative TrueHD/MLP header;
        // the first AC-3 frame becomes a second track (PARSER-027).
        let header = frames
            .iter()
            .find(|f| f.is_sync() && f.codec != Codec::Ac3)
            .copied()
            .ok_or(ParseError::Unrecognised)?;
        let ac3_frame = frames.iter().find(|f| f.codec == Codec::Ac3).copied();

        out.container.format = ContainerFormat::TrueHd;
        out.container.recognized = true;
        out.container.supported = true;

        push_audio_track(out, 0, header);
        if let Some(ac3) = ac3_frame {
            push_audio_track(out, 1, ac3);
        }
        Ok(())
    }
}

fn push_audio_track(out: &mut MediaMetadata, id: i64, frame: Frame) {
    let (codec_id, codec_name) = match frame.codec {
        Codec::TrueHd => ("A_TRUEHD", "TrueHD"),
        Codec::Mlp => ("A_MLP", "MLP"),
        Codec::Ac3 => ("A_AC3", "AC-3"),
    };
    let mut common = CommonTrackProperties::default();
    common.number = Some((id + 1) as u64);
    let mut audio = AudioTrackProperties::default();
    if frame.sampling_rate > 0 {
        audio.sampling_frequency = Some(frame.sampling_rate as f64);
    }
    if frame.channels > 0 {
        audio.channels = Some(frame.channels);
    }
    out.tracks.push(Track {
        id,
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
}

#[cfg(test)]
mod test_support {
    pub struct BitWriter {
        pub bytes: Vec<u8>,
        bit_pos: usize,
    }

    impl BitWriter {
        pub fn new() -> Self {
            BitWriter { bytes: Vec::new(), bit_pos: 0 }
        }
        pub fn put(&mut self, n: u32, value: u64) {
            for i in (0..n).rev() {
                let bit = ((value >> i) & 1) as u8;
                let byte_idx = self.bit_pos / 8;
                if byte_idx >= self.bytes.len() {
                    self.bytes.push(0);
                }
                if bit != 0 {
                    self.bytes[byte_idx] |= 0x80 >> (self.bit_pos % 8);
                }
                self.bit_pos += 1;
            }
        }
    }
}

/// Build a 32-byte TrueHD sync frame. `chanmap` populates chanmap_substream_2.
#[cfg(test)]
pub(crate) fn build_truehd_frame(rate_bits: u32, chanmap: u32) -> Vec<u8> {
    use test_support::BitWriter;
    let mut w = BitWriter::new();
    w.put(4, 0); // check_nibble
    w.put(12, 16); // access_unit_length → size = 32 bytes
    w.put(16, 0); // input_timing
    w.put(32, TRUEHD_SYNC_WORD as u64);
    w.put(4, rate_bits as u64);
    w.put(4, 0); // (skip)
    w.put(4, 0); // (skip before chanmap_substream_1)
    w.put(5, 0); // chanmap_substream_1
    w.put(2, 0); // (skip)
    w.put(13, chanmap as u64); // chanmap_substream_2
    let mut bytes = w.bytes;
    bytes.resize(32, 0);
    bytes
}

/// Build a 32-byte MLP sync frame.
#[cfg(test)]
pub(crate) fn build_mlp_frame(rate_bits: u8, mlp_channel_idx: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    // first_word: access_unit_length 16 → size 32.
    bytes[0] = 0x00;
    bytes[1] = 0x10;
    bytes[4] = 0xf8;
    bytes[5] = 0x72;
    bytes[6] = 0x6f;
    bytes[7] = 0xbb; // MLP sync
    bytes[9] = rate_bits << 4;
    bytes[11] = mlp_channel_idx & 0x1f;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decode_rate_bits_table() {
        assert_eq!(decode_rate_bits(0), 48_000);
        assert_eq!(decode_rate_bits(1), 96_000);
        assert_eq!(decode_rate_bits(2), 192_000);
        assert_eq!(decode_rate_bits(8), 44_100);
        assert_eq!(decode_rate_bits(9), 88_200);
        assert_eq!(decode_rate_bits(0xf), 0);
    }

    #[test]
    fn decode_channel_map_counts() {
        assert_eq!(decode_channel_map(0b1), 2); // L/R
        assert_eq!(decode_channel_map(0b111), 4); // L/R + C + LFE
        assert_eq!(decode_channel_map(0b1111), 6); // + Ls/Rs
    }

    // ---- PARSER-025: correct sample-rate / channel decode -----------------

    #[test]
    fn truehd_sync_decodes_rate_and_channels() {
        let frame = build_truehd_frame(1, 0b1111); // 96 kHz, 6 channels
        let f = parse_header(&frame).unwrap();
        assert_eq!(f.codec, Codec::TrueHd);
        assert!(f.is_sync());
        assert_eq!(f.sampling_rate, 96_000);
        assert_eq!(f.channels, 6);
        assert_eq!(f.size, 32);
    }

    #[test]
    fn mlp_sync_uses_mlp_channel_table() {
        // mlp channel idx 2 → MLP_CHANNELS[2] = 3 channels; rate_bits 0 → 48k.
        let frame = build_mlp_frame(0, 2);
        let f = parse_header(&frame).unwrap();
        assert_eq!(f.codec, Codec::Mlp);
        assert_eq!(f.sampling_rate, 48_000);
        assert_eq!(f.channels, 3);
    }

    #[test]
    fn truehd_and_mlp_sync_word_assignment() {
        // 0xBA → TrueHD, 0xBB → MLP (was reversed before).
        let mut thd = build_truehd_frame(0, 1);
        assert_eq!(thd[7], 0xba);
        assert_eq!(parse_header(&thd).unwrap().codec, Codec::TrueHd);
        thd[7] = 0xbb;
        assert_eq!(parse_header(&thd).unwrap().codec, Codec::Mlp);
    }

    // ---- PARSER-026: probe requires two sync frames -----------------------

    #[test]
    fn probe_requires_two_sync_frames() {
        let single = build_truehd_frame(1, 1);
        let mut s = FileSource::from_reader_for_test(Cursor::new(single));
        assert!(!TrueHdReader.probe(&mut s).unwrap());

        let mut two = build_truehd_frame(1, 1);
        two.extend(build_truehd_frame(1, 1));
        let mut s = FileSource::from_reader_for_test(Cursor::new(two));
        assert!(TrueHdReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_garbage() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 4096]));
        assert!(!TrueHdReader.probe(&mut s).unwrap());
    }

    #[test]
    fn parse_frames_walks_consecutive_syncs() {
        let mut data = build_truehd_frame(2, 0b1111);
        data.extend(build_truehd_frame(2, 0b1111));
        data.extend(build_truehd_frame(2, 0b1111));
        let frames = parse_frames(&data);
        let syncs = frames.iter().filter(|f| f.is_sync()).count();
        assert_eq!(syncs, 3);
    }

    // ---- PARSER-027: embedded AC-3 substream ------------------------------

    #[test]
    fn read_headers_exposes_ac3_substream() {
        use crate::media_metadata::audio::ac3::build_ac3_frame;
        let mut data = build_truehd_frame(1, 0b1111);
        data.extend(build_truehd_frame(1, 0b1111));
        data.extend(build_ac3_frame(0, 8)); // AC-3 48 kHz frame
        let mut s = FileSource::from_reader_for_test(Cursor::new(data));
        let mut out = MediaMetadata::new("clip.thd", 0);
        TrueHdReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 2);
        assert_eq!(out.tracks[0].codec.id, "A_TRUEHD");
        assert_eq!(out.tracks[1].codec.id, "A_AC3");
    }

    #[test]
    fn read_headers_emits_truehd_track() {
        let mut data = build_truehd_frame(2, 0b1111);
        data.extend(build_truehd_frame(2, 0b1111));
        let mut s = FileSource::from_reader_for_test(Cursor::new(data));
        let mut out = MediaMetadata::new("clip.thd", 0);
        TrueHdReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::TrueHd);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.sampling_frequency, Some(192_000.0));
        assert_eq!(a.channels, Some(6));
    }

    // ---- PARSER-028: ID3v2 prefix ----------------------------------------

    #[test]
    fn probe_skips_leading_id3v2_tag() {
        let mut bytes = crate::media_metadata::audio::id3v2::build_id3v2_tag(false, 128);
        bytes.extend(build_truehd_frame(1, 1));
        bytes.extend(build_truehd_frame(1, 1));
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(TrueHdReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_rejects_garbage() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 4096]));
        let mut out = MediaMetadata::new("x.thd", 0);
        assert!(TrueHdReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .is_err());
    }
}
