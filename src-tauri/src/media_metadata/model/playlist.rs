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

use super::duration::DurationValue;

/// Blu-ray / multi-file playlist metadata.  Mirrors mkvmerge's
/// `playlist*` identification fields (sourced from
/// `mm_mpls_multi_file_io.cpp`).  Populated only when the input is a
/// `.mpls` playlist that demultiplexes to several `.m2ts` segment files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
  /// Total duration across all segment files in the playlist.
  pub duration: Option<DurationValue>,
  /// Number of chapter entries declared by the playlist.
  pub chapters: u32,
  /// Sum of segment-file sizes (bytes).
  #[specta(type = Number)]
  pub total_size: u64,
  /// File-system paths of the segment files in playback order.
  pub files: Vec<String>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_is_empty_playlist() {
    let p = PlaylistInfo::default();
    assert!(p.duration.is_none());
    assert_eq!(p.chapters, 0);
    assert_eq!(p.total_size, 0);
    assert!(p.files.is_empty());
  }

  #[test]
  fn round_trips_through_json() {
    let p = PlaylistInfo {
      duration: Some(DurationValue::from_ns(7_200_000_000_000)),
      chapters: 12,
      total_size: 24_000_000_000,
      files: vec!["00001.m2ts".to_owned(), "00002.m2ts".to_owned()],
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"chapters\":12"));
    assert!(s.contains("\"totalSize\":24000000000"));
    assert!(s.contains("\"files\":[\"00001.m2ts\",\"00002.m2ts\"]"));
    let back: PlaylistInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(back, p);
  }
}
