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

//! VobSub `.idx` reader.
//!
//! The `.idx` file is a UTF-8 text manifest produced by VobSub-style
//! demuxers; the sibling `.sub` blob contains the actual MPEG-PS payload.
//!
//! mkvtoolnix's `r_vobsub.cpp` recognises VobSub by the literal
//! `"# VobSub index file, v"` banner on the first line of the `.idx`.  The
//! probe accepts both `.idx` and `.sub` extensions and always resolves the
//! sibling `.idx` (`idx_and_sub_file_names`), then opens the `.sub` data file
//! during header reading.  We mirror that resolution here so dragging a `.sub`
//! file produces the same track listing as its `.idx` (PARSER-210).
//!
//! Probing claims the file on the banner alone (PARSER-233); the version number
//! is validated in `read_headers` (`require_supported_version`), which reports
//! an explicit error for missing-version and pre-v7 manifests instead of
//! letting them fall through to unrelated readers.  The path-aware entry points
//! additionally require the sibling `.sub` data file to exist (PARSER-232),
//! mirroring upstream's `m_sub_file` open that errors when the `.sub` is
//! absent.
//!
//! `parse_headers()` is ported faithfully (PARSER-211): per-`id:` track entry
//! lists with `delay:` accumulation, `timestamp: HH:MM:SS:mmm, filepos: 0xNN`
//! parsing, negative-timestamp delay correction, out-of-order sorting, and the
//! "skip tracks with zero entries" rule.  Codec-private is built from the
//! filtered global settings lines (the `id:`, `timestamp:`, `delay:`, `alt:`
//! and `langidx:` control lines are stripped), matching mkvtoolnix's
//! `idx_data`.  The `.sub` MPEG-PS payload is never demuxed — only located and
//! recorded under `container.properties.otherFiles`.

use std::path::{Path, PathBuf};

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

use super::read_source_to_end;

/// Upper bound on how much of the `.idx` manifest we decode.  These are tiny
/// text files in practice; the cap guards against a pathological input while
/// staying well above any real-world manifest.
const PROBE_BYTES: usize = 64 * 1024;
const MAGIC: &str = "# VobSub index file, v";

/// A single subtitle entry under an `id:` track (mirrors `vobsub_entry_c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VobSubEntry {
  /// Byte offset of the SPU packet inside the `.sub` data file.
  pub position: u64,
  /// Presentation timestamp in nanoseconds, after `delay` correction.
  pub timestamp: i64,
}

/// One parsed VobSub track (mirrors `vobsub_track_c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VobSubTrack {
  /// Two-/three-letter language code from the `id:` line.
  pub language: String,
  pub entries: Vec<VobSubEntry>,
}

/// Result of parsing an `.idx` manifest: the per-language tracks plus the
/// filtered codec-private text shared across every track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIdx {
  pub tracks: Vec<VobSubTrack>,
  /// `idx_data` — the global settings lines (size, palette, ...) with the
  /// `id:`, `timestamp:`, `delay:`, `alt:` and `langidx:` control lines
  /// removed.  Shared as the codec-private blob for every track.
  pub codec_private: String,
}

/// Parse a VobSub `HH:MM:SS:mmm` timestamp into nanoseconds.
///
/// VobSub uses a colon (not a decimal point) before the millisecond field.
/// mkvtoolnix's `parse_timestamp` treats the third colon as the fractional
/// separator (`parsing.cpp:154-155`), so `00:00:01:000` is one second.  We
/// accept `HH:MM:SS:mmm` and the more lenient `HH:MM:SS.mmm` / `MM:SS.mmm`
/// forms the generic parser also handles.
fn parse_idx_timestamp(src: &str) -> Option<i64> {
  let src = src.trim();
  if src.is_empty() {
    return None;
  }
  // Split the integer time fields (h/m/s) from the fractional part.  Either a
  // `.` or the *third* `:` introduces the fraction.
  let mut int_part = String::new();
  let mut frac_part = String::new();
  let mut colons = 0u32;
  let mut in_frac = false;
  for ch in src.chars() {
    if ch == '.' {
      if in_frac {
        return None;
      }
      in_frac = true;
      continue;
    }
    if ch == ':' {
      if in_frac {
        // A colon inside the fractional part is invalid.
        return None;
      }
      if colons == 2 {
        // Third colon → start of the millisecond fraction.
        in_frac = true;
        continue;
      }
      colons += 1;
      int_part.push(':');
      continue;
    }
    if !ch.is_ascii_digit() {
      return None;
    }
    if in_frac {
      frac_part.push(ch);
    } else {
      int_part.push(ch);
    }
  }

  let fields: Vec<&str> = int_part.split(':').collect();
  let (h, m, s) = match fields.as_slice() {
    [h, m, s] => (parse_u64(h)?, parse_u64(m)?, parse_u64(s)?),
    [m, s] => (0u64, parse_u64(m)?, parse_u64(s)?),
    _ => return None,
  };
  if m > 59 || s > 59 {
    return None;
  }
  // Pad / truncate the fractional digits to nanosecond precision (9 digits).
  let mut nanos: u64 = 0;
  for digit in frac_part.chars().take(9) {
    nanos = nanos * 10 + (digit as u64 - '0' as u64);
  }
  for _ in frac_part.len().min(9)..9 {
    nanos *= 10;
  }
  let total = ((h * 3600 + m * 60 + s) * 1_000_000_000 + nanos) as i64;
  Some(total)
}

