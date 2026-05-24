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

//! SSA / ASS reader.
//!
//! Both versions begin with a `[Script Info]` section.  The distinguishing
//! factor between SSA (v4) and ASS (v4+) is the `ScriptType:` value and the
//! styles-section header (`[V4 Styles]` vs `[V4+ Styles]`).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsaVariant {
    Ssa,
    Ass,
}

pub fn classify(text: &str) -> Option<SsaVariant> {
    let mut script_info_seen = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower == "[script info]" {
            script_info_seen = true;
            continue;
        }
        if lower == "[v4+ styles]" {
            return Some(SsaVariant::Ass);
        }
        if lower == "[v4 styles]" {
            return Some(SsaVariant::Ssa);
        }
        if let Some(rest) = lower.strip_prefix("scripttype:") {
            let v = rest.trim();
            if v.contains("v4.00+") {
                return Some(SsaVariant::Ass);
            }
            if v.contains("v4.00") {
                return Some(SsaVariant::Ssa);
            }
        }
    }
    if script_info_seen {
        // Header alone → assume modern ASS.
        Some(SsaVariant::Ass)
    } else {
        None
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SsaReader;

impl Reader for SsaReader {
    fn name(&self) -> &'static str {
        "ssa"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        Ok(read > 0 && classify(&encoding::decode_lossy(&buf[..read])).is_some())
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
        let detected = encoding::detect(&buf[..read]);
        let variant = classify(&encoding::decode_lossy(&buf[..read]))
            .ok_or(ParseError::Unrecognised)?;

        let (codec_id, codec_name, variant_label, format) = match variant {
            SsaVariant::Ass => (
                "S_TEXT/ASS",
                "ASS subtitles",
                "ASS",
                ContainerFormat::SsaAss,
            ),
            SsaVariant::Ssa => (
                "S_TEXT/SSA",
                "SSA subtitles",
                "SSA",
                ContainerFormat::SsaAss,
            ),
        };
        out.container.format = format;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Subtitles,
            codec: CodecInfo {
                id: codec_id.to_string(),
                name: Some(codec_name.to_string()),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                subtitle: Some(SubtitleTrackProperties {
                    text_subtitles: true,
                    encoding: Some(detected.label.to_string()),
                    variant: Some(variant_label.to_string()),
                    teletext_page: None,
                }),
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn classify_ass_via_styles_section() {
        let text = "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n";
        assert_eq!(classify(text), Some(SsaVariant::Ass));
    }

    #[test]
    fn classify_ssa_via_styles_section() {
        let text = "[Script Info]\n\n[V4 Styles]\n";
        assert_eq!(classify(text), Some(SsaVariant::Ssa));
    }

    #[test]
    fn classify_ass_via_script_type() {
        let text = "[Script Info]\nScriptType: v4.00+\n";
        assert_eq!(classify(text), Some(SsaVariant::Ass));
    }

    #[test]
    fn classify_ssa_via_script_type() {
        let text = "[Script Info]\nScriptType: v4.00\n";
        assert_eq!(classify(text), Some(SsaVariant::Ssa));
    }

    #[test]
    fn classify_returns_none_without_script_info() {
        assert!(classify("[Events]\n").is_none());
    }

    #[test]
    fn classify_falls_back_to_ass_when_only_script_info_seen() {
        assert_eq!(classify("[Script Info]\n"), Some(SsaVariant::Ass));
    }

    #[test]
    fn probe_accepts_ass_blob() {
        let blob = b"[Script Info]\nScriptType: v4.00+\n";
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
        assert!(SsaReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_ass_track() {
        use crate::media_metadata::deadline::Deadline;
        let blob = b"[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\n";
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
        let mut out = MediaMetadata::new("clip.ass", 0);
        SsaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.id, "S_TEXT/ASS");
    }

    #[test]
    fn read_headers_emits_ssa_track() {
        use crate::media_metadata::deadline::Deadline;
        let blob = b"[Script Info]\n[V4 Styles]\n";
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
        let mut out = MediaMetadata::new("clip.ssa", 0);
        SsaReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.id, "S_TEXT/SSA");
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
        assert!(!SsaReader.probe(&mut s).unwrap());
    }
}
