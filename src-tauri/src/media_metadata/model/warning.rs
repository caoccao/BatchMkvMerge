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

/// Non-fatal observations made during parse.  Fatal errors are returned as
/// `Err(ParseError)` and never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
  pub category: WarningCategory,
  pub message: String,
  /// Byte offset within the source file where the situation was detected,
  /// or `None` for category-wide warnings that don't tie to a single byte.
  #[specta(type = Option<Number>)]
  pub offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WarningCategory {
  /// Best-effort interpretation of a forward-compat field (e.g. EBML version
  /// newer than parser supports).
  ForwardCompat,
  /// Recognised but unsupported / un-decodable substructure was skipped.
  Skipped,
  /// Heuristic fallback was used because authoritative metadata was missing.
  Heuristic,
  /// Value was out of the spec-permitted range and was clamped.
  Clamped,
  /// A registered language / codec id was not in the parser's lookup table;
  /// the raw value was passed through.
  Unknown,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn warning_round_trips_through_json() {
    let w = Warning {
      category: WarningCategory::ForwardCompat,
      message: "EBML version 3 newer than supported 1".to_owned(),
      offset: Some(42),
    };
    let s = serde_json::to_string(&w).unwrap();
    assert!(s.contains("\"category\":\"forwardCompat\""));
    assert!(s.contains("\"offset\":42"));
    let back: Warning = serde_json::from_str(&s).unwrap();
    assert_eq!(back, w);
  }

  #[test]
  fn warning_without_offset_serializes_null() {
    let w = Warning {
      category: WarningCategory::Unknown,
      message: "codec id pass-through".to_owned(),
      offset: None,
    };
    let s = serde_json::to_string(&w).unwrap();
    assert!(s.contains("\"offset\":null"));
  }

  #[test]
  fn all_categories_round_trip() {
    for cat in [
      WarningCategory::ForwardCompat,
      WarningCategory::Skipped,
      WarningCategory::Heuristic,
      WarningCategory::Clamped,
      WarningCategory::Unknown,
    ] {
      let json = serde_json::to_string(&cat).unwrap();
      let back: WarningCategory = serde_json::from_str(&json).unwrap();
      assert_eq!(back, cat);
    }
  }
}
