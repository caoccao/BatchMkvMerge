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

//! SRT (SubRip Text) subtitle reader.
//!
//! Layout (de-facto standard — there is no formal spec):
//!
//! ```text
//! 1                                              ← index line (decimal)
//! 00:00:00,000 --> 00:00:02,500                  ← timecode line
//! Subtitle text on one or more lines.            ← payload
//! <blank line>
//! 2
//! 00:00:02,500 --> 00:00:04,000
//! ...
//! ```
//!
//! We probe for an `HH:MM:SS,mmm --> HH:MM:SS,mmm` (or `.mmm` / `:mmm`) line
//! within the first 16 KB.  The arrow and numeric fields follow mkvtoolnix's
//! flexible regex, so forms like `00:00:01,000-->00:00:02,000` and
//! `00:00:01,000 -> 00:00:02,000` are accepted too.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 16 * 1024;

/// `true` when `text` contains a line matching the SRT timecode pattern.
///
/// This scans *every* line and is used to classify an already-extracted
/// subtitle payload (e.g. AVI GAB2) whose container kind is unknown.  It is
/// deliberately *not* used for whole-file probing — see [`looks_like_srt`].
pub fn has_srt_timecode_line(text: &str) -> bool {
  for line in text.lines() {
    if looks_like_srt_timecode(line.trim()) {
      return true;
    }
  }
  false
}

/// Whole-file SRT probe — port of `srt_parser_c::probe`
/// (`../mkvtoolnix/src/input/subtitles.cpp:106-124`).
///
/// Upstream strips leading blank lines, requires the first non-empty line to
/// parse as a number (the cue index), and only then tests the *immediately
/// following* line against the timestamp regex.  Scanning the whole prefix
/// for any timestamp-shaped line (PARSER-223) let text files with an
/// incidental timestamp be misclassified as SRT.
pub fn looks_like_srt(text: &str) -> bool {
  let mut lines = text.lines();
  // Skip leading blank lines, then the first non-empty line must be a number.
  let index_line = loop {
    match lines.next() {
      Some(line) => {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
          break trimmed;
        }
      }
      None => return false,
    }
  };
  if !is_srt_index(index_line) {
    return false;
  }
  // The line immediately after the index must be an SRT timecode line.
  match lines.next() {
    Some(line) => looks_like_srt_timecode(line.trim()),
    None => false,
  }
}

/// Mirror of `mtx::string::parse_number` for the SRT cue index: the whole
/// (stripped) line must be an optionally-signed run of ASCII digits.
fn is_srt_index(line: &str) -> bool {
  let body = line
    .strip_prefix('-')
    .or_else(|| line.strip_prefix('+'))
    .unwrap_or(line);
  !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
}

/// Port of mkvtoolnix's `SRT_RE_TIMESTAMP_LINE`
/// (`../mkvtoolnix/src/input/subtitles.cpp:99-101`):
///
/// ```text
/// VALUE     := \s*(-?)\s*(\d+)
/// TIMESTAMP := VALUE ":" VALUE ":" VALUE (?:[,\.:] VALUE)?
/// LINE      := ^ TIMESTAMP \s*[\-\s]+>\s* TIMESTAMP \s*
/// ```
///
/// The pattern is anchored at the start but not the end, so trailing content
/// after the second timestamp is tolerated (mirrors upstream's
/// `QString::contains` with the `^`-anchored regex).  The arrow accepts any
/// non-empty run of dashes/spaces before `>`, so `-->`, `->`, `>` (with at
/// least one leading space) and zero-spaced forms all match.
fn looks_like_srt_timecode(line: &str) -> bool {
  let bytes = line.as_bytes();
  let pos = match parse_srt_timestamp(bytes, 0) {
    Some(p) => p,
    None => return false,
  };
  let pos = match parse_srt_arrow(bytes, pos) {
    Some(p) => p,
    None => return false,
  };
  parse_srt_timestamp(bytes, pos).is_some()
}

/// Port of `SRT_RE_VALUE` = `\s*(-?)\s*(\d+)` — leading whitespace, an
/// optional single sign, more optional whitespace, then one or more digits.
fn parse_srt_value(bytes: &[u8], mut pos: usize) -> Option<usize> {
  while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
    pos += 1;
  }
  if pos < bytes.len() && bytes[pos] == b'-' {
    pos += 1;
  }
  while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
    pos += 1;
  }
  let start = pos;
  while pos < bytes.len() && bytes[pos].is_ascii_digit() {
    pos += 1;
  }
  if pos == start { None } else { Some(pos) }
}

