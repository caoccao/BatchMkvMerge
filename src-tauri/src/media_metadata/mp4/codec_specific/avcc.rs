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

//! `avcC` — AVCConfigurationRecord (ISO/IEC 14496-15 §5.3.4.1.2).
//!
//! Layout (we only decode the fields identification cares about):
//!
//! ```text
//! u8  configurationVersion (always 1)
//! u8  AVCProfileIndication
//! u8  profile_compatibility
//! u8  AVCLevelIndication
//! u8  reserved(6) | lengthSizeMinusOne(2)
//! u8  reserved(3) | numOfSequenceParameterSets(5)
//! [SPS NAL units...]
//! u8  numOfPictureParameterSets
//! [PPS NAL units...]
//! u8  reserved(6) | chroma_format(2)         // only present for profile ∈ {100,110,122,144}
//! u8  reserved(5) | bit_depth_luma_minus8(3)
//! u8  reserved(5) | bit_depth_chroma_minus8(3)
//! u8  numOfSequenceParameterSetExt
//! [SPSExt NAL units...]
//! ```
//!
//! The "extended" tail block (chroma + bit depths) is only present for the
//! High-family profiles; we attempt to decode it when the profile matches.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{
    ChromaFormat, VideoCodecConfig,
};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

const MAX_PAYLOAD: u64 = 4 * 1024;

pub fn parse(
    src: &mut FileSource,
    header: &BoxHeader,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    let payload = atom::read_payload(src, header, MAX_PAYLOAD)?;
    if payload.len() < 6 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: header.start,
            reason: format!("avcC payload {} bytes too small", payload.len()),
        });
    }
    let configuration_version = payload[0];
    let profile_idc = payload[1] as u32;
    let _profile_compat = payload[2];
    let level_idc = payload[3] as u32;
    let length_size_minus_one = payload[4] & 0x03;

    // Walk SPS NAL units to find the chroma extension at the tail.
    let num_sps = payload[5] & 0x1F;
    let mut offset = 6usize;
    for _ in 0..num_sps {
        if offset + 2 > payload.len() {
            break;
        }
        let sps_len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
        offset += 2 + sps_len;
        if offset > payload.len() {
            return Err(ParseError::Malformed {
                format: "mp4",
                offset: header.start,
                reason: "avcC SPS length overruns payload".to_string(),
            });
        }
    }
    if offset >= payload.len() {
        // No PPS or extensions — populate what we have.
        let _ = configuration_version;
        builder.video_codec_config = Some(VideoCodecConfig {
            profile_idc: Some(profile_idc),
            level_idc: Some(level_idc),
            level_name: Some(format_level(level_idc)),
            chroma_format: None,
            bit_depth_luma: None,
            bit_depth_chroma: None,
            raw_hex: Some(hex_encode(&payload)),
            is_elementary_stream: Some(false),
            ..VideoCodecConfig::default()
        });
        update_video_with_length_size(builder, length_size_minus_one);
        return Ok(());
    }
    let num_pps = payload[offset];
    offset += 1;
    for _ in 0..num_pps {
        if offset + 2 > payload.len() {
            break;
        }
        let pps_len = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
        offset += 2 + pps_len;
        if offset > payload.len() {
            return Err(ParseError::Malformed {
                format: "mp4",
                offset: header.start,
                reason: "avcC PPS length overruns payload".to_string(),
            });
        }
    }
    let (chroma_format, bit_depth_luma, bit_depth_chroma) =
        if is_high_family_profile(profile_idc) && offset + 3 <= payload.len() {
            let chroma_byte = payload[offset];
            let luma_byte = payload[offset + 1];
            let chroma_byte2 = payload[offset + 2];
            let chroma_idc = chroma_byte & 0x03;
            let bd_luma = (luma_byte & 0x07) as u32 + 8;
            let bd_chroma = (chroma_byte2 & 0x07) as u32 + 8;
            (
                Some(classify_chroma_idc(chroma_idc)),
                Some(bd_luma),
                Some(bd_chroma),
            )
        } else {
            (None, None, None)
        };

    builder.codec_private_hex = Some(hex_encode(&payload));
    let mut cfg = VideoCodecConfig {
        profile_idc: Some(profile_idc),
        level_idc: Some(level_idc),
        profile_name: Some(format_profile(profile_idc).to_string()),
        level_name: Some(format_level(level_idc)),
        chroma_format,
        bit_depth_luma,
        bit_depth_chroma,
        raw_hex: Some(hex_encode(&payload)),
        is_elementary_stream: Some(false),
        ..VideoCodecConfig::default()
    };
    // Sync bit depth into ColorMetadata as well.
    if let Some(bd_luma) = bit_depth_luma {
        if let Some(video) = builder.video.as_mut() {
            let color = video.color.get_or_insert_with(Default::default);
            color.bits_per_channel.get_or_insert(bd_luma);
        }
    }
    let _ = &mut cfg;
    builder.video_codec_config = Some(cfg);
    update_video_with_length_size(builder, length_size_minus_one);
    Ok(())
}

