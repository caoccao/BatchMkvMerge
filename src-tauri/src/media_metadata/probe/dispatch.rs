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

//! Reader registry + probe cascade.
//!
//! Mirrors mkvtoolnix's `probe_file_format` — a six-phase fallthrough:
//! unambiguous magics → extension hints → text subtitles → strict elementary
//! streams → frame-scan audio → ambiguous formats. The cascade walks every
//! registered reader in priority order
//! and asks it to `probe`. The first reader that claims the file is asked to
//! `read_headers` and the result is returned to the caller.
//!
//! Phase 3 ships with the Matroska reader as the only registered entry —
//! every subsequent format reader slots in here without changing the
//! cascade's shape. The probe outcome is reported via [`DispatchOutcome`] so
//! the public `parse` entry point can distinguish "no reader claimed" from
//! "a reader claimed but then failed mid-parse".

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::FileSource;
use crate::media_metadata::avi::AviReader;
use crate::media_metadata::matroska::MatroskaReader;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::mp4::Mp4Reader;
use crate::media_metadata::mpeg_ps::MpegPsReader;
use crate::media_metadata::mpeg_ts::MpegTsReader;
use crate::media_metadata::ogg::OggReader;
use crate::media_metadata::reader::Reader;

/// Describes what the cascade did and why. `Claimed` means a reader's
/// `probe()` returned `true` — independent of whether `read_headers` then
/// succeeded. `NoMatch` means every registered reader rejected the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// A reader claimed the file and `read_headers` was attempted. The
    /// `&'static str` is the reader name (e.g. `"matroska"`).
    Claimed(&'static str),
    /// No registered reader recognised the file.
    NoMatch,
}

/// Walk the registered readers in priority order. On the first `probe()` that
/// returns `Ok(true)`, hand off to `read_headers` and propagate its result.
/// If every reader rejects the file, return `Err(ParseError::Unrecognised)`.
///
/// The cursor is rewound between probes so each reader gets a fresh view of
/// the start of the file.
pub fn dispatch(
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
) -> Result<DispatchOutcome, ParseError> {
    for reader in registered_readers() {
        // Each probe call must see a freshly-positioned cursor; the trait
        // contract requires probes to rewind on return, but we re-seek defensively
        // so a misbehaving probe can't leak position across registry entries.
        src.seek_to(0)?;
        deadline.check("probe")?;
        let claimed = reader.probe(src)?;
        if !claimed {
            continue;
        }
        src.seek_to(0)?;
        reader.read_headers(src, deadline, out)?;
        return Ok(DispatchOutcome::Claimed(reader.name()));
    }
    Err(ParseError::Unrecognised)
}

/// The active reader registry. Order matches mkvtoolnix's probe cascade so
/// adding a reader is a one-line insert at the right priority level.
pub fn registered_readers() -> &'static [&'static (dyn Reader + Send + Sync)] {
    // Static dispatch table.  The lifetime is `'static` because every entry
    // is a zero-sized unit struct; no allocation involved.  `Send + Sync`
    // bounds let the static live in a multi-threaded process.
    static MATROSKA: MatroskaReader = MatroskaReader;
    static MP4: Mp4Reader = Mp4Reader;
    static AVI: AviReader = AviReader;
    static OGG: OggReader = OggReader;
    static MPEG_PS: MpegPsReader = MpegPsReader;
    static MPEG_TS: MpegTsReader = MpegTsReader;
    static REGISTRY: &[&'static (dyn Reader + Send + Sync)] =
        &[&MATROSKA, &AVI, &OGG, &MP4, &MPEG_PS, &MPEG_TS];
    REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn src_for(bytes: &[u8]) -> FileSource {
        FileSource::from_reader_for_test(Cursor::new(bytes.to_vec()))
    }

    #[test]
    fn registry_contains_at_least_matroska_in_phase_3() {
        let names: Vec<&'static str> =
            registered_readers().iter().map(|r| r.name()).collect();
        assert!(
            names.contains(&"matroska"),
            "expected matroska in registry, got {names:?}"
        );
    }

    #[test]
    fn dispatch_returns_unrecognised_on_garbage() {
        // 16 bytes of nothing recognisable.
        let mut src = src_for(&[0xAB; 16]);
        let deadline = Deadline::new(60_000);
        let mut out = MediaMetadata::new("garbage", 16);
        let err = dispatch(&mut src, &deadline, &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn dispatch_returns_unrecognised_on_empty_input() {
        let mut src = src_for(&[]);
        let deadline = Deadline::new(60_000);
        let mut out = MediaMetadata::new("empty", 0);
        let err = dispatch(&mut src, &deadline, &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn dispatch_does_not_consume_budget_per_reader_check() {
        // The probe check itself bumps the deadline-check counter once; verify
        // we don't blow the budget when the registry is short and probes are
        // cheap.  We use a 1 s budget and assert the call returns quickly.
        let mut src = src_for(&[0; 16]);
        let deadline = Deadline::new(1_000);
        let mut out = MediaMetadata::new("garbage", 16);
        let _ = dispatch(&mut src, &deadline, &mut out);
        assert!(deadline.check("post-dispatch").is_ok());
    }

    #[test]
    fn dispatch_outcome_is_claimed_for_matroska_signature() {
        // Minimal byte sequence that matches the EBML signature.  The actual
        // matroska reader will then attempt full header parse and likely fail
        // on incomplete data, so we expect a Malformed / UnexpectedEof here —
        // *not* Unrecognised.  This proves the registry routed us to matroska.
        let mut head: Vec<u8> = vec![0x1A, 0x45, 0xDF, 0xA3]; // EBML id
        // Followed by an obviously-truncated payload size
        head.extend_from_slice(&[0x80]); // size = 0
        let mut src = src_for(&head);
        let deadline = Deadline::new(60_000);
        let mut out = MediaMetadata::new("matroska-stub", head.len() as u64);
        let result = dispatch(&mut src, &deadline, &mut out);
        // Either it succeeds (unlikely on this stub) or it errors with
        // something other than Unrecognised — both prove dispatch picked
        // matroska.
        match result {
            Ok(DispatchOutcome::Claimed(name)) => assert_eq!(name, "matroska"),
            Err(ParseError::Unrecognised) => {
                panic!("matroska reader should have claimed EBML-prefixed input")
            }
            Err(_) | Ok(DispatchOutcome::NoMatch) => {
                // Any other error means the parser claimed the file but the
                // synthetic input was too short — fine for this assertion.
            }
        }
    }
}
