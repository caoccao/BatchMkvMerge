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

//! Per-PID stream registry built from PMT entries + descriptors.

use crate::media_metadata::codec::mpegts_stream_types;
use crate::media_metadata::codec::TrackKind;

use super::descriptors::DescriptorSummary;
use super::pmt::PmtStreamEntry;

#[derive(Debug, Clone)]
pub struct StreamRow {
    pub pid: u16,
    pub stream_type: u8,
    pub program_number: u16,
    pub language: Option<String>,
    pub teletext_page: Option<u32>,
    pub service_name: Option<String>,
    pub codec_id: String,
    pub codec_name: String,
    pub track_kind: TrackKind,
}

/// Canonical Matroska-style codec id string for known MPEG-TS stream types.
/// Mirrors mkvtoolnix's mapping in `r_mpeg_ts.cpp::create_packetizer`.
fn canonical_codec_id(stream_type: u8) -> Option<&'static str> {
    Some(match stream_type {
        0x01 => "V_MPEG1",
        0x02 => "V_MPEG2",
        0x03 | 0x04 => "A_MPEG/L3",
        0x06 => "S_TX", // generic private; usually overridden by descriptors
        0x0F | 0x11 => "A_AAC",
        0x10 => "V_MPEG4/ISO/ASP",
        0x1B => "V_MPEG4/ISO/AVC",
        0x20 => "V_MPEG4/ISO/AVC",
        0x21 => "V_MPEG4/ISO/AVC",
        0x24 => "V_MPEGH/ISO/HEVC",
        // BD-style PMT stream types (ATSC / Blu-ray)
        0x80 => "A_PCM",
        0x81 => "A_AC3",
        0x82 | 0x85 | 0x88 => "A_DTS",
        0x83 => "A_TRUEHD",
        0x84 | 0x87 => "A_EAC3",
        0x86 => "A_DTS",
        0xA1 | 0xA2 => "A_AC3",
        0xA0 => "V_VC1",
        0x90 => "S_HDMV/PGS",
        0x92 => "S_HDMV/TEXTST",
        _ => return None,
    })
}