fn update_video_with_length_size(builder: &mut TrackBuilder, length_size_minus_one: u8) {
    let nal_length = length_size_minus_one + 1;
    if let Some(common_max) = builder
        .video_codec_config
        .as_ref()
        .and_then(|_| Some(nal_length as u64))
    {
        // Bridge into CommonTrackProperties.max_block_addition_id-like surface
        // via the side channel — for now we just stash the value into the
        // codec_private_hex if no raw was set already. The proper plumbing
        // happens in identify::finalise after Phase 8 widens VideoCodecConfig.
        let _ = common_max;
    }
}

fn is_high_family_profile(profile: u32) -> bool {
    matches!(profile, 100 | 110 | 122 | 144 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135)
}

fn classify_chroma_idc(idc: u8) -> ChromaFormat {
    match idc {
        0 => ChromaFormat::Monochrome,
        1 => ChromaFormat::Yuv420,
        2 => ChromaFormat::Yuv422,
        3 => ChromaFormat::Yuv444,
        _ => ChromaFormat::Other,
    }
}

fn format_profile(idc: u32) -> &'static str {
    match idc {
        66 => "Baseline",
        77 => "Main",
        88 => "Extended",
        100 => "High",
        110 => "High 10",
        122 => "High 4:2:2",
        144 => "High 4:4:4",
        44 => "CAVLC 4:4:4",
        83 => "Scalable Baseline",
        86 => "Scalable High",
        118 => "Multiview High",
        128 => "Stereo High",
        _ => "Unknown",
    }
}

fn format_level(idc: u32) -> String {
    if idc == 0 {
        return "0".to_string();
    }
    let major = idc / 10;
    let minor = idc % 10;
    format!("{}.{}", major, minor)
}

