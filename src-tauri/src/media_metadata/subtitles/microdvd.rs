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

//! MicroDVD subtitle reader.
//!
//! Lines have the shape `{startFrame}{endFrame}text`, e.g.
//! `{1}{125}Hello world|second line`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 16 * 1024;

pub fn looks_like_microdvd_line(line: &str) -> bool {
  let bytes = line.as_bytes();
  if bytes.first().copied() != Some(b'{') {
    return false;
  }
  let close_a = match bytes.iter().position(|&b| b == b'}') {
    Some(i) => i,
    None => return false,
  };
  if close_a < 2 {
    return false;
  }
  if !bytes[1..close_a].iter().all(|b| b.is_ascii_digit()) {
    return false;
  }
  let rest = &bytes[close_a + 1..];
  if rest.first().copied() != Some(b'{') {
    return false;
  }
  let close_b = match rest.iter().position(|&b| b == b'}') {
    Some(i) => i,
    None => return false,
  };
  if close_b < 2 {
    return false;
  }
  if !rest[1..close_b].iter().all(|b| b.is_ascii_digit()) {
    return false;
  }
  // mkvtoolnix's `r_microdvd.cpp` regex (`^\{\d+?\}\{\d+?\}.+$`) requires
  // at least one character of content after the second `}`.
  !rest[close_b + 1..].is_empty()
}

pub fn has_microdvd_line(text: &str) -> bool {
  text.lines().any(|l| looks_like_microdvd_line(l.trim_start()))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MicroDvdReader;

impl Reader for MicroDvdReader {
  fn name(&self) -> &'static str {
    "microdvd"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read > 0 && has_microdvd_line(&encoding::decode_lossy(&buf[..read])))
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    let text = encoding::decode_lossy(&buf[..read]);
    if !has_microdvd_line(&text) {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::MicroDvd;
    out.container.recognized = true;
    out.container.supported = false;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn looks_like_microdvd_line_accepts_canonical_form() {
    assert!(looks_like_microdvd_line("{1}{125}Hello"));
    assert!(looks_like_microdvd_line("{42}{99}Some text|line two"));
  }

  #[test]
  fn looks_like_microdvd_line_rejects_other_shapes() {
    assert!(!looks_like_microdvd_line("Hello world"));
    assert!(!looks_like_microdvd_line("{1}Hello"));
    assert!(!looks_like_microdvd_line("{a}{b}text"));
    assert!(!looks_like_microdvd_line("{}{}text"));
  }

  #[test]
  fn looks_like_microdvd_line_requires_text_after_brace_pair() {
    assert!(!looks_like_microdvd_line("{1}{2}"));
  }

  #[test]
  fn probe_accepts_minimal_microdvd_blob() {
    let blob = b"{1}{125}Hello\n{126}{250}World\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(MicroDvdReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_marks_microdvd_unsupported_without_tracks() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"{1}{125}Hello\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.sub", 0);
    MicroDvdReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::MicroDvd);
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!MicroDvdReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_skips_leading_whitespace() {
    let blob = b"   {1}{125}Hello\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(MicroDvdReader.probe(&mut s).unwrap());
  }
}
