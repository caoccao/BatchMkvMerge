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

use super::attachment::Attachment;
use super::chapter::ChapterSummary;
use super::container::Container;
use super::tag::TagsBundle;
use super::track::Track;
use super::warning::Warning;

/// Our own protocol version.  Bumped on breaking changes to the wire shape —
/// this is **not** the mkvmerge schema version.
pub const PARSER_PROTOCOL_VERSION: u32 = 1;

/// Root struct returned by [`crate::media_metadata::parse`] on success.  The
/// nested hierarchy is preserved across the wire — never flattened.  See
/// [[feedback-protocol-shape]].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MediaMetadata {
  pub protocol_version: u32,
  pub file_name: String,
  #[specta(type = Number)]
  pub file_size: u64,
  pub container: Container,
  pub tracks: Vec<Track>,
  pub attachments: Vec<Attachment>,
  pub chapters: ChapterSummary,
  pub tags: TagsBundle,
  pub warnings: Vec<Warning>,
}

impl MediaMetadata {
  /// Builder used by readers to seed a fresh result for one parse call.
  pub fn new(file_name: impl Into<String>, file_size: u64) -> Self {
    Self {
      protocol_version: PARSER_PROTOCOL_VERSION,
      file_name: file_name.into(),
      file_size,
      container: Container::default(),
      tracks: Vec::new(),
      attachments: Vec::new(),
      chapters: ChapterSummary::default(),
      tags: TagsBundle::default(),
      warnings: Vec::new(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn new_seeds_protocol_version_and_size() {
    let m = MediaMetadata::new("clip.mkv", 1024);
    assert_eq!(m.protocol_version, PARSER_PROTOCOL_VERSION);
    assert_eq!(m.file_name, "clip.mkv");
    assert_eq!(m.file_size, 1024);
    assert!(m.tracks.is_empty());
    assert!(m.attachments.is_empty());
    assert!(m.warnings.is_empty());
    assert_eq!(m.chapters.num_entries, 0);
  }

  #[test]
  fn protocol_version_is_one_until_breaking_change() {
    assert_eq!(PARSER_PROTOCOL_VERSION, 1);
  }

  #[test]
  fn root_round_trips_through_json() {
    let m = MediaMetadata::new("clip.mkv", 1024);
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("\"protocolVersion\":1"));
    assert!(s.contains("\"fileName\":\"clip.mkv\""));
    assert!(s.contains("\"fileSize\":1024"));
    assert!(s.contains("\"tracks\":[]"));
    assert!(s.contains("\"chapters\":{\"numEntries\":0,\"numEditions\":0}"));
    let back: MediaMetadata = serde_json::from_str(&s).unwrap();
    assert_eq!(back, m);
  }
}
