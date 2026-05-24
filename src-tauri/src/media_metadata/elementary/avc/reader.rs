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

//! Top-level `AvcReader` — recognises raw AVC/H.264 elementary streams.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{
    ChromaFormat, Dimensions2D, VideoCodecConfig, VideoTrackProperties,
};
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::nal::{self, NAL_UNIT_TYPE_SPS};
use super::sps;

const PROBE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct AvcReader;

impl Reader for AvcReader {
    fn name(&self) -> &'static str {
        "avc"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        if read < 5 {
            return Ok(false);
        }
        let units = nal::split_nal_units(&buf[..read]);
        Ok(units.iter().any(|u| u.nal_unit_type == NAL_UNIT_TYPE_SPS))
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
        let units = nal::split_nal_units(&buf[..read]);
        let sps_unit = units
            .iter()
            .find(|u| u.nal_unit_type == NAL_UNIT_TYPE_SPS)
            .ok_or(ParseError::Unrecognised)?;
        let rbsp = nal::strip_emulation_prevention(sps_unit.payload);
        let sps = sps::parse(&rbsp)?;

        out.container.format = ContainerFormat::Avc;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let video = VideoTrackProperties {
            pixel_dimensions: Some(Dimensions2D {
                width: sps.display_width,
                height: sps.display_height,
            }),
            display_dimensions: Some(Dimensions2D {
                width: sps.display_width,
                height: sps.display_height,
            }),
            codec_config: Some(VideoCodecConfig {
                profile_idc: Some(sps.profile_idc as u32),
                profile_name: Some(sps::format_profile(sps.profile_idc).to_string()),
                level_idc: Some(sps.level_idc as u32),
                level_name: Some(sps::format_level(sps.level_idc)),
                chroma_format: Some(map_chroma(sps.chroma_format_idc)),
                bit_depth_luma: Some(sps.bit_depth_luma as u32),
                bit_depth_chroma: Some(sps.bit_depth_chroma as u32),
                coded_dimensions: Some(Dimensions2D {
                    width: sps.coded_width,
                    height: sps.coded_height,
                }),
                is_elementary_stream: Some(true),
                ..VideoCodecConfig::default()
            }),
            ..VideoTrackProperties::default()
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: "V_MPEG4/ISO/AVC".to_string(),
                name: Some("AVC/H.264".to_string()),
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

fn map_chroma(idc: u8) -> ChromaFormat {
    match idc {
        0 => ChromaFormat::Monochrome,
        1 => ChromaFormat::Yuv420,
        2 => ChromaFormat::Yuv422,
        3 => ChromaFormat::Yuv444,
        _ => ChromaFormat::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a tiny Annex B byte stream with a baseline 1920x1080 SPS so the
    /// reader produces a complete track.
    fn build_avc_with_baseline_1080p_sps() -> Vec<u8> {
        let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67]; // SPS NAL
        bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
        // Reuse the test BitWriter from sps.rs by reconstructing the bits here.
        bytes.extend(build_baseline_1080p_tail());
        // Append an AUD NAL so the SPS NAL has a definite end.
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]);
        bytes
    }

    fn build_baseline_1080p_tail() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write_ue(0); // seq_parameter_set_id
        w.write_ue(0); // log2_max_frame_num_minus4
        w.write_ue(0); // pic_order_cnt_type
        w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
        w.write_ue(0); // num_ref_frames
        w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
        w.write_ue(119); // pic_width_in_mbs_minus1 = 1920/16-1
        w.write_ue(67);  // pic_height_in_map_units_minus1 = 1080/16-1 → coded 1088
        w.write_bit(true); // frame_mbs_only_flag
        w.write_bit(false); // direct_8x8_inference_flag
        w.write_bit(true); // frame_cropping_flag
        w.write_ue(0); // crop_left
        w.write_ue(0); // crop_right
        w.write_ue(0); // crop_top
        w.write_ue(4); // crop_bottom → 1088 - 4*2 = 1080
        w.into_bytes()
    }

    struct BitWriter {
        buf: Vec<u8>,
        bit_index: u8,
    }
    impl BitWriter {
        fn new() -> Self {
            Self { buf: Vec::new(), bit_index: 0 }
        }
        fn write_bit(&mut self, b: bool) {
            if self.bit_index == 0 {
                self.buf.push(0);
            }
            if b {
                let last = self.buf.len() - 1;
                self.buf[last] |= 1 << (7 - self.bit_index);
            }
            self.bit_index = (self.bit_index + 1) % 8;
        }
        fn write_bits(&mut self, value: u64, n: u32) {
            for i in 0..n {
                self.write_bit((value >> (n - 1 - i)) & 1 != 0);
            }
        }
        fn write_ue(&mut self, value: u32) {
            let codeword = value as u64 + 1;
            let nb = 64 - codeword.leading_zeros();
            for _ in 0..(nb - 1) {
                self.write_bit(false);
            }
            self.write_bits(codeword, nb);
        }
        fn into_bytes(mut self) -> Vec<u8> {
            self.write_bit(true);
            while self.bit_index != 0 {
                self.write_bit(false);
            }
            self.buf
        }
    }

    #[test]
    fn probe_accepts_annex_b_with_sps() {
        let bytes = build_avc_with_baseline_1080p_sps();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(AvcReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
        assert!(!AvcReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_dimensions_and_codec_config() {
        let bytes = build_avc_with_baseline_1080p_sps();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.h264", 0);
        AvcReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Avc);
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
        let cfg = v.codec_config.as_ref().unwrap();
        assert_eq!(cfg.profile_idc, Some(66));
        assert_eq!(cfg.level_name.as_deref(), Some("4.0"));
        assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
        assert_eq!(cfg.bit_depth_luma, Some(8));
        assert_eq!(cfg.is_elementary_stream, Some(true));
    }

    #[test]
    fn read_headers_returns_unrecognised_without_sps() {
        let bytes = vec![0x00, 0x00, 0x00, 0x01, 0x06]; // SEI NAL only
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.h264", 0);
        let err = AvcReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn map_chroma_table() {
        assert_eq!(map_chroma(0), ChromaFormat::Monochrome);
        assert_eq!(map_chroma(1), ChromaFormat::Yuv420);
        assert_eq!(map_chroma(2), ChromaFormat::Yuv422);
        assert_eq!(map_chroma(3), ChromaFormat::Yuv444);
        assert_eq!(map_chroma(7), ChromaFormat::Other);
    }
}