/// Port of `TIMESTAMP` = `VALUE : VALUE : VALUE (?:[,\.:] VALUE)?`.
fn parse_srt_timestamp(bytes: &[u8], pos: usize) -> Option<usize> {
  let pos = parse_srt_value(bytes, pos)?;
  let pos = expect_srt_byte(bytes, pos, b':')?;
  let pos = parse_srt_value(bytes, pos)?;
  let pos = expect_srt_byte(bytes, pos, b':')?;
  let pos = parse_srt_value(bytes, pos)?;
  // Optional milliseconds: `[,\.:]` then a value.  If the value does not
  // parse the optional group simply matches the empty string (upstream).
  if pos < bytes.len() && matches!(bytes[pos], b',' | b'.' | b':') {
    if let Some(after) = parse_srt_value(bytes, pos + 1) {
      return Some(after);
    }
  }
  Some(pos)
}

/// Port of `\s*[\-\s]+>\s*`.  Since `\s` is a subset of `[\-\s]`, the leading
/// `\s*` collapses into the mandatory run, so this is "≥1 dash/space, then
/// `>`, then optional whitespace".
fn parse_srt_arrow(bytes: &[u8], mut pos: usize) -> Option<usize> {
  let start = pos;
  while pos < bytes.len() && (bytes[pos] == b'-' || bytes[pos].is_ascii_whitespace()) {
    pos += 1;
  }
  if pos == start {
    return None;
  }
  if pos >= bytes.len() || bytes[pos] != b'>' {
    return None;
  }
  pos += 1;
  while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
    pos += 1;
  }
  Some(pos)
}

fn expect_srt_byte(bytes: &[u8], pos: usize, b: u8) -> Option<usize> {
  if pos < bytes.len() && bytes[pos] == b {
    Some(pos + 1)
  } else {
    None
  }
}

