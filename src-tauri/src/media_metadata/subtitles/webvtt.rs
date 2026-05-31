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

//! WebVTT reader.
//!
//! PARSER-196: mkvtoolnix's probe (`r_webvtt.cpp:24-27`) only checks whether the
//! first line *starts with* `WEBVTT` — `m_in->getline(100).find("WEBVTT") == 0` —
//! with no requirement on the following character.  We mirror that exactly rather
//! than enforcing the stricter W3C "WEBVTT followed by newline/tab/space" rule.
//!
//! PARSER-197: mkvtoolnix parses the whole file (`r_webvtt.cpp:35-44`) and takes
//! codec-private from `mtx::webvtt::parser_c::get_codec_private()`
//! (`common/webvtt.cpp:149-153`), which returns the blank-line-separated *global
//! blocks* — every block before the first cue, joined with `\n\n`.  We reproduce
//! that block model and read well past the probe window so long `STYLE` / `REGION`
//! / `NOTE` headers are not truncated.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

use super::{encoding, read_source_to_end};

/// Lightweight probe window — only the first line is needed to claim the file.
const PROBE_BYTES: usize = 1024;

/// PARSER-196: claim the file when its first line starts with `WEBVTT`, matching
/// `r_webvtt.cpp:24-27` (`getline(100).find("WEBVTT") == 0`).  The BOM is stripped
/// by the caller's decode step, so `text` here is already BOM-free.
pub fn looks_like_webvtt(text: &str) -> bool {
  let first_line = text.split(['\n', '\r']).next().unwrap_or("");
  first_line.starts_with("WEBVTT")
}

/// True when `line` is a WebVTT cue timestamp line, mirroring mkvtoolnix's
/// `timestamp_line_re` in `common/webvtt.cpp:34`:
/// `^[ \t]*TS[ \t]+-->[ \t]+TS(?:[ \t]+settings)?$` where
/// `TS = (?:\d+:)?\d{2}:\d{2}\.\d{3}`.
fn is_timestamp_line(line: &str) -> bool {
  let rest = line.trim_start_matches([' ', '\t']);
  let arrow = match rest.find("-->") {
    Some(i) => i,
    None => return false,
  };
  let lhs = &rest[..arrow];
  let after = &rest[arrow + 3..];
  // The `-->` must be separated by at least one space/tab on each side.
  if !lhs.ends_with([' ', '\t']) || !after.starts_with([' ', '\t']) {
    return false;
  }
  if !is_timestamp(lhs.trim_end_matches([' ', '\t'])) {
    return false;
  }
  // Right side: a timestamp optionally followed by whitespace + settings.
  let after = after.trim_start_matches([' ', '\t']);
  let end_ts = after.find([' ', '\t']).map(|i| &after[..i]).unwrap_or(after);
  is_timestamp(end_ts)
}

/// Matches `(?:\d+:)?\d{2}:\d{2}\.\d{3}` — an optional `H+:` group, then
/// `MM:SS.mmm` with fixed widths.
fn is_timestamp(s: &str) -> bool {
  // Split off the optional leading `\d+:` group.
  let core = match s.rsplit_once(':') {
    Some((head, ss_mmm)) => {
      // `head` is `MM` or `H+:MM`; verify the optional hours prefix.
      match head.rsplit_once(':') {
        Some((hours, mm)) => {
          if hours.is_empty() || !hours.bytes().all(|b| b.is_ascii_digit()) {
            return false;
          }
          format!("{mm}:{ss_mmm}")
        }
        None => format!("{head}:{ss_mmm}"),
      }
    }
    None => return false,
  };
  // `core` must now be exactly `MM:SS.mmm`.
  let (mm, rest) = match core.split_once(':') {
    Some(parts) => parts,
    None => return false,
  };
  let (ss, mmm) = match rest.split_once('.') {
    Some(parts) => parts,
    None => return false,
  };
  mm.len() == 2
    && ss.len() == 2
    && mmm.len() == 3
    && mm.bytes().all(|b| b.is_ascii_digit())
    && ss.bytes().all(|b| b.is_ascii_digit())
    && mmm.bytes().all(|b| b.is_ascii_digit())
}

