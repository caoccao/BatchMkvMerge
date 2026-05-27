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

//! Shared text-encoding detection for the subtitle readers.
//!
//! We use [`encoding_rs::Encoding::for_bom`] for BOM-anchored detection
//! (UTF-8 / UTF-16 LE / UTF-16 BE).  For BOM-less files we consult the
//! per-parse subtitle-charset hint pushed by `parse()` (PARSER-089) before
//! falling back to UTF-8.

use std::cell::RefCell;

use encoding_rs::Encoding;

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

thread_local! {
    /// Per-thread subtitle charset hint, set by `parse()` for the duration
    /// of a single call.  Empty string means "auto".
    static SUBTITLE_CHARSET_HINT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Set the subtitle charset hint for the current thread.  Returns the previous
/// value so callers can restore it on exit (`parse()` does this around its
/// dispatch call).
pub fn set_subtitle_charset_hint(label: String) -> String {
  SUBTITLE_CHARSET_HINT.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), label))
}

fn lookup_hint() -> Option<&'static Encoding> {
  SUBTITLE_CHARSET_HINT.with(|cell| {
    let s = cell.borrow();
    if s.is_empty() {
      None
    } else {
      Encoding::for_label(s.as_bytes())
    }
  })
}

/// Detected text encoding + byte offset where the decoded payload begins
/// (the BOM is stripped from the returned start).
#[derive(Debug, Clone, Copy)]
pub struct DetectedEncoding {
  pub label: &'static str,
  pub bom_length: usize,
}

/// Sniff the BOM at the start of `bytes`.  Falls back to the per-thread
/// charset hint if one was set, otherwise to UTF-8 (PARSER-089).
pub fn detect(bytes: &[u8]) -> DetectedEncoding {
  if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
    let label: &'static str = match enc.name() {
      "UTF-8" => "UTF-8",
      "UTF-16LE" => "UTF-16 LE",
      "UTF-16BE" => "UTF-16 BE",
      _ => "UTF-8",
    };
    return DetectedEncoding {
      label,
      bom_length: bom_len,
    };
  }
  if let Some(enc) = lookup_hint() {
    return DetectedEncoding {
      label: enc.name(),
      bom_length: 0,
    };
  }
  DetectedEncoding {
    label: "UTF-8",
    bom_length: 0,
  }
}

/// Decode a probe slice into a `Cow<str>` for line-prefix sniffing.  We
/// always run the decoder so callers don't have to special-case UTF-16.
pub fn decode_lossy(bytes: &[u8]) -> String {
  if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
    let (decoded, _, _) = enc.decode(&bytes[bom_len..]);
    return decoded.into_owned();
  }
  let encoding = lookup_hint().unwrap_or(encoding_rs::UTF_8);
  let (decoded, _, _) = encoding.decode(bytes);
  decoded.into_owned()
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProbeTextEncoding {
  Utf8Like,
  Utf8Bom,
  Utf16Le,
  Utf16Be,
}

impl ProbeTextEncoding {
  pub(crate) fn detect_from_source(src: &mut FileSource) -> Result<Self, ParseError> {
    src.seek_to(0)?;
    let mut prefix = [0u8; 3];
    let read = src.read_at_most(&mut prefix)?;
    let encoding = Self::from_prefix(&prefix[..read]);
    src.seek_to(encoding.bom_length() as u64)?;
    Ok(encoding)
  }

  fn from_prefix(prefix: &[u8]) -> Self {
    if prefix.starts_with(&[0xEF, 0xBB, 0xBF]) {
      Self::Utf8Bom
    } else if prefix.starts_with(&[0xFF, 0xFE]) {
      Self::Utf16Le
    } else if prefix.starts_with(&[0xFE, 0xFF]) {
      Self::Utf16Be
    } else {
      Self::Utf8Like
    }
  }

  pub(crate) fn bom_length(self) -> usize {
    match self {
      Self::Utf8Like => 0,
      Self::Utf8Bom => 3,
      Self::Utf16Le | Self::Utf16Be => 2,
    }
  }
}

