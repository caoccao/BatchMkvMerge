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

//! Subtitle disposition.  Matroska subtitle tracks have no nested sub-tree
//! beyond what `common` already collects — we only need to derive whether
//! the codec is text-based (SRT, ASS, USF, WebVTT) or image-based
//! (VobSub, HDMV PGS) so the frontend can pick the right viewer.  Mirrors
//! the codec-ID switch in `r_matroska.cpp::id_track_kind` and friends.

use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;

#[derive(Debug, Default)]
pub struct SubtitleBuilder;

impl SubtitleBuilder {
    pub fn build_from_codec_id(self, codec_id: &str) -> SubtitleTrackProperties {
        let (text, variant) = classify(codec_id);
        SubtitleTrackProperties {
            text_subtitles: text,
            encoding: None,
            variant: Some(variant.to_string()),
            teletext_page: None,
        }
    }
}

/// Map a Matroska subtitle CodecID to `(is_text, human-readable variant)`.
/// Source: mkvtoolnix `codec.cpp` table + the discussion in
/// `r_matroska.cpp::verify_subtitle_track`.
fn classify(codec_id: &str) -> (bool, &'static str) {
    let upper = codec_id.to_ascii_uppercase();
    match upper.as_str() {
        "S_TEXT/UTF8" | "S_TEXT/ASCII" => (true, "SRT"),
        "S_TEXT/SSA" => (true, "SSA"),
        "S_TEXT/ASS" => (true, "ASS"),
        "S_TEXT/USF" => (true, "USF"),
        "S_TEXT/WEBVTT" => (true, "WebVTT"),
        "S_TEXT/MICRODVD" => (true, "MicroDVD"),
        "S_KATE" => (true, "Kate"),
        "S_VOBSUB" => (false, "VobSub"),
        "S_VOBSUB/ZLIB" => (false, "VobSub (zlib)"),
        "S_HDMV/PGS" => (false, "PGS"),
        "S_HDMV/TEXTST" => (true, "HDMV TextST"),
        "S_DVBSUB" => (false, "DVB Subtitles"),
        "S_IMAGE/BMP" => (false, "Bitmap"),
        "S_TX3G" => (true, "TX3G"),
        _ => {
            // Default to "text" for any S_TEXT/* CodecID; image for S_HDMV/* etc.
            if upper.starts_with("S_TEXT/") {
                (true, "Text")
            } else if upper.starts_with("S_HDMV/") {
                (false, "HDMV")
            } else {
                (false, "Subtitle")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(codec_id: &str) -> SubtitleTrackProperties {
        SubtitleBuilder.build_from_codec_id(codec_id)
    }

    #[test]
    fn srt_marked_as_text() {
        let s = build("S_TEXT/UTF8");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("SRT"));
    }

    #[test]
    fn ass_marked_as_text() {
        let s = build("S_TEXT/ASS");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("ASS"));
    }

    #[test]
    fn pgs_marked_as_image() {
        let s = build("S_HDMV/PGS");
        assert!(!s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("PGS"));
    }

    #[test]
    fn vobsub_marked_as_image() {
        let s = build("S_VOBSUB");
        assert!(!s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("VobSub"));
    }

    #[test]
    fn hdmv_textst_marked_as_text() {
        let s = build("S_HDMV/TEXTST");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("HDMV TextST"));
    }

    #[test]
    fn unknown_text_prefix_defaults_to_text() {
        let s = build("S_TEXT/UNKNOWN-VARIANT");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("Text"));
    }

    #[test]
    fn unknown_hdmv_prefix_defaults_to_image() {
        let s = build("S_HDMV/NEW-IMAGE-CODEC");
        assert!(!s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("HDMV"));
    }

    #[test]
    fn bare_unknown_codec_defaults_to_image_subtitle() {
        let s = build("S_UNKNOWN");
        assert!(!s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("Subtitle"));
    }

    #[test]
    fn case_insensitive_match() {
        let s = build("s_text/utf8");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("SRT"));
    }

    #[test]
    fn kate_marked_as_text() {
        let s = build("S_KATE");
        assert!(s.text_subtitles);
        assert_eq!(s.variant.as_deref(), Some("Kate"));
    }

    #[test]
    fn dvbsub_marked_as_image() {
        let s = build("S_DVBSUB");
        assert!(!s.text_subtitles);
    }
}
