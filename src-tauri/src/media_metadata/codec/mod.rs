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

//! Codec-identifier catalogues: Matroska CodecIDs, FOURCCs, MPEG-TS stream
//! types.

pub mod fourcc;
pub mod matroska_codec_ids;
pub mod mpegts_stream_types;

use crate::media_metadata::model::TrackType;

/// Coarse track classification used by the codec catalogues.  Maps to the
/// wire-format [`crate::media_metadata::model::TrackType`] enum via
/// [`TrackKind::to_track_type`].  We use a separate enum here so the codec
/// catalogues don't depend on serde / specta and can stay zero-cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Button,
    Unknown,
}

impl TrackKind {
    pub fn to_track_type(self) -> TrackType {
        match self {
            TrackKind::Video => TrackType::Video,
            TrackKind::Audio => TrackType::Audio,
            TrackKind::Subtitle => TrackType::Subtitles,
            TrackKind::Button => TrackType::Buttons,
            TrackKind::Unknown => TrackType::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_kind_maps_to_track_type() {
        assert_eq!(TrackKind::Video.to_track_type(), TrackType::Video);
        assert_eq!(TrackKind::Audio.to_track_type(), TrackType::Audio);
        assert_eq!(TrackKind::Subtitle.to_track_type(), TrackType::Subtitles);
        assert_eq!(TrackKind::Button.to_track_type(), TrackType::Buttons);
        assert_eq!(TrackKind::Unknown.to_track_type(), TrackType::Unknown);
    }
}
