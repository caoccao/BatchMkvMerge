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

/// A single name/value tag pair sourced from container metadata
/// (Matroska SimpleTag, MP4 iTunes ilst, ID3v2 frame, VorbisComment, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TagEntry {
  pub name: String,
  pub value: String,
  /// BCP-47 language code if the source format provides one.
  pub language: Option<String>,
}

/// All container-level tags bucketed by reach.  Per-track tags live on each
/// [`super::track::Track`] via `properties.tags`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TagsBundle {
  /// Tags that apply to the whole file (Matroska tags with no TrackUID, MP4
  /// /moov/udta/meta, OGG VorbisComment block, ID3v2 frame outside MPEG
  /// streams, ...).
  pub global: Vec<TagEntry>,
  /// Sum of `Track.properties.tags.len()` across every track — duplicated
  /// here so the frontend can rule out per-track tags without walking
  /// `tracks[]`.  Always derived from the tracks list — never authored
  /// independently.
  pub per_track_count: u32,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn empty_default() {
    let t = TagsBundle::default();
    assert!(t.global.is_empty());
    assert_eq!(t.per_track_count, 0);
  }

  #[test]
  fn entry_round_trips_through_json() {
    let entry = TagEntry {
      name: "ARTIST".to_owned(),
      value: "Hans Zimmer".to_owned(),
      language: Some("en".to_owned()),
    };
    let s = serde_json::to_string(&entry).unwrap();
    assert!(s.contains("\"language\":\"en\""));
    let back: TagEntry = serde_json::from_str(&s).unwrap();
    assert_eq!(back, entry);
  }

  #[test]
  fn bundle_round_trips_through_json() {
    let bundle = TagsBundle {
      global: vec![TagEntry {
        name: "TITLE".to_owned(),
        value: "Movie".to_owned(),
        language: None,
      }],
      per_track_count: 3,
    };
    let s = serde_json::to_string(&bundle).unwrap();
    assert!(s.contains("\"perTrackCount\":3"));
    let back: TagsBundle = serde_json::from_str(&s).unwrap();
    assert_eq!(back, bundle);
  }
}
