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

//! The message shown when a file cannot be parsed. Port of the React app's
//! `metadataError.ts`: each error kind has an i18n key under
//! `merge.error.parser.*` with named arguments; the caller translates it.

use bmm_core::protocol::MediaMetadataErrorPayload;

/// An i18n key with its named arguments, in the order the messages use them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedMessage {
  pub key: &'static str,
  pub args: Vec<(&'static str, String)>,
}

/// The message of a parse failure.
pub fn metadata_error_message(error: &MediaMetadataErrorPayload) -> LocalizedMessage {
  let detail_only = |key, detail: &String| LocalizedMessage {
    key,
    args: vec![("detail", detail.clone())],
  };
  match error {
    MediaMetadataErrorPayload::Io { detail } => detail_only("merge.error.parser.io", detail),
    MediaMetadataErrorPayload::UnexpectedEof { detail } => detail_only("merge.error.parser.unexpectedEof", detail),
    MediaMetadataErrorPayload::Unrecognised { detail } => detail_only("merge.error.parser.unrecognised", detail),
    MediaMetadataErrorPayload::Timeout {
      budget_ms,
      stage,
      detail,
    } => LocalizedMessage {
      key: "merge.error.parser.timeout",
      args: vec![
        ("budgetMs", budget_ms.to_string()),
        ("stage", stage.clone()),
        ("detail", detail.clone()),
      ],
    },
    MediaMetadataErrorPayload::Malformed { detail } => detail_only("merge.error.parser.malformed", detail),
    MediaMetadataErrorPayload::OversizedElement { detail } => {
      detail_only("merge.error.parser.oversizedElement", detail)
    }
    MediaMetadataErrorPayload::Internal { detail } => detail_only("merge.error.parser.internal", detail),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn maps_every_kind_to_its_key() {
    let detail = || "boom".to_string();
    let cases = [
      (
        MediaMetadataErrorPayload::Io { detail: detail() },
        "merge.error.parser.io",
      ),
      (
        MediaMetadataErrorPayload::UnexpectedEof { detail: detail() },
        "merge.error.parser.unexpectedEof",
      ),
      (
        MediaMetadataErrorPayload::Unrecognised { detail: detail() },
        "merge.error.parser.unrecognised",
      ),
      (
        MediaMetadataErrorPayload::Malformed { detail: detail() },
        "merge.error.parser.malformed",
      ),
      (
        MediaMetadataErrorPayload::OversizedElement { detail: detail() },
        "merge.error.parser.oversizedElement",
      ),
      (
        MediaMetadataErrorPayload::internal("boom"),
        "merge.error.parser.internal",
      ),
    ];
    for (error, key) in cases {
      assert_eq!(
        metadata_error_message(&error),
        LocalizedMessage {
          key,
          args: vec![("detail", "boom".to_string())],
        }
      );
    }
  }

  #[test]
  fn passes_the_timeout_budget_and_stage() {
    let error = MediaMetadataErrorPayload::Timeout {
      budget_ms: 10_000,
      stage: "tracks".to_string(),
      detail: "took too long".to_string(),
    };
    assert_eq!(
      metadata_error_message(&error),
      LocalizedMessage {
        key: "merge.error.parser.timeout",
        args: vec![
          ("budgetMs", "10000".to_string()),
          ("stage", "tracks".to_string()),
          ("detail", "took too long".to_string()),
        ],
      }
    );
  }
}
