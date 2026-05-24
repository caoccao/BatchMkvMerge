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

//! VC-1 (SMPTE 421M) elementary stream reader.
//!
//! Advanced-profile sequence-layer start code: `0x00 0x00 0x01 0x0F`.
//! After the start code:
//!
//! ```text
//! 2 bits profile     (3 = Advanced)
//! 3 bits level
//! 2 bits colordiff_format
//! 3 bits frmrtq_postproc
//! 5 bits bitrtq_postproc
//! 1 bit  postprocflag
//! 12 bits max_coded_width (in macroblocks - 1)
//! 12 bits max_coded_height (in macroblocks - 1)
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 16 * 1024;
const SEQUENCE_HEADER_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0x0F];

#[derive(Debug, Clone, Copy)]
pub struct SequenceHeader {
    pub profile: u8,
    pub level: u8,
    pub max_coded_width: u32,
    pub max_coded_height: u32,
}

pub fn decode_sequence_header(bytes: &[u8]) -> Option<SequenceHeader> {
    let pos = bytes
        .windows(4)
        .position(|w| w == SEQUENCE_HEADER_CODE)?;
    // 40 bits needed = 5 bytes after the start code.
    let body_end = (pos + 4 + 5).min(bytes.len());
    if body_end < pos + 4 + 5 {
        return None;
    }
    let body = &bytes[pos + 4..body_end];
    let mut reader = BitReader::from_rbsp(body);
    let profile = reader.read_bits(2).ok()? as u8;
    let level = reader.read_bits(3).ok()? as u8;
    let _colordiff = reader.read_bits(2).ok()?;
    let _frmrtq = reader.read_bits(3).ok()?;
    let _bitrtq = reader.read_bits(5).ok()?;
    let _postproc = reader.read_bit().ok()?;
    let max_w_mb = reader.read_bits(12).ok()? as u32;
    let max_h_mb = reader.read_bits(12).ok()? as u32;
    Some(SequenceHeader {
        profile,
        level,
        max_coded_width: (max_w_mb + 1) * 2,
        max_coded_height: (max_h_mb + 1) * 2,
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Vc1Reader;

impl Reader for Vc1Reader {
    fn name(&self) -> &'static str {
        "vc1"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        Ok(read >= 4 && buf[..4] == SEQUENCE_HEADER_CODE)
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
        let header = decode_sequence_header(&buf[..read]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Vc1;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let video = VideoTrackProperties {
            pixel_dimensions: Some(Dimensions2D {
                width: header.max_coded_width,
                height: header.max_coded_height,
            }),
            display_dimensions: Some(Dimensions2D {
                width: header.max_coded_width,
                height: header.max_coded_height,
            }),
            ..VideoTrackProperties::default()
        };
        let _ = (header.profile, header.level);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: "V_VC1".to_string(),
                name: Some("VC-1".to_string()),
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

#[cfg(test)]
pub(crate) fn build_sequence_header(max_coded_width: u32, max_coded_height: u32) -> Vec<u8> {
    // Hand-pack the bits: profile=3, level=4, colordiff=1, frmrtq=0,
    // bitrtq=0, postproc=0, max_w_mb = (w/2)-1, max_h_mb = (h/2)-1.
    let mut w = BitWriter::new();
    w.write_bits(3, 2);  // profile = Advanced
    w.write_bits(4, 3);  // level = 4
    w.write_bits(1, 2);  // colordiff_format
    w.write_bits(0, 3);  // frmrtq
    w.write_bits(0, 5);  // bitrtq
    w.write_bit(false);  // postproc
    w.write_bits(((max_coded_width / 2) - 1) as u64, 12);
    w.write_bits(((max_coded_height / 2) - 1) as u64, 12);
    let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
    bytes.extend(w.into_bytes());
    bytes
}

#[cfg(test)]
struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
}

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_advanced_profile_1080p() {
        let bytes = build_sequence_header(1920, 1080);
        let h = decode_sequence_header(&bytes).unwrap();
        assert_eq!(h.profile, 3);
        assert_eq!(h.level, 4);
        assert_eq!(h.max_coded_width, 1920);
        assert_eq!(h.max_coded_height, 1080);
    }

    #[test]
    fn probe_requires_start_code_at_offset_zero() {
        let bytes = build_sequence_header(1920, 1080);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(Vc1Reader.probe(&mut s).unwrap());

        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAA; 16]));
        assert!(!Vc1Reader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_vc1_track() {
        let bytes = build_sequence_header(1280, 720);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.vc1", 0);
        Vc1Reader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Vc1);
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1280, height: 720 }));
    }
}
