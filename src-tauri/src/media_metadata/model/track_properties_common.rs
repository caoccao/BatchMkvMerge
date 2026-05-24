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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use crate::media_metadata::language::Language;

/// Track-level properties shared across all track kinds.  Domain-specific
/// fields live on `TrackProperties.video / audio / subtitle` — see
/// [[feedback-protocol-shape]].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommonTrackProperties {
    /// Container-assigned 1-based number (Matroska TrackNumber, MP4 trackID).
    #[specta(type = Option<Number>)]
    pub number: Option<u64>,
    /// Stable unique id (Matroska TrackUID).  Hex-encoded because 64-bit
    /// values can exceed JavaScript's safe-integer range.
    pub uid_hex: Option<String>,
    /// User-facing track name when the container provides one.
    pub track_name: Option<String>,
    /// Resolved language pair (ISO-639-2 + BCP-47 IETF).  See
    /// [`crate::media_metadata::language::Language`].
    pub language: Option<Language>,
    pub enabled: TrackFlag,
    pub default: TrackFlag,
    pub forced: TrackFlag,
    pub hearing_impaired: Option<bool>,
    pub visual_impaired: Option<bool>,
    pub text_descriptions: Option<bool>,
    pub original: Option<bool>,
    pub commentary: Option<bool>,
    #[specta(type = Option<Number>)]
    pub seek_pre_roll_ns: Option<u64>,
    #[specta(type = Option<Number>)]
    pub codec_delay_ns: Option<u64>,
    /// Maximum payload size advertised by the container (Matroska
    /// MaxBlockAdditionID descriptor, MP4 nalLengthSize, ...).  Optional.
    #[specta(type = Option<Number>)]
    pub max_block_addition_id: Option<u64>,
    /// Container compression / encryption descriptors applied to the track
    /// (Matroska ContentEncodings).  Human-readable algorithm names, e.g.
    /// `["zlib"]`.  Empty when the track has none.
    pub content_encodings: Vec<String>,
    /// MPEG-TS / MPEG-PS stream identifier (PID on TS, stream_id on PS).
    pub stream_id: Option<u32>,
    /// Sub-stream identifier (BD audio sub-stream, PES private stream sub-id).
    pub sub_stream_id: Option<u32>,
    /// MPEG-TS program number this track belongs to.  When set, matches one
    /// of the [`super::container::ContainerProperties::programs`] entries.
    pub program_number: Option<u32>,
    /// DVB teletext page number (`teletext_page` in mkvmerge JSON).  Only
    /// populated for teletext substreams.
    pub teletext_page: Option<u32>,
    /// Other tracks sharing this packetized substream (MPEG-TS PES
    /// multiplex with DVB-Sub subtitles, ...).  Holds Track.id values.
    #[specta(type = Vec<Number>)]
    pub multiplexed_with: Vec<i64>,
    /// Lowest cluster / segment-start timestamp seen during header walk.
    /// Matroska-specific; `None` for all other containers.
    #[specta(type = Option<Number>)]
    pub minimum_timestamp_ns: Option<u64>,
    /// Number of Matroska Cue entries pointing at this track.
    #[specta(type = Option<Number>)]
    pub num_index_entries: Option<u64>,
}

/// A presence-aware tri-state for `FlagEnabled` / `FlagDefault` / `FlagForced`.
/// We distinguish "absent" from "explicitly false" so the frontend can fall
/// back to the spec default rather than guess.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TrackFlag {
    /// Field not present in the source container; spec default applies.
    #[default]
    Unspecified,
    True,
    False,
}

impl TrackFlag {
    pub fn from_bool(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }

    pub fn resolve_with_default(self, default: bool) -> bool {
        match self {
            Self::Unspecified => default,
            Self::True => true,
            Self::False => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::language::Language;

    #[test]
    fn default_is_all_unspecified() {
        let c = CommonTrackProperties::default();
        assert!(c.number.is_none());
        assert!(c.language.is_none());
        assert_eq!(c.enabled, TrackFlag::Unspecified);
        assert_eq!(c.default, TrackFlag::Unspecified);
        assert_eq!(c.forced, TrackFlag::Unspecified);
    }

    #[test]
    fn track_flag_round_trip() {
        for f in [TrackFlag::Unspecified, TrackFlag::True, TrackFlag::False] {
            let back: TrackFlag =
                serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
            assert_eq!(back, f);
        }
    }

    #[test]
    fn track_flag_from_bool() {
        assert_eq!(TrackFlag::from_bool(true), TrackFlag::True);
        assert_eq!(TrackFlag::from_bool(false), TrackFlag::False);
    }

    #[test]
    fn track_flag_resolves_with_spec_default() {
        assert!(TrackFlag::Unspecified.resolve_with_default(true));
        assert!(!TrackFlag::Unspecified.resolve_with_default(false));
        assert!(TrackFlag::True.resolve_with_default(false));
        assert!(!TrackFlag::False.resolve_with_default(true));
    }

    #[test]
    fn round_trips_through_json() {
        let c = CommonTrackProperties {
            number: Some(1),
            uid_hex: Some("0123456789abcdef".to_owned()),
            track_name: Some("English Commentary".to_owned()),
            language: Some(Language::from_iso_639_2("eng")),
            enabled: TrackFlag::True,
            default: TrackFlag::False,
            forced: TrackFlag::False,
            hearing_impaired: Some(false),
            visual_impaired: None,
            text_descriptions: None,
            original: Some(true),
            commentary: Some(true),
            seek_pre_roll_ns: Some(80_000_000),
            codec_delay_ns: Some(0),
            max_block_addition_id: None,
            content_encodings: vec!["zlib".to_owned()],
            stream_id: Some(0x1100),
            sub_stream_id: Some(0xa1),
            program_number: Some(1),
            teletext_page: None,
            multiplexed_with: vec![1, 2],
            minimum_timestamp_ns: Some(0),
            num_index_entries: Some(120),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"trackName\":\"English Commentary\""));
        assert!(s.contains("\"enabled\":\"true\""));
        assert!(s.contains("\"default\":\"false\""));
        assert!(s.contains("\"contentEncodings\":[\"zlib\"]"));
        assert!(s.contains("\"streamId\":4352"));
        assert!(s.contains("\"programNumber\":1"));
        assert!(s.contains("\"multiplexedWith\":[1,2]"));
        let back: CommonTrackProperties = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
    }
}