/// PARSER-197: reproduce `mtx::webvtt::parser_c::get_codec_private()`.
///
/// The parser (`common/webvtt.cpp`) splits the chomped, newline-normalised text
/// into blocks delimited by empty lines.  A block is a cue when its first line —
/// or, for multi-line blocks, its second line (a labelled cue) — is a timestamp
/// line; the first such block ends the global section.  Every non-cue block seen
/// before then (the `WEBVTT` block plus any `STYLE` / `REGION` / `NOTE` blocks) is
/// a *global block*; `get_codec_private()` returns them joined with `\n\n`.
pub fn codec_private_header(text: &str) -> Option<String> {
  let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
  let mut global_blocks: Vec<String> = Vec::new();
  let mut current_block: Vec<&str> = Vec::new();

  // Mirrors `add_block`: classify the accumulated block, returning `true` once a
  // cue block has been reached (i.e. global-data parsing is over).
  let flush_block = |block: &mut Vec<&str>, globals: &mut Vec<String>| -> bool {
    if block.is_empty() {
      return false;
    }
    let is_cue = if is_timestamp_line(block[0]) {
      true
    } else if block.len() <= 1 {
      false
    } else {
      is_timestamp_line(block[1])
    };
    if is_cue {
      block.clear();
      return true;
    }
    globals.push(block.join("\n"));
    block.clear();
    false
  };

  for raw in normalised.split('\n') {
    // `chomp` strips trailing whitespace from each line.
    let line = raw.trim_end();
    if line.is_empty() {
      if flush_block(&mut current_block, &mut global_blocks) {
        break;
      }
    } else {
      current_block.push(line);
    }
  }
  // `flush()` adds the trailing block (only relevant when no cue followed).
  if !current_block.is_empty() {
    flush_block(&mut current_block, &mut global_blocks);
  }

  if global_blocks.is_empty() {
    return None;
  }
  Some(global_blocks.join("\n\n"))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WebVttReader;

impl Reader for WebVttReader {
  fn name(&self) -> &'static str {
    "webvtt"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read > 0 && looks_like_webvtt(&encoding::decode_lossy(&buf[..read])))
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    // PARSER-197: mkvtoolnix parses the entire file before extracting
    // codec-private, so read the full text instead of stopping at a fixed
    // header window.
    let buf = read_source_to_end(src, Some(deadline), "webvtt::headers")?;
    let text = encoding::decode_lossy(&buf);
    if !looks_like_webvtt(&text) {
      return Err(ParseError::Unrecognised);
    }
    let private = codec_private_header(&text);

    out.container.format = ContainerFormat::Webvtt;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_TEXT/WEBVTT".to_string(),
        name: Some("WebVTT".to_string()),
        codec_private: private
          .as_deref()
          .map(|header| CodecPrivate::from_bytes(header.as_bytes())),
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: true,
          // PARSER-310: mkvtoolnix normalises WebVTT text before packetising,
          // so identification always reports UTF-8 regardless of the source
          // file's BOM or configured charset hint.
          encoding: Some("UTF-8".to_string()),
          variant: Some("WebVTT".to_string()),
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
  fn looks_like_webvtt_accepts_canonical_signature() {
    assert!(looks_like_webvtt("WEBVTT\n"));
    assert!(looks_like_webvtt("WEBVTT\r\n"));
    assert!(looks_like_webvtt("WEBVTT title"));
    assert!(looks_like_webvtt("WEBVTT\tcomment"));
  }

  #[test]
  fn looks_like_webvtt_accepts_webvtt_followed_by_non_whitespace() {
    // PARSER-196: mkvtoolnix only checks `first_line.starts_with("WEBVTT")`
    // (`r_webvtt.cpp:24-27`), so a non-whitespace char right after must still
    // probe true — the old strict W3C rule rejected these.
    assert!(looks_like_webvtt("WEBVTTX"));
    assert!(looks_like_webvtt("WEBVTT-extra\nrest"));
  }

  #[test]
  fn looks_like_webvtt_rejects_other_prefixes() {
    // The signature is case-sensitive and must be at the very start of the
    // first line, mirroring `find("WEBVTT") == 0`.
    assert!(!looks_like_webvtt("webvtt\n"));
    assert!(!looks_like_webvtt("HELLO"));
    assert!(!looks_like_webvtt(" WEBVTT"));
    assert!(!looks_like_webvtt("xWEBVTT"));
  }

  #[test]
  fn probe_accepts_webvtt_blob() {
    let blob = b"WEBVTT\n\n00:00.000 --> 00:02.000\nHello\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_webvtt_immediately_followed_by_text() {
    // PARSER-196: the file `WEBVTTsomething` (no whitespace after the magic)
    // is claimed by mkvtoolnix and now by us too.
    let blob = b"WEBVTTextra header text\n\n00:00.000 --> 00:01.000\nHi\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_webvtt_track() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"WEBVTT\n\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Webvtt);
    assert_eq!(out.tracks[0].codec.id, "S_TEXT/WEBVTT");
  }

  #[test]
  fn read_headers_reports_utf8_encoding_after_source_normalisation() {
    use crate::media_metadata::deadline::Deadline;
    // UTF-16 LE with BOM: "WEBVTT\n\n".
    let blob = [
      0xFFu8, 0xFE, b'W', 0, b'E', 0, b'B', 0, b'V', 0, b'T', 0, b'T', 0, b'\n', 0, b'\n', 0,
    ];
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let sub = out.tracks[0].properties.subtitle.as_ref().unwrap();
    assert_eq!(sub.encoding.as_deref(), Some("UTF-8"));
  }

  #[test]
  fn codec_private_is_the_webvtt_block_only_when_no_other_global_blocks() {
    // PARSER-197: mkvtoolnix's `get_codec_private()` joins all global blocks,
    // and the leading `WEBVTT` block IS a global block, so a minimal file's
    // codec-private is exactly "WEBVTT".
    assert_eq!(
      codec_private_header("WEBVTT\n\n00:00.000 --> 00:01.000\nHi\n").as_deref(),
      Some("WEBVTT")
    );
  }

  #[test]
  fn codec_private_joins_global_blocks_with_blank_line() {
    // Global blocks (WEBVTT header, STYLE, NOTE) before the first cue are
    // joined with "\n\n", reproducing `join(global_blocks, "\n\n")`.
    let text = "WEBVTT - Some title\n\nNOTE a comment\n\nSTYLE\n::cue { color: lime }\n\n00:00.000 --> 00:01.000\nHi\n";
    let private = codec_private_header(text).unwrap();
    assert_eq!(
      private,
      "WEBVTT - Some title\n\nNOTE a comment\n\nSTYLE\n::cue { color: lime }"
    );
  }

  #[test]
  fn codec_private_stops_at_a_labelled_cue() {
    // A two-line block whose second line is the timestamp is a labelled cue,
    // so it ends the global section (mirrors `add_block` timestamp_line == 1).
    let text = "WEBVTT\n\ncue-label\n00:00.000 --> 00:01.000\nHi\n";
    assert_eq!(codec_private_header(text).as_deref(), Some("WEBVTT"));
  }

  #[test]
  fn read_headers_preserves_style_region_header() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"WEBVTT\n\nSTYLE\n::cue { color: lime }\n\n00:00.000 --> 00:01.000\nHi\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(
      out.tracks[0]
        .codec
        .codec_private
        .as_ref()
        .unwrap()
        .hex
        .contains("5354594c45")
    );
  }

  #[test]
  fn read_headers_captures_header_beyond_one_kib() {
    use crate::media_metadata::deadline::Deadline;
    // PARSER-197: build a STYLE block whose contents exceed 1 KiB before the
    // first cue.  The old 1 KiB cap truncated it; now the whole header is kept.
    let mut blob = String::from("WEBVTT\n\nSTYLE\n");
    // A unique sentinel placed past the 1 KiB mark within the STYLE block.
    let filler = "::cue(.line) { color: rgb(255, 255, 255) }\n".repeat(64);
    blob.push_str(&filler);
    blob.push_str("::cue(.sentinel) { color: rebeccapurple }\n");
    let header_len = blob.len();
    assert!(header_len > 1024, "header must exceed the old 1 KiB cap");
    blob.push_str("\n00:00.000 --> 00:01.000\nHi\n");

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.into_bytes()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    // The sentinel hex ("sentinel" = 73 65 6e 74 69 6e 65 6c) must survive.
    assert!(
      private.hex.contains("73656e74696e656c"),
      "codec-private should include the STYLE content past 1 KiB"
    );
    assert!(
      private.length as usize > 1024,
      "codec-private must exceed the old 1 KiB cap (was {})",
      private.length
    );
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_utf8_bom_prefix() {
    let mut blob = vec![0xEFu8, 0xBB, 0xBF];
    blob.extend_from_slice(b"WEBVTT\n");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn is_timestamp_line_matches_webvtt_cue_shapes() {
    assert!(is_timestamp_line("00:00.000 --> 00:01.000"));
    assert!(is_timestamp_line("  01:02:03.456 --> 01:02:04.000"));
    assert!(is_timestamp_line("00:00.000 --> 00:01.000 line:0 position:20%"));
    assert!(is_timestamp_line("00:00.000\t-->\t00:01.000"));
    // Not timestamp lines.
    assert!(!is_timestamp_line("WEBVTT"));
    assert!(!is_timestamp_line("STYLE"));
    assert!(!is_timestamp_line("00:00.000 00:01.000"));
    assert!(!is_timestamp_line("0:0.0 --> 0:0.0"));
    assert!(!is_timestamp_line("00:00.000-->00:01.000"));
  }
}
