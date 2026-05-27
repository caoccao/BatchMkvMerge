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

use super::deadline::Deadline;
use super::error::ParseError;
use super::io::FileSource;
use super::model::MediaMetadata;

/// Implemented once per container/codec family. The probe cascade
/// ([`crate::media_metadata::probe::dispatch`]) walks every registered reader
/// in priority order, calling `probe` on each. The first reader whose
/// `probe` returns `Ok(true)` owns the file and is asked to `read_headers`.
///
/// Implementors are pure types (unit structs); no instance state.
pub trait Reader {
  /// Stable label used in error messages and the probe registry. Lower-case
  /// snake_case, matching the module name (e.g. `"matroska"`, `"mp4"`).
  fn name(&self) -> &'static str;

  /// Cheap signature/magic probe. Reads at most a few kilobytes near the
  /// start of the file. Must rewind the cursor before returning.
  /// `Ok(false)` is "not me — try the next reader". `Ok(true)` is "claim".
  /// `Err(_)` is fatal (e.g. true I/O failure).
  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError>;

  /// Deadline-aware probe hook for readers whose probe does real parsing.
  /// Most readers only sniff a tiny magic prefix, so the default delegates to
  /// [`probe`](Self::probe).
  fn probe_with_deadline(&self, src: &mut FileSource, _deadline: &Deadline) -> Result<bool, ParseError> {
    self.probe(src)
  }

  /// Parse headers and populate `out` in-place. The cursor on entry is at
  /// offset 0; on successful return the cursor position is unspecified.
  /// Returning `Ok(())` implies a successful parse — the caller stamps the
  /// container's `recognized` / `supported` flags before yielding the
  /// metadata to the public entry point.
  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError>;
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  /// Trivial reader used to exercise the trait shape — claims any file
  /// whose first byte is `0xAB` and otherwise defers.
  struct SentinelReader;
  impl Reader for SentinelReader {
    fn name(&self) -> &'static str {
      "sentinel"
    }
    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
      let mut byte = [0u8; 1];
      let read = src.read_at_most(&mut byte)?;
      src.seek_to(0)?;
      Ok(read == 1 && byte[0] == 0xAB)
    }
    fn read_headers(&self, _src: &mut FileSource, _d: &Deadline, _out: &mut MediaMetadata) -> Result<(), ParseError> {
      Ok(())
    }
  }

  #[test]
  fn trait_can_be_implemented_and_called_dyn() {
    let r: &dyn Reader = &SentinelReader;
    assert_eq!(r.name(), "sentinel");
  }

  #[test]
  fn probe_returns_true_on_matching_signature() {
    let bytes = vec![0xAB, 0x00, 0x00];
    let mut src = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(SentinelReader.probe(&mut src).unwrap());
    // probe must rewind
    assert_eq!(src.position(), 0);
  }

  #[test]
  fn probe_returns_false_on_other_byte() {
    let bytes = vec![0x00, 0xAB];
    let mut src = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!SentinelReader.probe(&mut src).unwrap());
  }

  #[test]
  fn probe_returns_false_on_empty_input() {
    let mut src = FileSource::from_reader_for_test(Cursor::new(Vec::<u8>::new()));
    assert!(!SentinelReader.probe(&mut src).unwrap());
  }

  #[test]
  fn read_headers_can_consult_deadline() {
    let bytes = vec![0xAB];
    let mut src = FileSource::from_reader_for_test(Cursor::new(bytes));
    let d = Deadline::new(60_000);
    let mut out = MediaMetadata::new("synthetic", 1);
    SentinelReader.read_headers(&mut src, &d, &mut out).unwrap();
  }
}