fn parse_u64(s: &str) -> Option<u64> {
  if s.is_empty() {
    return None;
  }
  s.parse::<u64>().ok()
}

/// Decode a hex `filepos` string (no `0x` prefix) into a byte offset.
fn parse_filepos(src: &str) -> Option<u64> {
  let src = src.trim();
  if src.is_empty() {
    return None;
  }
  let mut value: u64 = 0;
  for ch in src.chars() {
    let digit = ch.to_digit(16)?;
    value = (value << 4) | u64::from(digit);
  }
  Some(value)
}

/// Parse the VobSub `.idx` text into per-language tracks plus the shared
/// codec-private blob.  Direct port of `vobsub_reader_c::parse_headers`
/// (`r_vobsub.cpp:193-352`).
pub fn parse_idx(text: &str) -> Result<ParsedIdx, ParseError> {
  let mut tracks: Vec<VobSubTrack> = Vec::new();
  let mut current: Option<VobSubTrack> = None;
  let mut idx_data = String::new();

  let mut delay: i64 = 0;
  let mut last_timestamp: i64 = 0;
  let mut sort_required = false;

  // Flush helper mirroring the C++ "push if non-empty, else drop" logic.
  let flush = |tracks: &mut Vec<VobSubTrack>, track: Option<VobSubTrack>, sort_required: bool| {
    if let Some(mut track) = track {
      if track.entries.is_empty() {
        // r_vobsub.cpp:219 / :331 — tracks without entries are dropped.
      } else {
        if sort_required {
          // stable_sort by timestamp (r_vobsub.cpp:225 / :337).
          track.entries.sort_by_key(|entry| entry.timestamp);
        }
        tracks.push(track);
      }
    }
  };

  for line in text.lines() {
    // r_vobsub.cpp:209 — blank lines and comments are skipped.  The banner
    // line (first `#` comment) is therefore never captured into idx_data.
    if line.is_empty() || line.starts_with('#') {
      continue;
    }

    // `id: <lang>, index: N` opens a new track (r_vobsub.cpp:212-234).
    if let Some(lang) = parse_id_line(line) {
      flush(&mut tracks, current.take(), sort_required);
      current = Some(VobSubTrack {
        language: lang,
        entries: Vec::new(),
      });
      delay = 0;
      last_timestamp = 0;
      sort_required = false;
      continue;
    }

    let lower = line.to_ascii_lowercase();

    // `alt:` / `langidx:` are control lines that never reach idx_data
    // (r_vobsub.cpp:236-237).
    if lower.starts_with("alt:") || lower.starts_with("langidx:") {
      continue;
    }

    // `delay:` accumulates into the running track delay (r_vobsub.cpp:239-253).
    if lower.starts_with("delay:") {
      let mut value = line[6..].trim();
      let mut factor: i64 = 1;
      if let Some(rest) = value.strip_prefix('-') {
        factor = -1;
        value = rest;
      }
      if let Some(ts) = parse_idx_timestamp(value) {
        delay += ts * factor;
      }
      continue;
    }

    // `timestamp: HH:MM:SS:mmm, filepos: 0xNN` is a subtitle entry
    // (r_vobsub.cpp:255-324).  Only meaningful inside an `id:` track.
    if lower.starts_with("timestamp:") {
      if current.is_none() {
        // r_vobsub.cpp:256-257 — entries before any `id:` are a hard error
        // upstream.
        return Err(ParseError::Malformed {
          format: "vobsub",
          offset: 0,
          reason: "VobSub timestamp entry encountered before any id track".to_string(),
        });
      }
      if let Some((timestamp, position)) =
        parse_timestamp_line(line, &mut delay, &mut last_timestamp, &mut sort_required)
      {
        if let Some(track) = current.as_mut() {
          track.entries.push(VobSubEntry { position, timestamp });
        }
      }
      continue;
    }

    // Everything else is a global settings line that forms idx_data.
    idx_data.push_str(line);
    idx_data.push('\n');
  }

  flush(&mut tracks, current.take(), sort_required);

  Ok(ParsedIdx {
    tracks,
    codec_private: idx_data,
  })
}

