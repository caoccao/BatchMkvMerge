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

// `unsafe` is forbidden throughout the parser sub-tree — see plan §6.5.
#![forbid(unsafe_code)]

pub mod codec;
pub mod deadline;
pub mod error;
pub mod io;
pub mod language;
pub mod model;
pub mod reader;

pub use deadline::Deadline;
pub use error::ParseError;
pub use model::{
    MediaMetadata, PARSER_PROTOCOL_VERSION,
};
pub use reader::Reader;

use std::path::Path;

/// Tuning knobs for a single parse call. Built per-invocation from the user's
/// persisted config; never global. See plan §6.1 and [[feedback-parser-timeout]].
#[derive(Debug, Clone, Copy)]
pub struct ParseOptions {
    pub timeout_ms: u64,
    pub max_element_size: u64,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 1000,
            max_element_size: 16 * 1024 * 1024,
        }
    }
}

/// Public entry point. Phase 1 only wires the foundations — every call returns
/// `Err(Unrecognised)` until a format reader lands in a later phase. The
/// signature is final.
pub fn parse<P: AsRef<Path>>(_path: P, _options: ParseOptions) -> Result<(), ParseError> {
    Err(ParseError::Unrecognised)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_one_second_and_sixteen_mib() {
        let opts = ParseOptions::default();
        assert_eq!(opts.timeout_ms, 1000);
        assert_eq!(opts.max_element_size, 16 * 1024 * 1024);
    }

    #[test]
    fn parse_returns_unrecognised_in_phase_1() {
        let err = parse("nonexistent.mkv", ParseOptions::default()).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }
}