/// Read one decoded line with mkvtoolnix-style `mm_text_io_c::getline(max)`
/// semantics: the BOM is expected to have been skipped already, line endings
/// are consumed but not returned, and reaching `max_chars` leaves the rest of
/// the physical line for the next call.
pub(crate) fn read_bounded_text_line(
  src: &mut FileSource,
  encoding: ProbeTextEncoding,
  max_chars: usize,
) -> Result<Option<String>, ParseError> {
  let mut line = String::new();
  let mut chars_read = 0usize;
  let mut previous_was_cr = false;

  loop {
    let previous_pos = src.position();
    let Some(ch) = read_probe_char(src, encoding)? else {
      return if line.is_empty() { Ok(None) } else { Ok(Some(line)) };
    };

    if ch == '\r' {
      previous_was_cr = true;
      continue;
    }
    if ch == '\n' {
      return Ok(Some(line));
    }
    if previous_was_cr {
      src.seek_to(previous_pos)?;
      return Ok(Some(line));
    }

    previous_was_cr = false;
    line.push(ch);
    chars_read += 1;
    if chars_read >= max_chars {
      return Ok(Some(line));
    }
  }
}

fn read_probe_char(src: &mut FileSource, encoding: ProbeTextEncoding) -> Result<Option<char>, ParseError> {
  match encoding {
    ProbeTextEncoding::Utf8Like | ProbeTextEncoding::Utf8Bom => read_utf8_like_char(src),
    ProbeTextEncoding::Utf16Le => read_utf16_char(src, true),
    ProbeTextEncoding::Utf16Be => read_utf16_char(src, false),
  }
}

fn read_utf8_like_char(src: &mut FileSource) -> Result<Option<char>, ParseError> {
  let mut first = [0u8; 1];
  if src.read_at_most(&mut first)? == 0 {
    return Ok(None);
  }
  let expected = match first[0] {
    0x00..=0x7F => 1,
    0xC2..=0xDF => 2,
    0xE0..=0xEF => 3,
    0xF0..=0xF4 => 4,
    _ => return Ok(Some(char::REPLACEMENT_CHARACTER)),
  };
  let mut bytes = vec![first[0]];
  while bytes.len() < expected {
    let mut next = [0u8; 1];
    if src.read_at_most(&mut next)? == 0 {
      return Ok(Some(char::REPLACEMENT_CHARACTER));
    }
    bytes.push(next[0]);
  }
  Ok(std::str::from_utf8(&bytes)
    .ok()
    .and_then(|s| s.chars().next())
    .or(Some(char::REPLACEMENT_CHARACTER)))
}

fn read_utf16_char(src: &mut FileSource, little_endian: bool) -> Result<Option<char>, ParseError> {
  let Some(first) = read_utf16_unit(src, little_endian)? else {
    return Ok(None);
  };
  if !(0xD800..=0xDBFF).contains(&first) {
    return Ok(Some(
      char::decode_utf16([first])
        .next()
        .and_then(Result::ok)
        .unwrap_or(char::REPLACEMENT_CHARACTER),
    ));
  }
  let Some(second) = read_utf16_unit(src, little_endian)? else {
    return Ok(Some(char::REPLACEMENT_CHARACTER));
  };
  Ok(Some(
    char::decode_utf16([first, second])
      .next()
      .and_then(Result::ok)
      .unwrap_or(char::REPLACEMENT_CHARACTER),
  ))
}