/// Populate `out` with an empty SRT track.  Used by the dispatch
/// extension-fallback path (PARSER-088) when an empty `.srt` file would be
/// rejected by the normal byte-signature probe — mkvtoolnix accepts these
/// based on extension alone.
pub fn populate_empty_srt(out: &mut MediaMetadata) {
  out.container.format = ContainerFormat::Srt;
  out.container.recognized = true;
  out.container.supported = true;

  let mut common = CommonTrackProperties::default();
  common.number = Some(1);
  out.tracks.push(Track {
    id: 0,
    track_type: TrackType::Subtitles,
    codec: CodecInfo {
      id: "S_TEXT/UTF8".to_string(),
      name: Some("SubRip Text".to_string()),
      codec_private: None,
    },
    properties: TrackProperties {
      common,
      subtitle: Some(SubtitleTrackProperties {
        text_subtitles: true,
        encoding: Some("UTF-8".to_string()),
        variant: Some("SRT".to_string()),
        teletext_page: None,
      }),
      ..TrackProperties::default()
    },
  });
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SrtReader;

impl Reader for SrtReader {
  fn name(&self) -> &'static str {
    "srt"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    if read == 0 {
      return Ok(false);
    }
    let text = encoding::decode_lossy(&buf[..read]);
    Ok(looks_like_srt(&text))
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
    let detected = encoding::detect(&buf[..read]);
    let text = encoding::decode_lossy(&buf[..read]);
    if !looks_like_srt(&text) {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::Srt;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_TEXT/UTF8".to_string(),
        name: Some("SubRip Text".to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: true,
          encoding: Some(detected.label.to_string()),
          variant: Some("SRT".to_string()),
          teletext_page: None,
        }),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn has_srt_timecode_line_recognises_comma_separator() {
    assert!(has_srt_timecode_line("1\n00:00:00,500 --> 00:00:02,500\nHello"));
  }

  #[test]
  fn has_srt_timecode_line_recognises_dot_separator() {
    assert!(has_srt_timecode_line("00:00:00.000 --> 00:00:02.500"));
  }

  #[test]
  fn has_srt_timecode_line_recognises_colon_separator() {
    assert!(has_srt_timecode_line("00:00:00:000 --> 00:00:02:500"));
  }

  #[test]
  fn has_srt_timecode_line_rejects_garbage() {
    assert!(!has_srt_timecode_line("just text"));
    assert!(!has_srt_timecode_line("00:00:00 -- 00:00:02"));
  }

  #[test]
  fn has_srt_timecode_line_rejects_when_only_one_side_is_timestamp() {
    assert!(!has_srt_timecode_line("hello --> 00:00:02,500"));
    assert!(!has_srt_timecode_line("00:00:02,500 --> hello"));
  }

  #[test]
  fn timecode_tolerates_long_hours() {
    assert!(looks_like_srt_timecode("100:00:00,000 --> 101:00:00,000"));
  }

  #[test]
  fn timecode_rejects_missing_colons() {
    assert!(!looks_like_srt_timecode("123456 --> 234567"));
  }

  // ---- PARSER-235: flexible arrow / numeric-field grammar --------------

  #[test]
  fn timecode_accepts_zero_spaced_arrow() {
    assert!(looks_like_srt_timecode("00:00:01,000-->00:00:02,000"));
  }

  #[test]
  fn timecode_accepts_single_dash_arrow() {
    assert!(looks_like_srt_timecode("00:00:01,000 -> 00:00:02,000"));
  }

  #[test]
  fn timecode_accepts_extra_dashes_and_spaces() {
    assert!(looks_like_srt_timecode("00:00:01,000  --->  00:00:02,000"));
    assert!(looks_like_srt_timecode("00:00:01,000 - > 00:00:02,000"));
  }

  #[test]
  fn timecode_accepts_whitespace_inside_fields() {
    // SRT_RE_VALUE allows `\s*(-?)\s*` before each number.
    assert!(looks_like_srt_timecode("00:00: 1, 000 --> 00:00: 2, 000"));
  }

  #[test]
  fn timecode_rejects_arrow_without_gt() {
    assert!(!looks_like_srt_timecode("00:00:01,000 -- 00:00:02,000"));
  }

  #[test]
  fn timecode_accepts_optional_milliseconds() {
    assert!(looks_like_srt_timecode("00:00:01 --> 00:00:02"));
  }

  #[test]
  fn timecode_tolerates_trailing_content() {
    // Anchored at start, not end — trailing coordinates etc. are ignored.
    assert!(looks_like_srt_timecode(
      "00:00:01,000 --> 00:00:02,000 X1:200 Y2:100"
    ));
  }

  #[test]
  fn probe_accepts_minimal_srt_blob() {
    let blob = b"1\r\n00:00:00,000 --> 00:00:02,500\r\nHello\r\n\r\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(SrtReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_srt_track_with_encoding() {
    use crate::media_metadata::deadline::Deadline;
    let mut blob = vec![0xEFu8, 0xBB, 0xBF];
    blob.extend_from_slice(b"1\n00:00:00,000 --> 00:00:02,500\nHello\n");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.srt", 0);
    SrtReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Srt);
    let sub = out.tracks[0].properties.subtitle.as_ref().unwrap();
    assert_eq!(sub.encoding.as_deref(), Some("UTF-8"));
    assert!(sub.text_subtitles);
  }

  #[test]
  fn probe_returns_false_on_empty_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(Vec::<u8>::new()));
    assert!(!SrtReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_returns_false_on_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
    assert!(!SrtReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-223: structural probe (index line, then timestamp) -------

  #[test]
  fn looks_like_srt_requires_index_then_timestamp() {
    assert!(looks_like_srt("1\n00:00:00,000 --> 00:00:02,500\nHello\n"));
    // Leading blank lines are skipped before the index line.
    assert!(looks_like_srt("\n\n  \n42\n00:00:01,000 --> 00:00:02,000\nHi\n"));
  }

  #[test]
  fn looks_like_srt_rejects_incidental_timestamp_line() {
    // A text file whose first non-empty line is not a cue index must be
    // rejected even though a later line *is* a clean timestamp line
    // (PARSER-223).
    let text = "Title: My Notes\n00:00:01,000 --> 00:00:02,000\nsome note\n";
    assert!(!looks_like_srt(text));
    // The whole-payload scanner still finds the timestamp — it is used for
    // already-classified subtitle payloads, not whole-file probing.
    assert!(has_srt_timecode_line(text));
  }

  #[test]
  fn looks_like_srt_rejects_index_without_following_timestamp() {
    assert!(!looks_like_srt("1\nHello there\n00:00:01,000 --> 00:00:02,000\n"));
    assert!(!looks_like_srt("1\n"));
  }

  #[test]
  fn probe_rejects_incidental_timestamp_file() {
    let text = "readme\nsee 00:00:01,000 --> 00:00:02,000 below\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(text.as_bytes().to_vec()));
    assert!(!SrtReader.probe(&mut s).unwrap());
  }

  #[test]
  fn is_srt_index_accepts_optionally_signed_digits() {
    assert!(is_srt_index("1"));
    assert!(is_srt_index("-3"));
    assert!(is_srt_index("+12"));
    assert!(!is_srt_index("1a"));
    assert!(!is_srt_index(""));
    assert!(!is_srt_index("-"));
  }
}
