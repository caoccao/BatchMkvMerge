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

use std::io;

use thiserror::Error;

/// Every fatal parse outcome. The first `Err` short-circuits via `?` from the
/// deepest parser call all the way back to `media_metadata::parse`.
/// Non-fatal mkvmerge-style warnings live on `MediaMetadata::warnings`, not here.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("I/O at offset {offset}: {source}")]
    Io {
        offset: u64,
        #[source]
        source: io::Error,
    },

    #[error("unexpected end of file at offset {offset} (wanted {wanted} more bytes)")]
    UnexpectedEof { offset: u64, wanted: u64 },

    #[error("unrecognised file format")]
    Unrecognised,

    #[error("malformed {format} at offset {offset}: {reason}")]
    Malformed {
        format: &'static str,
        offset: u64,
        reason: String,
    },

    #[error("{format} element {id:#x} too large ({size} bytes, cap {cap}) at offset {offset}")]
    OversizedElement {
        format: &'static str,
        id: u64,
        size: u64,
        cap: u64,
        offset: u64,
    },

    #[error("operation exceeded {budget_ms} ms (stage: {stage})")]
    Timeout { budget_ms: u64, stage: &'static str },
}

impl ParseError {
    /// Convenience for the common pattern in I/O sites: pair an `io::Error`
    /// with the stream offset where it surfaced.
    pub fn io_at(offset: u64, source: io::Error) -> Self {
        Self::Io { offset, source }
    }

    /// Whether this error is a clean "we don't recognise this container"
    /// signal versus a fatal parse failure. The probe cascade uses this to
    /// fall through to the next reader.
    pub fn is_unrecognised(&self) -> bool {
        matches!(self, Self::Unrecognised)
    }

    /// Stage label for `Timeout`, or `"-"` for other variants. Useful for
    /// log lines that always want a stage string.
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Timeout { stage, .. } => stage,
            _ => "-",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn io_at_pairs_offset_and_source() {
        let err = ParseError::io_at(42, io::Error::new(ErrorKind::PermissionDenied, "nope"));
        match err {
            ParseError::Io { offset, source } => {
                assert_eq!(offset, 42);
                assert_eq!(source.kind(), ErrorKind::PermissionDenied);
            }
            _ => panic!("expected Io variant"),
        }
    }

    #[test]
    fn unrecognised_predicate_is_true_only_for_unrecognised() {
        assert!(ParseError::Unrecognised.is_unrecognised());
        assert!(!ParseError::UnexpectedEof { offset: 0, wanted: 1 }.is_unrecognised());
        assert!(!ParseError::Malformed {
            format: "matroska",
            offset: 0,
            reason: "bad".to_string(),
        }
        .is_unrecognised());
    }

    #[test]
    fn stage_is_static_for_timeout_and_dash_otherwise() {
        let t = ParseError::Timeout {
            budget_ms: 1000,
            stage: "matroska::seek_head",
        };
        assert_eq!(t.stage(), "matroska::seek_head");
        assert_eq!(ParseError::Unrecognised.stage(), "-");
    }

    #[test]
    fn display_format_is_descriptive() {
        let s = format!(
            "{}",
            ParseError::OversizedElement {
                format: "matroska",
                id: 0x1A45DFA3,
                size: 1 << 30,
                cap: 16 * 1024 * 1024,
                offset: 0,
            }
        );
        assert!(s.contains("matroska"), "missing format: {s}");
        assert!(s.contains("0x1a45dfa3"), "missing id: {s}");
        assert!(s.contains("1073741824"), "missing size: {s}");
    }

    #[test]
    fn unexpected_eof_carries_wanted_bytes() {
        let s = format!(
            "{}",
            ParseError::UnexpectedEof {
                offset: 100,
                wanted: 4,
            }
        );
        assert!(s.contains("100"), "missing offset: {s}");
        assert!(s.contains("wanted 4"), "missing wanted: {s}");
    }

    #[test]
    fn malformed_includes_reason() {
        let s = format!(
            "{}",
            ParseError::Malformed {
                format: "mp4",
                offset: 0,
                reason: "atom size 0 in non-final position".to_string(),
            }
        );
        assert!(s.contains("mp4"));
        assert!(s.contains("atom size 0 in non-final position"));
    }
}
