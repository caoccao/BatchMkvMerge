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

// `unsafe` is forbidden throughout the parser sub-tree.
#![forbid(unsafe_code)]

pub mod audio;
pub mod avi;
pub mod codec;
pub mod coreaudio;
pub mod deadline;
pub mod elementary;
pub mod error;
pub mod io;
pub mod language;
pub mod matroska;
pub mod model;
pub mod mp4;
pub mod mpeg_ps;
pub mod mpeg_ts;
pub mod ogg;
pub mod probe;
pub mod reader;

pub use deadline::Deadline;
pub use error::ParseError;
pub use model::{
    MediaMetadata, PARSER_PROTOCOL_VERSION,
};
pub use reader::Reader;

use std::path::Path;

use crate::media_metadata::io::file_source::FileSource;

/// Tuning knobs for a single parse call. Built per-invocation from the user's
/// persisted config; never global. See [[feedback-parser-timeout]].
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

/// Public entry point.  Opens `path`, builds a `FileSource`, runs the probe
/// cascade and returns a populated `MediaMetadata` on success.
///
/// As of Phase 3 the Matroska reader is the only registered format reader.
/// Files of types whose reader has not yet landed return
/// `Err(ParseError::Unrecognised)`.
pub fn parse<P: AsRef<Path>>(
    path: P,
    options: ParseOptions,
) -> Result<MediaMetadata, ParseError> {
    let path_ref = path.as_ref();
    let mut src = FileSource::open(path_ref)?;
    let file_name = path_ref
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let file_size = src.length().unwrap_or(0);
    let deadline = Deadline::new(options.timeout_ms);
    let mut metadata = MediaMetadata::new(file_name, file_size);
    probe::dispatch(&mut src, &deadline, &mut metadata)?;
    Ok(metadata)
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
    fn parse_returns_io_error_when_file_missing() {
        let err = parse("does-not-exist-12345.mkv", ParseOptions::default()).unwrap_err();
        assert!(matches!(err, ParseError::Io { .. }));
    }
}