fn read_utf16_unit(src: &mut FileSource, little_endian: bool) -> Result<Option<u16>, ParseError> {
  let mut bytes = [0u8; 2];
  let read = src.read_at_most(&mut bytes)?;
  if read == 0 {
    return Ok(None);
  }
  if read < 2 {
    return Ok(Some(0xFFFD));
  }
  Ok(Some(if little_endian {
    u16::from_le_bytes(bytes)
  } else {
    u16::from_be_bytes(bytes)
  }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn detects_utf8_bom() {
    let bytes = [0xEFu8, 0xBB, 0xBF, b'a', b'b'];
    let d = detect(&bytes);
    assert_eq!(d.label, "UTF-8");
    assert_eq!(d.bom_length, 3);
  }

  #[test]
  fn detects_utf16_le_bom() {
    let bytes = [0xFFu8, 0xFE, b'a', 0];
    let d = detect(&bytes);
    assert_eq!(d.label, "UTF-16 LE");
    assert_eq!(d.bom_length, 2);
  }

  #[test]
  fn detects_utf16_be_bom() {
    let bytes = [0xFEu8, 0xFF, 0, b'a'];
    let d = detect(&bytes);
    assert_eq!(d.label, "UTF-16 BE");
    assert_eq!(d.bom_length, 2);
  }

  #[test]
  fn no_bom_defaults_to_utf8() {
    let bytes = b"plain ascii";
    let d = detect(bytes);
    assert_eq!(d.label, "UTF-8");
    assert_eq!(d.bom_length, 0);
  }

  #[test]
  fn decode_lossy_handles_utf16_le() {
    // "Hi" in UTF-16 LE with BOM: FF FE 48 00 69 00
    let bytes = [0xFFu8, 0xFE, b'H', 0, b'i', 0];
    let decoded = decode_lossy(&bytes);
    assert_eq!(decoded, "Hi");
  }

  #[test]
  fn decode_lossy_passes_through_utf8() {
    assert_eq!(decode_lossy(b"hello"), "hello");
  }

  #[test]
  fn decode_lossy_handles_utf8_bom() {
    let bytes = [0xEFu8, 0xBB, 0xBF, b'h', b'i'];
    assert_eq!(decode_lossy(&bytes), "hi");
  }

  #[test]
  fn decode_lossy_replaces_invalid_utf8_bytes() {
    let bytes = [b'a', 0xFF, b'b'];
    let decoded = decode_lossy(&bytes);
    assert!(decoded.starts_with('a'));
    assert!(decoded.ends_with('b'));
  }

  // ---- PARSER-089: configurable subtitle charset --------------------

  #[test]
  fn hint_overrides_default_for_bom_less_text() {
    // Pre-test cleanup in case a parallel test set the hint.
    let prev = set_subtitle_charset_hint("windows-1252".to_string());
    // "café" in Windows-1252 — 0xE9 is `é`.
    let bytes = [b'c', b'a', b'f', 0xE9];
    let d = detect(&bytes);
    assert_eq!(d.label, "windows-1252");
    let decoded = decode_lossy(&bytes);
    assert_eq!(decoded, "café");
    set_subtitle_charset_hint(prev);
  }

  #[test]
  fn hint_ignored_when_bom_is_present() {
    let prev = set_subtitle_charset_hint("windows-1252".to_string());
    let bytes = [0xEFu8, 0xBB, 0xBF, b'a'];
    let d = detect(&bytes);
    assert_eq!(d.label, "UTF-8");
    set_subtitle_charset_hint(prev);
  }

  #[test]
  fn empty_hint_keeps_utf8_default() {
    let prev = set_subtitle_charset_hint(String::new());
    let d = detect(b"plain");
    assert_eq!(d.label, "UTF-8");
    set_subtitle_charset_hint(prev);
  }

  #[test]
  fn bounded_line_leaves_remainder_after_character_cap() {
    let mut src = FileSource::from_reader_for_test(Cursor::new(b"12345\nnext\n".to_vec()));
    let enc = ProbeTextEncoding::detect_from_source(&mut src).unwrap();
    assert_eq!(read_bounded_text_line(&mut src, enc, 3).unwrap().as_deref(), Some("123"));
    assert_eq!(read_bounded_text_line(&mut src, enc, 10).unwrap().as_deref(), Some("45"));
  }

  #[test]
  fn bounded_line_skips_utf16_bom_and_counts_units() {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "12345\n".encode_utf16() {
      bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let mut src = FileSource::from_reader_for_test(Cursor::new(bytes));
    let enc = ProbeTextEncoding::detect_from_source(&mut src).unwrap();
    assert_eq!(read_bounded_text_line(&mut src, enc, 3).unwrap().as_deref(), Some("123"));
    assert_eq!(read_bounded_text_line(&mut src, enc, 10).unwrap().as_deref(), Some("45"));
  }
}