/// Build one row per `PmtStreamEntry`, applying descriptor overrides where
/// applicable (e.g. an AC-3 descriptor on a private-data stream_type
/// promotes it to the AC-3 codec).
pub fn build_row(
    pid: u16,
    program_number: u16,
    entry: &PmtStreamEntry,
    program_descriptors: &DescriptorSummary,
) -> StreamRow {
    let stream_desc = super::descriptors::walk(&entry.descriptors);
    let language = stream_desc
        .language_iso_639_2
        .clone()
        .or_else(|| program_descriptors.language_iso_639_2.clone());
    let teletext_page = stream_desc.teletext_page;
    let service_name = program_descriptors.service_name.clone();

    let from_table = mpegts_stream_types::lookup(entry.stream_type);
    let mut codec_id = canonical_codec_id(entry.stream_type)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("0x{:02X}", entry.stream_type));
    let mut codec_name = from_table
        .map(|e| e.name.to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let mut kind = from_table.map(|e| e.kind).unwrap_or(TrackKind::Unknown);

    // Descriptor-driven overrides — handle the common "stream_type = 0x06
    // private data + a descriptor that disambiguates the codec" pattern.
    if entry.stream_type == 0x06 {
        if stream_desc.is_ac3 {
            codec_id = "A_AC3".to_string();
            codec_name = "AC-3".to_string();
            kind = TrackKind::Audio;
        } else if stream_desc.is_eac3 {
            codec_id = "A_EAC3".to_string();
            codec_name = "E-AC-3".to_string();
            kind = TrackKind::Audio;
        } else if stream_desc.is_dts {
            codec_id = "A_DTS".to_string();
            codec_name = "DTS".to_string();
            kind = TrackKind::Audio;
        } else if teletext_page.is_some() {
            codec_id = "S_TELETEXT".to_string();
            codec_name = "DVB Teletext".to_string();
            kind = TrackKind::Subtitle;
        }
    }
    // HEVC descriptor disambiguates 0x24 → HEVC.
    if stream_desc.is_hevc && kind == TrackKind::Unknown {
        codec_id = "V_HEVC".to_string();
        codec_name = "HEVC/H.265".to_string();
        kind = TrackKind::Video;
    }

    StreamRow {
        pid,
        stream_type: entry.stream_type,
        program_number,
        language,
        teletext_page,
        service_name,
        codec_id,
        codec_name,
        track_kind: kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mpeg_ts::descriptors::{
        build_descriptor, walk, TAG_AC3, TAG_DTS, TAG_EAC3, TAG_HEVC, TAG_ISO_639_LANGUAGE, TAG_TELETEXT,
    };

    fn entry(stream_type: u8, descriptors: Vec<u8>) -> PmtStreamEntry {
        PmtStreamEntry {
            stream_type,
            elementary_pid: 0x1234,
            descriptors,
        }
    }

    #[test]
    fn known_stream_type_resolved_via_catalogue() {
        let e = entry(0x1B, vec![]); // H.264
        let row = build_row(0x1234, 1, &e, &DescriptorSummary::default());
        assert_eq!(row.codec_id, "V_MPEG4/ISO/AVC");
        assert_eq!(row.track_kind, TrackKind::Video);
    }

    #[test]
    fn iso_639_descriptor_populates_language() {
        let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"fra\x00");
        let row = build_row(
            0x1234,
            1,
            &entry(0x0F, descs),
            &DescriptorSummary::default(),
        );
        assert_eq!(row.language.as_deref(), Some("fra"));
    }

    #[test]
    fn ac3_descriptor_on_private_data_promotes_to_ac3() {
        let descs = build_descriptor(TAG_AC3, &[]);
        let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
        assert_eq!(row.codec_id, "A_AC3");
        assert_eq!(row.track_kind, TrackKind::Audio);
    }

    #[test]
    fn eac3_descriptor_on_private_data_promotes_to_eac3() {
        let descs = build_descriptor(TAG_EAC3, &[]);
        let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
        assert_eq!(row.codec_id, "A_EAC3");
    }

    #[test]
    fn dts_descriptor_on_private_data_promotes_to_dts() {
        let descs = build_descriptor(TAG_DTS, &[]);
        let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
        assert_eq!(row.codec_id, "A_DTS");
    }

    #[test]
    fn teletext_descriptor_on_private_data_promotes_to_subtitle() {
        let descs = build_descriptor(TAG_TELETEXT, &[b'e', b'n', b'g', 0x08, 0x88]);
        let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
        assert_eq!(row.codec_id, "S_TELETEXT");
        assert_eq!(row.track_kind, TrackKind::Subtitle);
        assert_eq!(row.teletext_page, Some(888));
    }

    #[test]
    fn hevc_descriptor_promotes_unknown_to_hevc() {
        let descs = build_descriptor(TAG_HEVC, &[]);
        let row = build_row(0x1234, 1, &entry(0xFA, descs), &DescriptorSummary::default());
        assert_eq!(row.codec_id, "V_HEVC");
        assert_eq!(row.track_kind, TrackKind::Video);
    }

    #[test]
    fn program_level_language_used_when_stream_lacks_one() {
        let mut prog = DescriptorSummary::default();
        prog.language_iso_639_2 = Some("jpn".to_string());
        let row = build_row(0x1234, 1, &entry(0x0F, vec![]), &prog);
        assert_eq!(row.language.as_deref(), Some("jpn"));
    }

    #[test]
    fn stream_level_language_takes_precedence_over_program_level() {
        let mut prog = DescriptorSummary::default();
        prog.language_iso_639_2 = Some("jpn".to_string());
        let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"fra\x00");
        let row = build_row(0x1234, 1, &entry(0x0F, descs), &prog);
        assert_eq!(row.language.as_deref(), Some("fra"));
    }

    #[test]
    fn unknown_stream_type_falls_back_to_hex_id() {
        let row = build_row(
            0x1234,
            1,
            &entry(0xEE, vec![]),
            &DescriptorSummary::default(),
        );
        assert_eq!(row.codec_id, "0xEE");
        assert_eq!(row.track_kind, TrackKind::Unknown);
    }

    #[test]
    fn descriptor_walk_compiles_into_summary() {
        let mut descs = Vec::new();
        descs.extend(build_descriptor(TAG_ISO_639_LANGUAGE, b"eng\x00"));
        descs.extend(build_descriptor(TAG_AC3, &[]));
        let s = walk(&descs);
        assert_eq!(s.language_iso_639_2.as_deref(), Some("eng"));
        assert!(s.is_ac3);
    }

    #[test]
    fn canonical_codec_id_covers_known_stream_types() {
        let cases = [
            (0x01u8, "V_MPEG1"),
            (0x02, "V_MPEG2"),
            (0x03, "A_MPEG/L3"),
            (0x04, "A_MPEG/L3"),
            (0x0F, "A_AAC"),
            (0x10, "V_MPEG4/ISO/ASP"),
            (0x11, "A_AAC"),
            (0x1B, "V_MPEG4/ISO/AVC"),
            (0x24, "V_MPEGH/ISO/HEVC"),
            (0x80, "A_PCM"),
            (0x81, "A_AC3"),
            (0x82, "A_DTS"),
            (0x83, "A_TRUEHD"),
            (0x84, "A_EAC3"),
            (0x85, "A_DTS"),
            (0x86, "A_DTS"),
            (0x87, "A_EAC3"),
            (0x88, "A_DTS"),
            (0xA0, "V_VC1"),
            (0x90, "S_HDMV/PGS"),
            (0x92, "S_HDMV/TEXTST"),
            (0xA1, "A_AC3"),
            (0xA2, "A_AC3"),
        ];
        for (st, expected) in cases {
            assert_eq!(canonical_codec_id(st), Some(expected), "stream_type 0x{st:02X}");
        }
        assert_eq!(canonical_codec_id(0xEE), None);
    }
}