/// Parse an `id: <lang>, index: N` line, returning the language code.
fn parse_id_line(line: &str) -> Option<String> {
  // Case-insensitive `id:` prefix (r_vobsub.cpp:212 regex with
  // CaseInsensitiveOption).
  let rest = if line.len() >= 3 && line[..3].eq_ignore_ascii_case("id:") {
    line[3..].trim_start()
  } else {
    return None;
  };
  // Language is everything up to the first comma (or newline).
  let lang = match rest.split_once(',') {
    Some((l, _)) => l.trim(),
    None => rest.trim(),
  };
  if lang.is_empty() {
    return None;
  }
  Some(lang.to_ascii_lowercase())
}

/// Parse a `timestamp: ..., filepos: ...` line, applying delay correction and
/// out-of-order detection.  Returns the corrected `(timestamp, position)` or
/// `None` if the entry should be skipped.  `last_timestamp` and
/// `sort_required` are updated in place (mirrors r_vobsub.cpp:255-324).
fn parse_timestamp_line(
  line: &str,
  delay: &mut i64,
  last_timestamp: &mut i64,
  sort_required: &mut bool,
) -> Option<(i64, u64)> {
  // mkvtoolnix splits on whitespace into exactly four parts:
  //   ["timestamp:", "HH:MM:SS:mmm,", "filepos:", "0xNN"]
  let parts: Vec<&str> = line.split_whitespace().collect();
  if parts.len() != 4 || !parts[2].eq_ignore_ascii_case("filepos:") {
    return None;
  }
  // The timestamp part carries a trailing comma (r_vobsub.cpp:277).
  let ts_field = parts[1].strip_suffix(',').unwrap_or(parts[1]);
  if ts_field.len() < 12 {
    // r_vobsub.cpp:263 requires `parts[1].length() >= 13` (incl. the comma).
    return None;
  }
  let mut ts_str = ts_field;
  let mut factor: i64 = 1;
  if let Some(rest) = ts_str.strip_prefix('-') {
    factor = -1;
    ts_str = rest;
  }
  let timestamp = parse_idx_timestamp(ts_str)?;
  let position = parse_filepos(parts[3])?;

  let mut corrected = timestamp * factor + *delay;

  // r_vobsub.cpp:295-300 — when the track delay is negative and this entry
  // would land before the previous one, advance the running delay so the
  // entry is clamped forward to the previous timestamp.
  if *delay < 0 && *last_timestamp != 0 && corrected < *last_timestamp {
    *delay += *last_timestamp - corrected;
    corrected = *last_timestamp;
  }

  // r_vobsub.cpp:302-307 — entries still negative after delay are unsupported
  // in Matroska and skipped.
  if corrected < 0 {
    return None;
  }

  // r_vobsub.cpp:311-320 — out-of-order entry → flag the track for sorting.
  if corrected < *last_timestamp {
    *sort_required = true;
  }
  *last_timestamp = corrected;

  Some((corrected, position))
}

/// Resolve the sibling `.sub` (any case) next to an `.idx` path.
pub fn sibling_sub_path(idx_path: &Path) -> Option<PathBuf> {
  for ext in ["sub", "SUB", "Sub"] {
    let candidate = idx_path.with_extension(ext);
    if candidate.exists() {
      return Some(candidate);
    }
  }
  None
}

