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

//! Results the core hands to the application: the tool detection status, the
//! reason a file could not be parsed, and the outcome of an update check.

#[derive(Debug, Clone)]
pub struct MkvToolNixStatus {
  pub found: bool,
  pub mkv_toolnix_path: String,
}

#[derive(Debug, Clone)]
pub struct BetterMediaInfoStatus {
  pub found: bool,
  pub path: String,
}

/// Why a file could not be parsed. The application keeps it per file and
/// picks the message from the variant; `detail` is a one-line
/// human-readable summary.
#[derive(Debug, Clone)]
pub enum MediaMetadataErrorPayload {
  Io {
    detail: String,
  },
  UnexpectedEof {
    detail: String,
  },
  Unrecognised {
    detail: String,
  },
  Timeout {
    budget_ms: u64,
    stage: String,
    detail: String,
  },
  Malformed {
    detail: String,
  },
  OversizedElement {
    detail: String,
  },
  Internal {
    detail: String,
  },
}

impl MediaMetadataErrorPayload {
  pub fn from_parse_error(err: &crate::media_metadata::ParseError) -> Self {
    use crate::media_metadata::ParseError;
    let detail = err.to_string();
    match err {
      ParseError::Io { .. } => Self::Io { detail },
      ParseError::UnexpectedEof { .. } => Self::UnexpectedEof { detail },
      ParseError::Unrecognised => Self::Unrecognised { detail },
      ParseError::Timeout { budget_ms, stage } => Self::Timeout {
        budget_ms: *budget_ms,
        stage: (*stage).to_owned(),
        detail,
      },
      ParseError::Malformed { .. } => Self::Malformed { detail },
      ParseError::OversizedElement { .. } => Self::OversizedElement { detail },
      ParseError::Internal { .. } => Self::Internal { detail },
    }
  }

  pub fn internal(detail: impl Into<String>) -> Self {
    Self::Internal { detail: detail.into() }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheckResult {
  pub has_update: bool,
  pub latest_version: Option<String>,
}
