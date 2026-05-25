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

/// Identification-time summary of the chapter editions in a file.  We do not
/// extract individual chapter titles / timecodes at identification time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChapterSummary {
  /// Total entries summed across all editions.
  pub num_entries: u32,
  /// Number of distinct editions / playlists.
  pub num_editions: u32,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_is_zero() {
    let c = ChapterSummary::default();
    assert_eq!(c.num_entries, 0);
    assert_eq!(c.num_editions, 0);
  }

  #[test]
  fn round_trips_through_json() {
    let c = ChapterSummary {
      num_entries: 12,
      num_editions: 2,
    };
    let s = serde_json::to_string(&c).unwrap();
    assert!(s.contains("\"numEntries\":12"));
    assert!(s.contains("\"numEditions\":2"));
    let back: ChapterSummary = serde_json::from_str(&s).unwrap();
    assert_eq!(back, c);
  }
}