/// Resolve the `.idx` path for a VobSub input.  When handed a `.sub` file the
/// sibling `.idx` is returned; an `.idx` path is returned unchanged.  Mirrors
/// `idx_and_sub_file_names` (r_vobsub.cpp:60-69) which always resolves `.idx`.
pub fn resolve_idx_path(path: &Path) -> PathBuf {
  if path
    .extension()
    .and_then(|e| e.to_str())
    .map_or(false, |e| e.eq_ignore_ascii_case("sub"))
  {
    path.with_extension("idx")
  } else {
    path.to_path_buf()
  }
}

/// Port of `vobsub_reader_c::probe_file` (`r_vobsub.cpp:81-99`): the file is
/// claimed purely on the first-line banner.  The version number is **not**
/// checked here — upstream validates it in `read_headers` and reports an
/// explicit "v7 and newer" error for older files (PARSER-233), so the probe
/// must claim v6-and-older manifests rather than letting them fall through to
/// unrelated readers.
pub fn looks_like_vobsub_idx(text: &str) -> bool {
  let line = match text.lines().next() {
    Some(l) => l.trim_start_matches('\u{feff}'),
    None => return false,
  };
  if line.as_bytes().len() < MAGIC.len() {
    return false;
  }
  line.as_bytes()[..MAGIC.len()].eq_ignore_ascii_case(MAGIC.as_bytes())
}

/// Validate the manifest version, mirroring the checks in
/// `vobsub_reader_c::read_headers` (`r_vobsub.cpp:124-132`): a missing version
/// number and any version below 7 are hard errors (PARSER-233).
fn require_supported_version(text: &str) -> Result<u32, ParseError> {
  match idx_version(text) {
    None => Err(ParseError::Malformed {
      format: "vobsub",
      offset: 0,
      reason: "No version number found".to_string(),
    }),
    Some(version) if version < 7 => Err(ParseError::Malformed {
      format: "vobsub",
      offset: 0,
      reason: "Only v7 and newer VobSub files are supported".to_string(),
    }),
    Some(version) => Ok(version),
  }
}

/// Require the sibling `.sub` data file next to `idx_path` (PARSER-232).
/// `vobsub_reader_c::read_headers` opens both the `.idx` and the `.sub`
/// (`r_vobsub.cpp:108-115`) and errors when the `.sub` cannot be opened, so a
/// VobSub `.idx` with no readable `.sub` is a hard error rather than a
/// silently-listed track set.
fn require_sibling_sub(idx_path: &Path) -> Result<PathBuf, ParseError> {
  sibling_sub_path(idx_path).ok_or_else(|| ParseError::Io {
    offset: 0,
    source: std::io::Error::new(
      std::io::ErrorKind::NotFound,
      "VobSub .sub data file not found next to the .idx manifest",
    ),
  })
}

pub fn idx_version(text: &str) -> Option<u32> {
  let line = text.lines().next()?.trim_start_matches('\u{feff}');
  let rest = line.get(MAGIC.len()..)?;
  let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
  digits.parse().ok()
}

/// Populate `out` from the decoded `.idx` text.  Pushes one `S_VOBSUB` track
/// per non-empty `id:` entry, sharing the filtered codec-private blob.
fn populate_from_idx(text: &str, out: &mut MediaMetadata) -> Result<(), ParseError> {
  out.container.format = ContainerFormat::VobSub;
  out.container.recognized = true;
  out.container.supported = true;

  let parsed = parse_idx(text)?;
  let codec_private = CodecPrivate::from_bytes(parsed.codec_private.as_bytes());

  for (track_idx, track) in parsed.tracks.iter().enumerate() {
    let mut common = CommonTrackProperties::default();
    common.number = Some(track_idx as u64 + 1);
    common.num_index_entries = Some(track.entries.len() as u64);
    common.language = Some(Language::resolve(
      Some(track.language.as_str()),
      Some(track.language.as_str()),
      false,
    ));
    out.tracks.push(Track {
      id: track_idx as i64,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_VOBSUB".to_string(),
        name: Some("VobSub".to_string()),
        codec_private: Some(codec_private.clone()),
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: false,
          encoding: None,
          variant: Some("VobSub".to_string()),
          teletext_page: None,
        }),
        ..TrackProperties::default()
      },
    });
  }
  Ok(())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VobSubReader;

