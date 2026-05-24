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

//! IVF container reader — port of `mkvtoolnix/src/input/r_ivf.cpp` and
//! `common/ivf.{h,cpp}`.
//!
//! Layout of the 32-byte fixed header (all multi-byte fields little-endian):
//!
//! ```text
//! offset  size   field
//! 0       4      "DKIF" magic
//! 4       2      version (usually 0)
//! 6       2      header_size (usually 32)
//! 8       4      fourcc (e.g. "AV01", "VP80", "VP90")
//! 12      2      width
//! 14      2      height
//! 16      4      frame_rate_num
//! 20      4      frame_rate_den
//! 24      4      frame_count
//! 28      4      unused
//! ```
//!
//! mkvtoolnix accepts only V_AV1 / V_VP8 / V_VP9 at probe time.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

pub const MAGIC: [u8; 4] = *b"DKIF";
const HEADER_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHeader {
    pub version: u16,
    pub header_size: u16,
    pub fourcc: [u8; 4],
    pub width: u16,
    pub height: u16,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub frame_count: u32,
}

impl FileHeader {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_LEN || bytes[..4] != MAGIC {
            return None;
        }
        Some(Self {
            version: u16::from_le_bytes([bytes[4], bytes[5]]),
            header_size: u16::from_le_bytes([bytes[6], bytes[7]]),
            fourcc: [bytes[8], bytes[9], bytes[10], bytes[11]],
            width: u16::from_le_bytes([bytes[12], bytes[13]]),
            height: u16::from_le_bytes([bytes[14], bytes[15]]),
            frame_rate_num: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            frame_rate_den: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            frame_count: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvfCodec {
    Vp8,
    Vp9,
    Av1,
}

impl IvfCodec {
    pub fn from_fourcc(f: &[u8; 4]) -> Option<Self> {
        match f {
            b"VP80" => Some(Self::Vp8),
            b"VP90" => Some(Self::Vp9),
            b"AV01" => Some(Self::Av1),
            _ => None,
        }
    }

    pub fn codec_id(self) -> &'static str {
        match self {
            Self::Vp8 => "V_VP8",
            Self::Vp9 => "V_VP9",
            Self::Av1 => "V_AV1",
        }
    }

    pub fn codec_name(self) -> &'static str {
        match self {
            Self::Vp8 => "VP8",
            Self::Vp9 => "VP9",
            Self::Av1 => "AV1",
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IvfReader;

impl Reader for IvfReader {
    fn name(&self) -> &'static str {
        "ivf"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = [0u8; HEADER_LEN];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        if read < HEADER_LEN {
            return Ok(false);
        }
        Ok(match FileHeader::parse(&buf) {
            Some(h) => IvfCodec::from_fourcc(&h.fourcc).is_some(),
            None => false,
        })
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut buf = [0u8; HEADER_LEN];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut buf)?;
        if read < HEADER_LEN {
            return Err(ParseError::Unrecognised);
        }
        let header = FileHeader::parse(&buf).ok_or(ParseError::Unrecognised)?;
        let codec = IvfCodec::from_fourcc(&header.fourcc).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Ivf;
        out.container.recognized = true;
        out.container.supported = true;

        let default_duration_ns = if header.frame_rate_num > 0 {
            Some((1_000_000_000u64 * header.frame_rate_den as u64) / header.frame_rate_num as u64)
        } else {
            None
        };

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);

        let video = VideoTrackProperties {
            pixel_dimensions: Some(Dimensions2D {
                width: header.width as u32,
                height: header.height as u32,
            }),
            display_dimensions: Some(Dimensions2D {
                width: header.width as u32,
                height: header.height as u32,
            }),
            default_duration_ns,
            ..VideoTrackProperties::default()
        };

        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: codec.codec_id().to_string(),
                name: Some(codec.codec_name().to_string()),
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
pub(crate) fn build_header(fourcc: &[u8; 4], width: u16, height: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&0u16.to_le_bytes()); // version
    buf.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes()); // header_size
    buf.extend_from_slice(fourcc);
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    buf.extend_from_slice(&30_000u32.to_le_bytes()); // frame_rate_num
    buf.extend_from_slice(&1000u32.to_le_bytes()); // frame_rate_den
    buf.extend_from_slice(&0u32.to_le_bytes()); // frame_count
    buf.extend_from_slice(&0u32.to_le_bytes()); // unused
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_av1_header() {
        let h = FileHeader::parse(&build_header(b"AV01", 1920, 1080)).unwrap();
        assert_eq!(h.fourcc, *b"AV01");
        assert_eq!(h.width, 1920);
        assert_eq!(h.height, 1080);
        assert_eq!(h.frame_rate_num, 30_000);
        assert_eq!(h.frame_rate_den, 1000);
    }

    #[test]
    fn parse_rejects_wrong_magic() {
        let mut bytes = build_header(b"AV01", 1920, 1080);
        bytes[0] = b'X';
        assert!(FileHeader::parse(&bytes).is_none());
    }

    #[test]
    fn parse_rejects_short_input() {
        assert!(FileHeader::parse(&[0u8; 10]).is_none());
    }

    #[test]
    fn codec_from_fourcc_recognises_supported_codecs() {
        assert_eq!(IvfCodec::from_fourcc(b"VP80"), Some(IvfCodec::Vp8));
        assert_eq!(IvfCodec::from_fourcc(b"VP90"), Some(IvfCodec::Vp9));
        assert_eq!(IvfCodec::from_fourcc(b"AV01"), Some(IvfCodec::Av1));
        assert_eq!(IvfCodec::from_fourcc(b"XYZW"), None);
    }

    #[test]
    fn codec_ids_match_matroska_convention() {
        assert_eq!(IvfCodec::Vp8.codec_id(), "V_VP8");
        assert_eq!(IvfCodec::Vp9.codec_id(), "V_VP9");
        assert_eq!(IvfCodec::Av1.codec_id(), "V_AV1");
    }

    #[test]
    fn probe_accepts_av1_blob() {
        let blob = build_header(b"AV01", 1280, 720);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        assert!(IvfReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_unsupported_fourcc() {
        let blob = build_header(b"ZZZZ", 1280, 720);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        assert!(!IvfReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_short_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"DKIF".to_vec()));
        assert!(!IvfReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_vp9_track() {
        let blob = build_header(b"VP90", 1920, 1080);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.ivf", 0);
        IvfReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .unwrap();
        assert_eq!(out.container.format, ContainerFormat::Ivf);
        assert_eq!(out.tracks[0].codec.id, "V_VP9");
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
        // 30000/1000 fps → 1/30 second = 33_333_333 ns
        assert_eq!(v.default_duration_ns, Some(33_333_333));
    }

    #[test]
    fn read_headers_rejects_unsupported_fourcc() {
        let blob = build_header(b"ZZZZ", 1280, 720);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.ivf", 0);
        let err = IvfReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }
}
