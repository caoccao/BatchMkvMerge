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

/// Subtitle-track-only properties.  Populated only on tracks whose `trackType`
/// is `Subtitles`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrackProperties {
  /// `true` for text-based codecs (SRT, ASS, WebVTT, USF, MicroDVD).
  /// `false` for image / graphical codecs (PGS, VobSub, HDMV TextST).
  pub text_subtitles: bool,
  /// Detected source-file encoding for text subtitles (UTF-8, Windows-1252,
  /// ...).  Always `None` for graphical formats.
  pub encoding: Option<String>,
  /// Container-specific format variant when meaningful (e.g. ASS vs SSA,
  /// VobSub idx flavour).
  pub variant: Option<String>,
  /// DVB teletext page number for teletext-based subtitle PIDs.  Always
  /// `None` for non-DVB-TS files.
  pub teletext_page: Option<u32>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_is_image_subtitle_with_no_encoding() {
    let s = SubtitleTrackProperties::default();
    assert!(!s.text_subtitles);
    assert!(s.encoding.is_none());
    assert!(s.variant.is_none());
  }

  #[test]
  fn round_trip_text_subtitle() {
    let st = SubtitleTrackProperties {
      text_subtitles: true,
      encoding: Some("UTF-8".to_owned()),
      variant: Some("ASS".to_owned()),
      teletext_page: None,
    };
    let s = serde_json::to_string(&st).unwrap();
    assert!(s.contains("\"textSubtitles\":true"));
    assert!(s.contains("\"encoding\":\"UTF-8\""));
    let back: SubtitleTrackProperties = serde_json::from_str(&s).unwrap();
    assert_eq!(back, st);
  }

  #[test]
  fn round_trip_image_subtitle() {
    let st = SubtitleTrackProperties {
      text_subtitles: false,
      encoding: None,
      variant: Some("PGS".to_owned()),
      teletext_page: None,
    };
    let s = serde_json::to_string(&st).unwrap();
    assert!(s.contains("\"textSubtitles\":false"));
    let back: SubtitleTrackProperties = serde_json::from_str(&s).unwrap();
    assert_eq!(back, st);
  }

  #[test]
  fn round_trip_teletext_subtitle() {
    let st = SubtitleTrackProperties {
      text_subtitles: true,
      encoding: Some("UTF-8".to_owned()),
      variant: Some("DVB Teletext".to_owned()),
      teletext_page: Some(888),
    };
    let s = serde_json::to_string(&st).unwrap();
    assert!(s.contains("\"teletextPage\":888"));
    let back: SubtitleTrackProperties = serde_json::from_str(&s).unwrap();
    assert_eq!(back, st);
  }
}
