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

//! AV1 Open Bitstream Units (AV1 §5.3).
//!
//! OBU header layout:
//!
//! ```text
//! 1 bit obu_forbidden_bit
//! 4 bits obu_type (1 = sequence_header, 6 = frame, ...)
//! 1 bit obu_extension_flag
//! 1 bit obu_has_size_field
//! 1 bit obu_reserved_1bit
//! [if extension] u8 temporal_id(3) | spatial_id(2) | reserved(3)
//! [if has_size_field] LEB128 size
//! ```
//!
//! Sequence header OBU body decode (AV1 §5.5):
//!
//! ```text
//! 3 bits seq_profile
//! 1 bit  still_picture
//! 1 bit  reduced_still_picture_header
//! ...
//! ```
//!
//! For identification we expose profile + max resolution + bit depth + monochrome.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
    ChromaFormat, Dimensions2D, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 64 * 1024;
const OBU_TYPE_SEQUENCE_HEADER: u8 = 1;
const OBU_TYPE_TEMPORAL_DELIMITER: u8 = 2;

#[derive(Debug, Clone, Copy)]
pub struct ObuHeader {
    pub obu_type: u8,
    pub has_extension: bool,
    pub has_size_field: bool,
}

pub fn decode_header(byte: u8) -> ObuHeader {
    let obu_type = (byte >> 3) & 0x0F;
    let has_extension = (byte >> 2) & 0x01 != 0;
    let has_size_field = (byte >> 1) & 0x01 != 0;
    ObuHeader {
        obu_type,
        has_extension,
        has_size_field,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SequenceHeader {
    pub seq_profile: u8,
    pub max_width: u32,
    pub max_height: u32,
    pub bit_depth: u8,
    pub monochrome: bool,
    pub subsampling_x: u8,
    pub subsampling_y: u8,
}

/// Decode a sequence-header OBU body (AV1 §5.5).  We stop after extracting
/// the fields identification cares about.
pub fn decode_sequence_header(body: &[u8]) -> Result<SequenceHeader, ParseError> {
    if body.is_empty() {
        return Err(ParseError::Malformed {
            format: "av1",
            offset: 0,
            reason: "empty sequence_header OBU".to_string(),
        });
    }
    let mut reader = BitReader::from_rbsp(body);
    let seq_profile = reader.read_bits(3)? as u8;
    let _still_picture = reader.read_bit()?;
    let reduced_still_picture = reader.read_bit()?;
    if reduced_still_picture {
        let _seq_level_idx = reader.read_bits(5)?;
    } else {
        let timing_info_present = reader.read_bit()?;
        let mut decoder_model_info_present = false;
        if timing_info_present {
            let _num_units_in_display_tick = reader.read_bits(32)?;
            let _time_scale = reader.read_bits(32)?;
            let equal_picture_interval = reader.read_bit()?;
            if equal_picture_interval {
                let _ = read_uvlc(&mut reader)?;
            }
            decoder_model_info_present = reader.read_bit()?;
            if decoder_model_info_present {
                let _bdl = reader.read_bits(5)?;
                let _eaadt = reader.read_bits(32)?;
                let _ec = reader.read_bits(10)?;
                let _ftpdc = reader.read_bits(5)?;
            }
        }
        let initial_display_delay_present_flag = reader.read_bit()?;
        let operating_points_cnt_minus_1 = reader.read_bits(5)? as u32;
        for _ in 0..=operating_points_cnt_minus_1 {
            let _idc = reader.read_bits(12)?;
            let seq_level_idx = reader.read_bits(5)?;
            if seq_level_idx > 7 {
                let _ = reader.read_bit()?; // seq_tier
            }
            if decoder_model_info_present {
                let present = reader.read_bit()?;
                if present {
                    let _ = reader.read_bits(5 + 5)?; // bitrate + buffer scale skipped
                }
            }
            if initial_display_delay_present_flag {
                let present = reader.read_bit()?;
                if present {
                    let _ = reader.read_bits(4)?;
                }
            }
        }
    }
    let frame_width_bits = reader.read_bits(4)? as u32 + 1;
    let frame_height_bits = reader.read_bits(4)? as u32 + 1;
    let max_frame_width = reader.read_bits(frame_width_bits)? as u32 + 1;
    let max_frame_height = reader.read_bits(frame_height_bits)? as u32 + 1;

    // Skip ahead to colour_config — the intermediate fields are reduced
    // when reduced_still_picture is set, but in either case the next bits
    // we need (bit_depth + monochrome + subsampling) come after a few flags
    // we don't care about here.  Per AV1 §5.5.4:
    if !reduced_still_picture {
        let frame_id_numbers_present = reader.read_bit()?;
        if frame_id_numbers_present {
            let _delta_frame_id_length_minus_2 = reader.read_bits(4)?;
            let _additional_frame_id_length_minus_1 = reader.read_bits(3)?;
        }
    }
    let _use_128x128_superblock = reader.read_bit()?;
    let _enable_filter_intra = reader.read_bit()?;
    let _enable_intra_edge_filter = reader.read_bit()?;
    if !reduced_still_picture {
        let _enable_interintra_compound = reader.read_bit()?;
        let _enable_masked_compound = reader.read_bit()?;
        let _enable_warped_motion = reader.read_bit()?;
        let _enable_dual_filter = reader.read_bit()?;
        let enable_order_hint = reader.read_bit()?;
        if enable_order_hint {
            let _ = reader.read_bit()?; // enable_jnt_comp
            let _ = reader.read_bit()?; // enable_ref_frame_mvs
        }
        let seq_choose_screen_detection_tools = reader.read_bit()?;
        if !seq_choose_screen_detection_tools {
            let _ = reader.read_bit()?; // seq_force_screen_content_tools
        }
        let seq_choose_integer_mv = reader.read_bit()?;
        if !seq_choose_integer_mv {
            let _ = reader.read_bit()?;
        }
        if enable_order_hint {
            let _order_hint_bits_minus_1 = reader.read_bits(3)?;
        }
    }
    let _enable_superres = reader.read_bit()?;
    let _enable_cdef = reader.read_bit()?;
    let _enable_restoration = reader.read_bit()?;
    // Color config
    let high_bitdepth = reader.read_bit()?;
    let twelve_bit = if seq_profile == 2 && high_bitdepth {
        reader.read_bit()?
    } else {
        false
    };
    let bit_depth: u8 = if twelve_bit {
        12
    } else if high_bitdepth {
        10
    } else {
        8
    };
    let monochrome = if seq_profile != 1 {
        reader.read_bit()?
    } else {
        false
    };
    let _color_description_present = reader.read_bit()?;
    // color_description fields skipped — identification doesn't need them.
    // subsampling — different rules per profile; safe defaults per AV1 §5.5.2
    let (subsampling_x, subsampling_y) = match seq_profile {
        0 => (1, 1), // 4:2:0
        1 => (0, 0), // 4:4:4
        2 if bit_depth == 12 => (1, 0), // 4:2:2 only at 12-bit
        _ => (1, 1),
    };
    Ok(SequenceHeader {
        seq_profile,
        max_width: max_frame_width,
        max_height: max_frame_height,
        bit_depth,
        monochrome,
        subsampling_x,
        subsampling_y,
    })
}

fn read_uvlc(reader: &mut BitReader<'_>) -> Result<u32, ParseError> {
    let mut leading_zeros = 0u32;
    loop {
        let done = reader.read_bit()?;
        if done {
            break;
        }
        leading_zeros += 1;
        if leading_zeros > 32 {
            return Err(ParseError::Malformed {
                format: "av1",
                offset: 0,
                reason: "uvlc too large".to_string(),
            });
        }
    }
    if leading_zeros >= 32 {
        return Ok(u32::MAX);
    }
    let value = reader.read_bits(leading_zeros)? as u32;
    Ok(value + (1u32 << leading_zeros) - 1)
}

/// Walk the OBU stream looking for a sequence_header.
pub fn find_sequence_header(bytes: &[u8]) -> Option<&[u8]> {
    let mut pos = 0usize;
    while pos < bytes.len() {
        let header = decode_header(bytes[pos]);
        pos += 1;
        if header.has_extension {
            if pos >= bytes.len() {
                return None;
            }
            pos += 1;
        }
        let payload_len = if header.has_size_field {
            let (size, consumed) = read_leb128(&bytes[pos..])?;
            pos += consumed;
            size
        } else {
            bytes.len().saturating_sub(pos)
        };
        let payload_end = pos.saturating_add(payload_len).min(bytes.len());
        if header.obu_type == OBU_TYPE_SEQUENCE_HEADER {
            return Some(&bytes[pos..payload_end]);
        }
        pos = payload_end;
    }
    None
}

fn read_leb128(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0u64;
    let mut consumed = 0usize;
    for i in 0..8 {
        if i >= bytes.len() {
            return None;
        }
        let b = bytes[i];
        value |= ((b & 0x7F) as u64) << (i * 7);
        consumed += 1;
        if b & 0x80 == 0 {
            return Some((value as usize, consumed));
        }
    }
    Some((value as usize, consumed))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ObuReader;

impl Reader for ObuReader {
    fn name(&self) -> &'static str {
        "obu"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 2 {
            return Ok(false);
        }
        // Strict: require a temporal_delimiter OBU as the first byte and a
        // sequence_header somewhere in the probe window.  Accepting a bare
        // sequence_header collides with codecs whose first byte happens to
        // decode as type 1 (e.g. AC-3's `0x0B` → type=1, has_size_field=1).
        let header = decode_header(head[0]);
        if header.obu_type != OBU_TYPE_TEMPORAL_DELIMITER || head[0] & 0x80 != 0 {
            return Ok(false);
        }
        Ok(find_sequence_header(&head[..read]).is_some())
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
        let seq_body = find_sequence_header(&buf[..read]).ok_or(ParseError::Unrecognised)?;
        let seq = decode_sequence_header(seq_body)?;

        out.container.format = ContainerFormat::Av1Obu;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let chroma = if seq.monochrome {
            ChromaFormat::Monochrome
        } else {
            match (seq.subsampling_x, seq.subsampling_y) {
                (1, 1) => ChromaFormat::Yuv420,
                (1, 0) => ChromaFormat::Yuv422,
                (0, 0) => ChromaFormat::Yuv444,
                _ => ChromaFormat::Other,
            }
        };
        let video = VideoTrackProperties {
            pixel_dimensions: Some(Dimensions2D {
                width: seq.max_width,
                height: seq.max_height,
            }),
            display_dimensions: Some(Dimensions2D {
                width: seq.max_width,
                height: seq.max_height,
            }),
            codec_config: Some(VideoCodecConfig {
                profile_idc: Some(seq.seq_profile as u32),
                profile_name: Some(format_av1_profile(seq.seq_profile).to_string()),
                bit_depth_luma: Some(seq.bit_depth as u32),
                bit_depth_chroma: Some(seq.bit_depth as u32),
                chroma_format: Some(chroma),
                is_elementary_stream: Some(true),
                ..VideoCodecConfig::default()
            }),
            ..VideoTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: "V_AV1".to_string(),
                name: Some("AV1".to_string()),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                video: Some(video),
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

pub fn format_av1_profile(profile: u8) -> &'static str {
    match profile {
        0 => "Main",
        1 => "High",
        2 => "Professional",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_header_reads_type_and_flags() {
        // OBU type 1 (sequence header), no extension, no size field
        let byte = (1 << 3) & 0xF8; // bits 7..4 zero, bits 3..0 = 1 << 3 -- but type is in bits 3..6
        // Re-craft cleanly: forbidden=0, type=1, ext=0, has_size=1, reserved=0
        // → 0_0001_0_1_0 = 0x0A
        let h = decode_header(0x0A);
        assert_eq!(h.obu_type, 1);
        assert!(!h.has_extension);
        assert!(h.has_size_field);
        let _ = byte;
    }

    #[test]
    fn leb128_single_byte() {
        let (v, n) = read_leb128(&[0x05]).unwrap();
        assert_eq!(v, 5);
        assert_eq!(n, 1);
    }

    #[test]
    fn leb128_multi_byte() {
        // 130 = 0b10000010 → bytes 0x82 0x01
        let (v, n) = read_leb128(&[0x82, 0x01]).unwrap();
        assert_eq!(v, 130);
        assert_eq!(n, 2);
    }

    #[test]
    fn leb128_truncated_returns_none() {
        assert!(read_leb128(&[]).is_none());
    }

    #[test]
    fn find_sequence_header_in_stream() {
        // Single OBU: header byte = sequence header, has_size = 1, size = 4, then 4 bytes body
        let bytes = vec![0x0A, 0x04, 0xAA, 0xBB, 0xCC, 0xDD];
        let body = find_sequence_header(&bytes).unwrap();
        assert_eq!(body, &[0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn format_av1_profile_table() {
        assert_eq!(format_av1_profile(0), "Main");
        assert_eq!(format_av1_profile(1), "High");
        assert_eq!(format_av1_profile(2), "Professional");
        assert_eq!(format_av1_profile(7), "Unknown");
    }

    // -- Synthetic sequence header tests ---------------------------------

    struct BitWriter {
        buf: Vec<u8>,
        bit_index: u8,
    }
    impl BitWriter {
        fn new() -> Self { Self { buf: Vec::new(), bit_index: 0 } }
        fn write_bit(&mut self, b: bool) {
            if self.bit_index == 0 { self.buf.push(0); }
            if b {
                let last = self.buf.len() - 1;
                self.buf[last] |= 1 << (7 - self.bit_index);
            }
            self.bit_index = (self.bit_index + 1) % 8;
        }
        fn write_bits(&mut self, value: u64, n: u32) {
            for i in 0..n { self.write_bit((value >> (n - 1 - i)) & 1 != 0); }
        }
        fn into_bytes(mut self) -> Vec<u8> {
            while self.bit_index != 0 { self.write_bit(false); }
            self.buf
        }
    }

    /// Build a minimal sequence_header body that matches the reduced-still-
    /// picture branch (sets `reduced_still_picture_header = 1` to skip the
    /// elaborate operating_points loop).
    fn build_reduced_sequence_header(
        seq_profile: u8,
        max_w: u32,
        max_h: u32,
        high_bitdepth: bool,
    ) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_bits(seq_profile as u64, 3);
        w.write_bit(false); // still_picture
        w.write_bit(true);  // reduced_still_picture_header
        w.write_bits(0, 5); // seq_level_idx
        // frame_width_bits_minus_1 + frame_height_bits_minus_1
        // Use 10 bits each → 9 in the field (means 10 bits to encode dims)
        let width_bits: u32 = 12; // pic up to 4095
        let height_bits: u32 = 12;
        w.write_bits((width_bits - 1) as u64, 4);
        w.write_bits((height_bits - 1) as u64, 4);
        w.write_bits((max_w - 1) as u64, width_bits);
        w.write_bits((max_h - 1) as u64, height_bits);
        // No frame_id_numbers because reduced.
        w.write_bit(false); // use_128x128_superblock
        w.write_bit(false); // enable_filter_intra
        w.write_bit(false); // enable_intra_edge_filter
        // (Skipped block fires only when !reduced_still_picture)
        w.write_bit(false); // enable_superres
        w.write_bit(false); // enable_cdef
        w.write_bit(false); // enable_restoration
        // Color config
        w.write_bit(high_bitdepth); // high_bitdepth
        // twelve_bit only when profile=2 && high_bitdepth
        if seq_profile == 2 && high_bitdepth {
            w.write_bit(false); // twelve_bit
        }
        if seq_profile != 1 {
            w.write_bit(false); // monochrome
        }
        w.write_bit(false); // color_description_present
        w.into_bytes()
    }

    #[test]
    fn decode_reduced_sequence_header_yields_width_and_height() {
        let body = build_reduced_sequence_header(0, 1920, 1080, false);
        let seq = decode_sequence_header(&body).unwrap();
        assert_eq!(seq.seq_profile, 0);
        assert_eq!(seq.max_width, 1920);
        assert_eq!(seq.max_height, 1080);
        assert_eq!(seq.bit_depth, 8);
        assert_eq!(seq.subsampling_x, 1);
        assert_eq!(seq.subsampling_y, 1);
    }

    #[test]
    fn decode_reduced_sequence_header_high_bit_depth() {
        let body = build_reduced_sequence_header(0, 3840, 2160, true);
        let seq = decode_sequence_header(&body).unwrap();
        assert_eq!(seq.bit_depth, 10);
        assert_eq!(seq.max_width, 3840);
        assert_eq!(seq.max_height, 2160);
    }

    #[test]
    fn decode_sequence_header_empty_rejected() {
        assert!(decode_sequence_header(&[]).is_err());
    }

    #[test]
    fn av1_stream_with_td_plus_seq_header_round_trips() {
        use crate::media_metadata::deadline::Deadline;
        use std::io::Cursor;
        use crate::media_metadata::reader::Reader;
        // Temporal delimiter OBU = type 2.  Encoded with has_size_field=1
        // and size=0 → 2-byte OBU: header(0x12), size byte (0x00).
        let body = build_reduced_sequence_header(0, 1280, 720, false);
        let mut bytes = vec![0x12u8, 0x00]; // temporal_delimiter
        bytes.push(0x0A); // sequence_header OBU header (type=1, has_size_field=1)
        bytes.push(body.len() as u8);
        bytes.extend_from_slice(&body);

        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.obu", 0);
        ObuReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Av1Obu);
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1280, height: 720 }));
        let cfg = v.codec_config.as_ref().unwrap();
        assert_eq!(cfg.profile_idc, Some(0));
        assert_eq!(cfg.bit_depth_luma, Some(8));
        assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
    }

    #[test]
    fn probe_rejects_first_byte_with_forbidden_bit_set() {
        use std::io::Cursor;
        use crate::media_metadata::reader::Reader;
        let bytes = vec![0x90, 0x00];
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(!ObuReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_short_input() {
        use std::io::Cursor;
        use crate::media_metadata::reader::Reader;
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0x12]));
        assert!(!ObuReader.probe(&mut s).unwrap());
    }
}
