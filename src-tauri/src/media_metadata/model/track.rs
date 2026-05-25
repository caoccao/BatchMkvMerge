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

use super::tag::TagEntry;
use super::track_properties_audio::AudioTrackProperties;
use super::track_properties_common::CommonTrackProperties;
use super::track_properties_subtitle::SubtitleTrackProperties;
use super::track_properties_video::VideoTrackProperties;

/// One track / stream / elementary substream of the parsed file.  The whole
/// shape is camelCase with the domain sub-trees (`video / audio / subtitle`)
/// stored as `Option<_>` so the frontend can ignore the irrelevant ones — see
/// [[feedback-protocol-shape]].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Track {
  /// 0-based parser-assigned identifier — stable for a single parse
  /// invocation, mirrors `mkvmerge -J` ordering.
  #[specta(type = Number)]
  pub id: i64,
  pub track_type: TrackType,
  pub codec: CodecInfo,
  pub properties: TrackProperties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TrackType {
  Video,
  Audio,
  Subtitles,
  Buttons,
  Unknown,
}

/// Typed codec descriptor.  The container's CodecID is exposed as `id` (raw
/// string for Matroska, FOURCC for AVI/MP4, stream_type byte for MPEG-TS);
/// `name` is the catalogue lookup; `codecPrivate` carries the codec-private
/// blob (typed properties live on the per-track domain sub-tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CodecInfo {
  pub id: String,
  pub name: Option<String>,
  pub codec_private: Option<CodecPrivate>,
}

/// Codec-private blob.  We expose both the byte length and a hex dump so the
/// frontend can show a length without having to copy the whole hex string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CodecPrivate {
  #[specta(type = Number)]
  pub length: u64,
  pub hex: String,
}

impl CodecPrivate {
  pub fn from_bytes(bytes: &[u8]) -> Self {
    Self {
      length: bytes.len() as u64,
      hex: bytes.iter().map(|b| format!("{:02x}", b)).collect(),
    }
  }
}

/// Per-track properties.  `common` is always present; the domain sub-trees
/// (`video / audio / subtitle`) are populated based on `track.trackType` and
/// never share fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrackProperties {
  pub common: CommonTrackProperties,
  pub video: Option<VideoTrackProperties>,
  pub audio: Option<AudioTrackProperties>,
  pub subtitle: Option<SubtitleTrackProperties>,
  pub tags: Vec<TagEntry>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn track_type_round_trip() {
    for t in [
      TrackType::Video,
      TrackType::Audio,
      TrackType::Subtitles,
      TrackType::Buttons,
      TrackType::Unknown,
    ] {
      let back: TrackType = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
      assert_eq!(back, t);
    }
  }

  #[test]
  fn codec_private_from_bytes() {
    let cp = CodecPrivate::from_bytes(&[0x01, 0x64, 0x00, 0x1f]);
    assert_eq!(cp.length, 4);
    assert_eq!(cp.hex, "0164001f");
  }

  #[test]
  fn codec_private_empty() {
    let cp = CodecPrivate::from_bytes(&[]);
    assert_eq!(cp.length, 0);
    assert_eq!(cp.hex, "");
  }

  #[test]
  fn codec_info_round_trip() {
    let c = CodecInfo {
      id: "V_MPEG4/ISO/AVC".to_owned(),
      name: Some("AVC/H.264/MPEG-4p10".to_owned()),
      codec_private: Some(CodecPrivate::from_bytes(&[0xff, 0x00, 0xa1])),
    };
    let s = serde_json::to_string(&c).unwrap();
    assert!(s.contains("\"id\":\"V_MPEG4/ISO/AVC\""));
    assert!(s.contains("\"codecPrivate\":{"));
    assert!(s.contains("\"hex\":\"ff00a1\""));
    let back: CodecInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(back, c);
  }

  #[test]
  fn track_round_trip_video() {
    let t = Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_MPEG4/ISO/AVC".to_owned(),
        name: Some("AVC".to_owned()),
        codec_private: None,
      },
      properties: TrackProperties {
        common: CommonTrackProperties::default(),
        video: Some(VideoTrackProperties::default()),
        audio: None,
        subtitle: None,
        tags: vec![],
      },
    };
    let s = serde_json::to_string(&t).unwrap();
    assert!(s.contains("\"trackType\":\"video\""));
    assert!(s.contains("\"video\":{"));
    assert!(s.contains("\"audio\":null"));
    let back: Track = serde_json::from_str(&s).unwrap();
    assert_eq!(back, t);
  }

  #[test]
  fn track_properties_default_has_no_domain() {
    let tp = TrackProperties::default();
    assert!(tp.video.is_none());
    assert!(tp.audio.is_none());
    assert!(tp.subtitle.is_none());
    assert!(tp.tags.is_empty());
  }
}
