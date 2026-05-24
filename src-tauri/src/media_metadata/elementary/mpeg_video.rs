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

//! MPEG-1 / MPEG-2 video elementary stream reader.
//!
//! Sequence header (ISO/IEC 11172-2 §2.4.2.3 + 13818-2 §6.2.2.1):
//!
//! ```text
//! 0x00 0x00 0x01 0xB3                 (sequence_header_code)
//! 12 bits horizontal_size
//! 12 bits vertical_size
//! 4  bits aspect_ratio
//! 4  bits frame_rate_code
//! 18 bits bit_rate
//! 1  bit  marker
//! 10 bits vbv_buffer_size
//! 1  bit  constrained
//! 1  bit  load_intra_quantiser_matrix
//! [64 bytes intra matrix if flag set]
//! 1  bit  load_non_intra_quantiser_matrix
//! [64 bytes non-intra matrix if flag set]
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 16 * 1024;
const SEQUENCE_HEADER_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0xB3];

const FRAME_RATE_TABLE: [(u32, u32); 16] = [
    (0, 1),
    (24_000, 1001),
    (24, 1),
    (25, 1),
    (30_000, 1001),
    (30, 1),
    (50, 1),
    (60_000, 1001),
    (60, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
    (0, 1),
];

#[derive(Debug, Clone, Copy)]
pub struct SequenceHeader {
    pub horizontal_size: u32,
    pub vertical_size: u32,
    pub aspect_ratio_code: u8,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
}

pub fn find_sequence_header(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == SEQUENCE_HEADER_CODE)
}

pub fn decode_sequence_header(bytes: &[u8]) -> Option<SequenceHeader> {
    let pos = find_sequence_header(bytes)?;
    let body = bytes.get(pos + 4..pos + 4 + 8)?;
    // 12 + 12 + 4 + 4 = 32 bits = first 4 bytes
    let horizontal_size = ((body[0] as u32) << 4) | ((body[1] as u32) >> 4);
    let vertical_size = (((body[1] as u32) & 0x0F) << 8) | body[2] as u32;
    let aspect_ratio_code = (body[3] >> 4) & 0x0F;
    let frame_rate_code = (body[3] & 0x0F) as usize;
    let (num, den) = FRAME_RATE_TABLE[frame_rate_code];
    Some(SequenceHeader {
        horizontal_size,
        vertical_size,
        aspect_ratio_code,
        frame_rate_num: num,
        frame_rate_den: den,
    })
}

pub fn frame_duration_ns(num: u32, den: u32) -> Option<u64> {
    if num == 0 {
        return None;
    }
    Some((den as u128 * 1_000_000_000 / num as u128) as u64)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegVideoReader;

impl Reader for MpegVideoReader {
    fn name(&self) -> &'static str {
        "mpeg_video"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        // Sequence header must sit at the very start to count — mkvtoolnix
        // requires this strict positioning for the elementary-stream cascade.
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

        out.container.format = ContainerFormat::MpegVideo;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let video = VideoTrackProperties {
            pixel_dimensions: Some(Dimensions2D {
                width: header.horizontal_size,
                height: header.vertical_size,
            }),
            display_dimensions: Some(Dimensions2D {
                width: header.horizontal_size,
                height: header.vertical_size,
            }),
            default_duration_ns: frame_duration_ns(header.frame_rate_num, header.frame_rate_den),
            ..VideoTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: "V_MPEG2".to_string(),
                name: Some("MPEG-1/2 Video".to_string()),
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
pub(crate) fn build_sequence_header(width: u32, height: u32, frame_rate_code: u8) -> Vec<u8> {
    let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
    bytes.push(((width >> 4) & 0xFF) as u8);
    bytes.push((((width & 0x0F) << 4) | ((height >> 8) & 0x0F)) as u8);
    bytes.push((height & 0xFF) as u8);
    bytes.push((1u8 << 4) | (frame_rate_code & 0x0F));
    bytes.extend_from_slice(&[0u8; 4]); // bitrate + markers
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_1920x1080_30fps_sequence_header() {
        let bytes = build_sequence_header(1920, 1080, 5);
        let h = decode_sequence_header(&bytes).unwrap();
        assert_eq!(h.horizontal_size, 1920);
        assert_eq!(h.vertical_size, 1080);
        assert_eq!(h.frame_rate_num, 30);
        assert_eq!(h.frame_rate_den, 1);
    }

    #[test]
    fn decodes_23976_fps() {
        let bytes = build_sequence_header(1920, 1080, 1);
        let h = decode_sequence_header(&bytes).unwrap();
        assert_eq!(h.frame_rate_num, 24_000);
        assert_eq!(h.frame_rate_den, 1001);
    }

    #[test]
    fn find_sequence_header_skips_garbage() {
        let mut bytes = vec![0xAAu8; 16];
        bytes.extend(build_sequence_header(640, 480, 3));
        assert_eq!(find_sequence_header(&bytes), Some(16));
    }

    #[test]
    fn find_sequence_header_returns_none() {
        assert!(find_sequence_header(&[0xAAu8; 16]).is_none());
    }

    #[test]
    fn probe_requires_header_at_offset_zero() {
        let bytes = build_sequence_header(640, 480, 3);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(MpegVideoReader.probe(&mut s).unwrap());

        let mut prefixed = vec![0xAAu8; 4];
        prefixed.extend(build_sequence_header(640, 480, 3));
        let mut s = FileSource::from_reader_for_test(Cursor::new(prefixed));
        assert!(!MpegVideoReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_video_track() {
        let bytes = build_sequence_header(1280, 720, 5);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpv", 0);
        MpegVideoReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::MpegVideo);
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1280, height: 720 }));
    }

    #[test]
    fn frame_duration_ns_for_known_rates() {
        assert_eq!(frame_duration_ns(30, 1), Some(33_333_333));
        assert_eq!(frame_duration_ns(24_000, 1001), Some(41_708_333));
        assert!(frame_duration_ns(0, 1).is_none());
    }
}