impl Reader for VobSubReader {
  fn name(&self) -> &'static str {
    "vobsub"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    if read == 0 {
      return Ok(false);
    }
    let text = String::from_utf8_lossy(&buf[..read]);
    Ok(looks_like_vobsub_idx(&text))
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    deadline.check("vobsub")?;
    let buf = read_source_to_end(src, Some(deadline), "vobsub::headers")?;
    let text = String::from_utf8_lossy(&buf);
    if !looks_like_vobsub_idx(&text) {
      return Err(ParseError::Unrecognised);
    }
    // PARSER-233: a recognised banner with an unsupported (or missing) version
    // is a hard error, not "unrecognised".  The sibling `.sub` requirement is
    // enforced only on the path-aware entry points, which know the file path.
    require_supported_version(&text)?;
    populate_from_idx(&text, out)?;
    Ok(())
  }
}

/// Parse a VobSub `.idx` from a filesystem path, resolving `.sub` inputs to the
/// sibling `.idx`.  Records the sibling `.sub` data file under
/// `container.properties.otherFiles` when present.  This is the path-aware
/// entry point used by the public `parse` dispatcher so dragging a `.sub`
/// produces the same listing as its `.idx` (PARSER-210).
pub fn parse_idx_at_path(path: &Path) -> Result<MediaMetadata, ParseError> {
  let idx_path = resolve_idx_path(path);
  let mut src = FileSource::open(&idx_path)?;
  let file_name = idx_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
  let file_size = src.length().unwrap_or(0);
  let mut metadata = MediaMetadata::new(file_name, file_size);
  // PARSER-232: the `.sub` data file must exist (mkvmerge opens it before
  // parsing the manifest body).
  let sub_path = require_sibling_sub(&idx_path)?;
  VobSubReader.read_headers(&mut src, &Deadline::new(60_000), &mut metadata)?;
  metadata
    .container
    .properties
    .other_files
    .push(sub_path.to_string_lossy().into_owned());
  Ok(metadata)
}