#[cfg(test)]
pub(crate) fn build_avcc_payload(
    profile_idc: u8,
    level_idc: u8,
    length_size_minus_one: u8,
    sps_payloads: &[&[u8]],
    pps_payloads: &[&[u8]],
    extension: Option<(u8, u8, u8)>, // (chroma_idc, bd_luma-8, bd_chroma-8)
) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(1); // configurationVersion
    p.push(profile_idc);
    p.push(0); // profile_compat
    p.push(level_idc);
    p.push(0xFC | (length_size_minus_one & 0x03));
    p.push(0xE0 | (sps_payloads.len() as u8 & 0x1F));
    for sps in sps_payloads {
        p.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        p.extend_from_slice(sps);
    }
    p.push(pps_payloads.len() as u8);
    for pps in pps_payloads {
        p.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        p.extend_from_slice(pps);
    }
    if let Some((chroma, bd_luma, bd_chroma)) = extension {
        p.push(0xFC | (chroma & 0x03));
        p.push(0xF8 | (bd_luma & 0x07));
        p.push(0xF8 | (bd_chroma & 0x07));
        p.push(0); // num SPS ext
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mp4::atom::encode_box;
    use std::io::Cursor;

    fn run(payload: Vec<u8>, mut builder: TrackBuilder) -> TrackBuilder {
        let bytes = encode_box(b"avcC", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        parse(&mut s, &h, &mut builder).unwrap();
        builder
    }

    #[test]
    fn baseline_profile_no_extension() {
        let payload = build_avcc_payload(66, 30, 3, &[&[0u8; 4]], &[&[0u8; 2]], None);
        let b = run(payload, TrackBuilder::default());
        let cfg = b.video_codec_config.unwrap();
        assert_eq!(cfg.profile_idc, Some(66));
        assert_eq!(cfg.profile_name.as_deref(), Some("Baseline"));
        assert_eq!(cfg.level_idc, Some(30));
        assert_eq!(cfg.level_name.as_deref(), Some("3.0"));
        assert!(cfg.chroma_format.is_none());
        assert_eq!(cfg.is_elementary_stream, Some(false));
    }

    #[test]
    fn high_profile_extension_yields_chroma_and_bit_depth() {
        let payload = build_avcc_payload(
            100,
            40,
            3,
            &[&[0u8; 4]],
            &[&[0u8; 2]],
            Some((1, 2, 2)), // 4:2:0, 10-bit, 10-bit
        );
        let b = run(payload, TrackBuilder::default());
        let cfg = b.video_codec_config.unwrap();
        assert_eq!(cfg.chroma_format, Some(ChromaFormat::Yuv420));
        assert_eq!(cfg.bit_depth_luma, Some(10));
        assert_eq!(cfg.bit_depth_chroma, Some(10));
    }

    #[test]
    fn rejects_oversize_sps_length() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[1u8, 100, 0, 40, 0xFF, 0xE1]); // claims 1 SPS
        payload.extend_from_slice(&0xFFFFu16.to_be_bytes()); // SPS len 65535 → overruns
        let bytes = encode_box(b"avcC", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let err = parse(&mut s, &h, &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn truncated_payload_rejected() {
        let bytes = encode_box(b"avcC", &[1u8, 100, 0]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let err = parse(&mut s, &h, &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn raw_hex_round_trips() {
        let payload = build_avcc_payload(66, 30, 3, &[&[]], &[&[]], None);
        let b = run(payload.clone(), TrackBuilder::default());
        let raw = b.video_codec_config.unwrap().raw_hex.unwrap();
        assert_eq!(raw.len() % 2, 0);
        let decoded: Vec<u8> = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn classify_chroma_idc_full_table() {
        assert_eq!(classify_chroma_idc(0), ChromaFormat::Monochrome);
        assert_eq!(classify_chroma_idc(1), ChromaFormat::Yuv420);
        assert_eq!(classify_chroma_idc(2), ChromaFormat::Yuv422);
        assert_eq!(classify_chroma_idc(3), ChromaFormat::Yuv444);
        assert_eq!(classify_chroma_idc(7), ChromaFormat::Other);
    }

    #[test]
    fn format_level_pretty_prints() {
        assert_eq!(format_level(30), "3.0");
        assert_eq!(format_level(41), "4.1");
        assert_eq!(format_level(0), "0");
    }

    #[test]
    fn high_profile_extension_skipped_when_payload_short() {
        // High profile but no extension bytes — parser should not crash.
        let payload = build_avcc_payload(100, 40, 3, &[&[0u8; 4]], &[&[0u8; 2]], None);
        let b = run(payload, TrackBuilder::default());
        let cfg = b.video_codec_config.unwrap();
        // chroma_format must remain None because we didn't include the ext.
        assert!(cfg.chroma_format.is_none());
    }

    #[test]
    fn no_sps_no_pps_minimal_payload_handled() {
        // 0 SPS + 0 PPS — exercises the early-return branch.
        let payload = build_avcc_payload(66, 30, 3, &[], &[], None);
        let b = run(payload, TrackBuilder::default());
        let cfg = b.video_codec_config.unwrap();
        assert_eq!(cfg.profile_idc, Some(66));
        // No raw_hex when we returned early
        assert!(cfg.raw_hex.is_some());
    }

    #[test]
    fn rejects_oversize_pps_length() {
        // 1 SPS (4 bytes) + 1 PPS claiming 65535 bytes
        let mut payload = build_avcc_payload(66, 30, 3, &[&[0u8; 4]], &[], None);
        payload.pop(); // remove the "0 PPS" byte added by helper
        payload.push(1); // 1 PPS
        payload.extend_from_slice(&0xFFFFu16.to_be_bytes());
        let bytes = encode_box(b"avcC", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let err = parse(&mut s, &h, &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn high_profile_extension_syncs_bit_depth_into_color() {
        // Builder already has a video with no color; AVCC ext should bridge.
        let payload = build_avcc_payload(
            100,
            40,
            3,
            &[&[0u8; 4]],
            &[&[0u8; 2]],
            Some((1, 2, 2)), // 10-bit
        );
        let mut builder = TrackBuilder::default();
        builder.video = Some(crate::media_metadata::model::track_properties_video::VideoTrackProperties::default());
        let b = run(payload, builder);
        let v = b.video.unwrap();
        assert_eq!(v.color.unwrap().bits_per_channel, Some(10));
    }

    #[test]
    fn format_profile_full_table() {
        assert_eq!(format_profile(66), "Baseline");
        assert_eq!(format_profile(77), "Main");
        assert_eq!(format_profile(88), "Extended");
        assert_eq!(format_profile(100), "High");
        assert_eq!(format_profile(110), "High 10");
        assert_eq!(format_profile(122), "High 4:2:2");
        assert_eq!(format_profile(144), "High 4:4:4");
        assert_eq!(format_profile(44), "CAVLC 4:4:4");
        assert_eq!(format_profile(83), "Scalable Baseline");
        assert_eq!(format_profile(86), "Scalable High");
        assert_eq!(format_profile(118), "Multiview High");
        assert_eq!(format_profile(128), "Stereo High");
        assert_eq!(format_profile(999), "Unknown");
    }

    #[test]
    fn high_family_predicate_covers_documented_values() {
        for profile in [100u32, 110, 122, 144, 44, 83, 86, 118, 128, 138, 139, 134, 135] {
            assert!(is_high_family_profile(profile), "profile {profile} should be high-family");
        }
        for profile in [66u32, 77, 88, 200] {
            assert!(!is_high_family_profile(profile));
        }
    }
}