/// Attempt the path-aware VobSub parse for a file whose extension hints VobSub
/// (`.idx`) or that is a `.sub` with a banner-bearing sibling `.idx`.  Returns
/// `Ok(true)` and populates `metadata` when the file is VobSub; `Ok(false)`
/// when it is not (so the caller can fall through to the normal cascade).
///
/// Mirrors mkvtoolnix's probe accepting both `.idx`/`.sub` extensions and
/// always resolving the `.idx` (r_vobsub.cpp:82-100).
pub fn try_open_by_path(path: &Path, metadata: &mut MediaMetadata) -> Result<bool, ParseError> {
  let idx_path = resolve_idx_path(path);
  // Only claim when the resolved `.idx` exists and carries the banner.  This
  // keeps `.sub` files that are *not* VobSub (e.g. MicroDVD text) flowing to
  // the normal cascade.
  let mut src = match FileSource::open(&idx_path) {
    Ok(src) => src,
    Err(_) => return Ok(false),
  };
  let mut buf = vec![0u8; PROBE_BYTES];
  let read = src.read_at_most(&mut buf)?;
  src.seek_to(0)?;
  if read == 0 {
    return Ok(false);
  }
  let text = String::from_utf8_lossy(&buf[..read]);
  if !looks_like_vobsub_idx(&text) {
    return Ok(false);
  }
  // The banner confirms this is VobSub; from here mkvmerge has already claimed
  // the file in `probe_file`, so a missing `.sub` (PARSER-232) or an
  // unsupported version (PARSER-233) is a hard error, not a fall-through.
  let sub_path = require_sibling_sub(&idx_path)?;
  require_supported_version(&text)?;
  let full = read_source_to_end(&mut src, None, "vobsub::path")?;
  let full_text = String::from_utf8_lossy(&full);
  populate_from_idx(&full_text, metadata)?;
  metadata
    .container
    .properties
    .other_files
    .push(sub_path.to_string_lossy().into_owned());
  Ok(true)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;
  use std::io::Write;

  fn temp_stem(label: &str) -> PathBuf {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!("bmm-vobsub-{label}-{pid}-{nanos}-{seq}"))
  }

  #[test]
  fn looks_like_vobsub_idx_accepts_canonical_magic() {
    assert!(looks_like_vobsub_idx("# VobSub index file, v7\n"));
  }

  #[test]
  fn looks_like_vobsub_idx_accepts_old_versions_on_banner() {
    // PARSER-233: the probe claims on the banner regardless of version; the
    // version is rejected later in read_headers.
    assert!(looks_like_vobsub_idx("# VobSub index file, v6\n"));
    assert!(looks_like_vobsub_idx("# VobSub index file, v\n"));
  }

  #[test]
  fn require_supported_version_rejects_old_and_missing() {
    assert!(require_supported_version("# VobSub index file, v6\n").is_err());
    assert!(require_supported_version("# VobSub index file, v\n").is_err());
    assert_eq!(require_supported_version("# VobSub index file, v7\n").unwrap(), 7);
    assert_eq!(require_supported_version("# VobSub index file, v8\n").unwrap(), 8);
  }

  #[test]
  fn read_headers_rejects_old_version_with_explicit_error() {
    let blob = b"# VobSub index file, v6\nsize: 720x576\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.idx", 0);
    let err = VobSubReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    match err {
      ParseError::Malformed { format, reason, .. } => {
        assert_eq!(format, "vobsub");
        assert!(reason.contains("v7 and newer"), "reason was: {reason}");
      }
      other => panic!("expected Malformed, got {other:?}"),
    }
  }

  #[test]
  fn looks_like_vobsub_idx_accepts_utf8_bom() {
    assert!(looks_like_vobsub_idx("\u{feff}# VobSub index file, v7\n"));
  }

  #[test]
  fn looks_like_vobsub_idx_accepts_mixed_case() {
    assert!(looks_like_vobsub_idx("# vobsub INDEX file, V8\n"));
  }

  #[test]
  fn looks_like_vobsub_idx_rejects_other_first_lines() {
    assert!(!looks_like_vobsub_idx("# some other tool\n"));
    assert!(!looks_like_vobsub_idx("# VobSub index file\n"));
    assert!(!looks_like_vobsub_idx(""));
  }

  #[test]
  fn parse_idx_timestamp_handles_colon_milliseconds() {
    // VobSub form `HH:MM:SS:mmm`.
    assert_eq!(parse_idx_timestamp("00:00:01:000"), Some(1_000_000_000));
    assert_eq!(parse_idx_timestamp("00:00:01:500"), Some(1_500_000_000));
    assert_eq!(parse_idx_timestamp("01:02:03:004"), Some(3_723_004_000_000));
  }

  #[test]
  fn parse_idx_timestamp_handles_decimal_and_short_forms() {
    assert_eq!(parse_idx_timestamp("00:00:02.250"), Some(2_250_000_000));
    assert_eq!(parse_idx_timestamp("01:02.000"), Some(62_000_000_000));
  }

  #[test]
  fn parse_idx_timestamp_rejects_garbage() {
    assert_eq!(parse_idx_timestamp("not-a-time"), None);
    assert_eq!(parse_idx_timestamp(""), None);
    assert_eq!(parse_idx_timestamp("00:99:00:000"), None);
  }

  #[test]
  fn parse_filepos_decodes_hex() {
    assert_eq!(parse_filepos("000000000"), Some(0));
    assert_eq!(parse_filepos("0x100"), None); // a real idx never has a 0x prefix
    assert_eq!(parse_filepos("1a2b"), Some(0x1a2b));
  }

  #[test]
  fn parse_idx_emits_one_track_per_id_with_entries() {
    let txt = "\
size: 720x576
palette: 000000, ffffff
id: en, index: 0
timestamp: 00:00:01:000, filepos: 000000000
timestamp: 00:00:05:000, filepos: 000001000
id: fr, index: 1
timestamp: 00:00:02:000, filepos: 000002000
";
    let parsed = parse_idx(txt).unwrap();
    assert_eq!(parsed.tracks.len(), 2);
    assert_eq!(parsed.tracks[0].language, "en");
    assert_eq!(parsed.tracks[0].entries.len(), 2);
    assert_eq!(parsed.tracks[1].language, "fr");
    assert_eq!(parsed.tracks[1].entries.len(), 1);
  }

  #[test]
  fn parse_idx_errors_on_timestamp_before_id() {
    let txt = "timestamp: 00:00:01:000, filepos: 000000000\nid: en, index: 0\n";
    let err = parse_idx(txt).unwrap_err();
    match err {
      ParseError::Malformed { format, reason, .. } => {
        assert_eq!(format, "vobsub");
        assert!(reason.contains("before any id"), "reason was: {reason}");
      }
      other => panic!("expected Malformed, got {other:?}"),
    }
  }

  #[test]
  fn parse_idx_skips_track_without_entries() {
    let txt = "\
size: 720x576
id: en, index: 0
id: fr, index: 1
timestamp: 00:00:02:000, filepos: 000002000
";
    let parsed = parse_idx(txt).unwrap();
    // `en` has no timestamp entry and is dropped.
    assert_eq!(parsed.tracks.len(), 1);
    assert_eq!(parsed.tracks[0].language, "fr");
  }

  #[test]
  fn parse_idx_sorts_out_of_order_entries() {
    let txt = "\
id: en, index: 0
timestamp: 00:00:05:000, filepos: 000000000
timestamp: 00:00:01:000, filepos: 000001000
timestamp: 00:00:03:000, filepos: 000002000
";
    let parsed = parse_idx(txt).unwrap();
    assert_eq!(parsed.tracks.len(), 1);
    let ts: Vec<i64> = parsed.tracks[0].entries.iter().map(|e| e.timestamp).collect();
    assert_eq!(ts, vec![1_000_000_000, 3_000_000_000, 5_000_000_000]);
  }

  #[test]
  fn parse_idx_skips_negative_timestamp_entries() {
    // A negative delay pushes the first entry below zero → skipped.
    let txt = "\
id: en, index: 0
delay: -00:00:10:000
timestamp: 00:00:01:000, filepos: 000000000
timestamp: 00:00:30:000, filepos: 000001000
";
    let parsed = parse_idx(txt).unwrap();
    assert_eq!(parsed.tracks.len(), 1);
    // First entry (1s - 10s = -9s) is dropped; second (30s - 10s = 20s) kept.
    assert_eq!(parsed.tracks[0].entries.len(), 1);
    assert_eq!(parsed.tracks[0].entries[0].timestamp, 20_000_000_000);
  }

  #[test]
  fn parse_idx_codec_private_excludes_control_lines() {
    let txt = "\
# VobSub index file, v7
size: 720x576
palette: 000000, ffffff
langidx: 0
id: en, index: 0
alt: english
delay: 00:00:00:000
timestamp: 00:00:01:000, filepos: 000000000
";
    let parsed = parse_idx(txt).unwrap();
    let cp = parsed.codec_private;
    assert!(cp.contains("size: 720x576"));
    assert!(cp.contains("palette: 000000, ffffff"));
    assert!(!cp.contains("id:"));
    assert!(!cp.contains("timestamp:"));
    assert!(!cp.contains("delay:"));
    assert!(!cp.contains("alt:"));
    assert!(!cp.contains("langidx:"));
    // The banner comment is also excluded (skipped as a `#` line).
    assert!(!cp.contains("# VobSub"));
  }

  #[test]
  fn probe_accepts_magic_blob() {
    let blob = b"# VobSub index file, v7\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 0\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(VobSubReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_other_text() {
    let blob = b"WEBVTT\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(!VobSubReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_one_track_per_idx_entry() {
    let blob = b"# VobSub index file, v7\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 0\nid: fr, index: 1\ntimestamp: 00:00:02:000, filepos: 10\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.idx", 0);
    VobSubReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::VobSub);
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(out.tracks[0].properties.common.num_index_entries, Some(1));
    assert!(out.tracks[0].codec.codec_private.is_some());
    let lang0 = out.tracks[0]
      .properties
      .common
      .language
      .as_ref()
      .expect("language populated");
    assert!(lang0.ietf.as_deref() == Some("en") || lang0.iso639_2 == "eng");
    let lang1 = out.tracks[1]
      .properties
      .common
      .language
      .as_ref()
      .expect("language populated");
    assert!(lang1.ietf.as_deref() == Some("fr") || lang1.iso639_2 == "fra");
  }

  #[test]
  fn read_headers_parses_manifest_beyond_sixty_four_kib() {
    let mut blob = String::from("# VobSub index file, v7\nsize: 720x576\n");
    while blob.len() < 70 * 1024 {
      blob.push_str("palette: 000000, ffffff\n");
    }
    blob.push_str("id: en, index: 0\ntimestamp: 00:00:01:000, filepos: 000000000\n");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.into_bytes()));
    let mut out = MediaMetadata::new("clip.idx", 0);
    VobSubReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].properties.common.num_index_entries, Some(1));
  }

  #[test]
  fn read_headers_drops_tracks_without_entries() {
    let blob = b"# VobSub index file, v7\nsize: 1920x1080\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.idx", 0);
    VobSubReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn resolve_idx_path_maps_sub_to_idx() {
    let sub = Path::new("/tmp/clip.sub");
    assert_eq!(resolve_idx_path(sub), PathBuf::from("/tmp/clip.idx"));
    let sub_upper = Path::new("/tmp/clip.SUB");
    assert_eq!(resolve_idx_path(sub_upper), PathBuf::from("/tmp/clip.idx"));
    let idx = Path::new("/tmp/clip.idx");
    assert_eq!(resolve_idx_path(idx), PathBuf::from("/tmp/clip.idx"));
  }

  #[test]
  fn parse_idx_at_path_records_sibling_sub_file() {
    let stem = temp_stem("path");
    let idx_path = stem.with_extension("idx");
    let sub_path = stem.with_extension("sub");
    std::fs::File::create(&idx_path)
      .unwrap()
      .write_all(b"# VobSub index file, v7\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 0\n")
      .unwrap();
    std::fs::File::create(&sub_path).unwrap().write_all(&[0u8; 16]).unwrap();
    let m = parse_idx_at_path(&idx_path).unwrap();
    assert!(m.container.properties.other_files.iter().any(|f| f.ends_with(".sub")));
    let _ = std::fs::remove_file(&idx_path);
    let _ = std::fs::remove_file(&sub_path);
  }

  #[test]
  fn try_open_by_path_resolves_sub_to_sibling_idx() {
    let stem = temp_stem("resolve");
    let idx_path = stem.with_extension("idx");
    let sub_path = stem.with_extension("sub");
    std::fs::File::create(&idx_path)
      .unwrap()
      .write_all(
        b"# VobSub index file, v7\nsize: 720x576\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 000000000\n",
      )
      .unwrap();
    std::fs::File::create(&sub_path).unwrap().write_all(&[0u8; 16]).unwrap();

    let mut m = MediaMetadata::new("clip.sub", 16);
    // Hand the *.sub* path — it must resolve to the sibling .idx.
    let claimed = try_open_by_path(&sub_path, &mut m).unwrap();
    assert!(claimed);
    assert_eq!(m.container.format, ContainerFormat::VobSub);
    assert_eq!(m.tracks.len(), 1);
    assert!(m.container.properties.other_files.iter().any(|f| f.ends_with(".sub")));

    let _ = std::fs::remove_file(&idx_path);
    let _ = std::fs::remove_file(&sub_path);
  }

  #[test]
  fn try_open_by_path_errors_when_sub_missing() {
    // PARSER-232: a VobSub `.idx` (banner present, v7) with no sibling `.sub`
    // is a hard error — mkvmerge opens the `.sub` and errors when it cannot.
    let stem = temp_stem("nosub");
    let idx_path = stem.with_extension("idx");
    std::fs::File::create(&idx_path)
      .unwrap()
      .write_all(b"# VobSub index file, v7\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 0\n")
      .unwrap();
    let mut m = MediaMetadata::new("clip.idx", 0);
    let err = try_open_by_path(&idx_path, &mut m).unwrap_err();
    assert!(matches!(err, ParseError::Io { .. }), "expected Io error, got {err:?}");
    let _ = std::fs::remove_file(&idx_path);
  }

  #[test]
  fn try_open_by_path_declines_sub_without_idx() {
    // A `.sub` file with no sibling `.idx` is not VobSub — caller falls
    // through to the normal cascade (e.g. MicroDVD).
    let stem = temp_stem("orphan");
    let sub_path = stem.with_extension("sub");
    std::fs::File::create(&sub_path)
      .unwrap()
      .write_all(b"{1}{125}Hello\n")
      .unwrap();
    let mut m = MediaMetadata::new("clip.sub", 14);
    let claimed = try_open_by_path(&sub_path, &mut m).unwrap();
    assert!(!claimed);
    let _ = std::fs::remove_file(&sub_path);
  }

  #[test]
  fn sibling_sub_path_returns_none_when_absent() {
    let stem = temp_stem("absent");
    let idx_path = stem.with_extension("idx");
    assert!(sibling_sub_path(&idx_path).is_none());
  }
}
