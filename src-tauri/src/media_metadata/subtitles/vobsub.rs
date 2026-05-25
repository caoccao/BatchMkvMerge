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
//! mkvtoolnix's `r_vobsub.cpp` recognises `.idx` files by the literal
//! `"# VobSub index file"` magic on the first line, then parses
//! `id: XX, index: N` entries to enumerate the per-language tracks.

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

const PROBE_BYTES: usize = 64 * 1024;
const MAGIC: &str = "# VobSub index file, v";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdxEntry {
  pub language: String,
  pub index: u32,
  pub num_entries: u32,
}

/// Parse `id: xx, index: N` lines.  Language is the two-letter code that
/// precedes the comma; lines that don't match are skipped.
pub fn parse_idx_entries(text: &str) -> Vec<IdxEntry> {
  let mut out: Vec<IdxEntry> = Vec::new();
  let mut current: Option<usize> = None;
  for line in text.lines() {
    let trimmed = line.trim();
    if trimmed.starts_with("timestamp:") {
      if let Some(idx) = current {
        out[idx].num_entries += 1;
      }
      continue;
    }
    let rest = match trimmed.strip_prefix("id:") {
      Some(r) => r.trim_start(),
      None => continue,
    };
    let (lang, rest) = match rest.split_once(',') {
      Some((l, r)) => (l.trim(), r.trim_start()),
      None => continue,
    };
    if lang.is_empty() {
      continue;
    }
    let idx_str = match rest.strip_prefix("index:") {
      Some(s) => s.trim(),
      None => continue,
    };
    if let Ok(index) = idx_str.parse::<u32>() {
      out.push(IdxEntry {
        language: lang.to_string(),
        index,
        num_entries: 0,
      });
      current = Some(out.len() - 1);
    }
  }
  out.into_iter().filter(|entry| entry.num_entries > 0).collect()
}

/// Resolve the sibling `.sub` (any case) next to an `.idx` path.
pub fn sibling_sub_path(idx_path: &Path) -> Option<PathBuf> {
  // Try `.sub`, `.SUB` and `.Sub` in that order.
  for ext in ["sub", "SUB", "Sub"] {
    let candidate = idx_path.with_extension(ext);
    if candidate.exists() {
      return Some(candidate);
    }
  }
  None
}

pub fn looks_like_vobsub_idx(text: &str) -> bool {
  let line = match text.lines().next() {
    Some(l) => l.trim_start_matches('\u{feff}'),
    None => return false,
  };
  if line.as_bytes().len() < MAGIC.len() {
    return false;
  }
  line.as_bytes()[..MAGIC.len()].eq_ignore_ascii_case(MAGIC.as_bytes())
    && idx_version(text).map_or(false, |version| version >= 7)
}

pub fn idx_version(text: &str) -> Option<u32> {
  let line = text.lines().next()?.trim_start_matches('\u{feff}');
  let rest = line.get(MAGIC.len()..)?;
  let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
  digits.parse().ok()
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

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    let text = String::from_utf8_lossy(&buf[..read]);
    if !looks_like_vobsub_idx(&text) {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::VobSub;
    out.container.recognized = true;
    out.container.supported = true;

    for (track_idx, entry) in parse_idx_entries(&text).iter().enumerate() {
      let mut common = CommonTrackProperties::default();
      common.number = Some(entry.index as u64 + 1);
      common.num_index_entries = Some(entry.num_entries as u64);
      common.language = Some(Language::resolve(
        Some(entry.language.as_str()),
        Some(entry.language.as_str()),
        false,
      ));
      out.tracks.push(Track {
        id: track_idx as i64,
        track_type: TrackType::Subtitles,
        codec: CodecInfo {
          id: "S_VOBSUB".to_string(),
          name: Some("VobSub".to_string()),
          codec_private: Some(CodecPrivate::from_bytes(text.as_bytes())),
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
}

/// Parse a VobSub `.idx` from a filesystem path.  Records the sibling `.sub`
/// path under `container.properties.otherFiles` when present.
pub fn parse_idx_at_path(path: &Path) -> Result<MediaMetadata, ParseError> {
  let idx_path = if path
    .extension()
    .and_then(|e| e.to_str())
    .map_or(false, |e| e.eq_ignore_ascii_case("sub"))
  {
    path.with_extension("idx")
  } else {
    path.to_path_buf()
  };
  let mut src = FileSource::open(&idx_path)?;
  let file_name = idx_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
  let file_size = src.length().unwrap_or(0);
  let mut metadata = MediaMetadata::new(file_name, file_size);
  VobSubReader.read_headers(&mut src, &Deadline::new(60_000), &mut metadata)?;
  if let Some(sub) = sibling_sub_path(&idx_path) {
    metadata
      .container
      .properties
      .other_files
      .push(sub.to_string_lossy().into_owned());
  }
  Ok(metadata)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn looks_like_vobsub_idx_accepts_canonical_magic() {
    assert!(looks_like_vobsub_idx("# VobSub index file, v7\n"));
  }

  #[test]
  fn looks_like_vobsub_idx_rejects_old_versions() {
    assert!(!looks_like_vobsub_idx("# VobSub index file, v6\n"));
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
  fn parse_idx_entries_extracts_language_and_index() {
    let txt = "\
id: en, index: 0
timestamp: 00:00:01:000, filepos: 000000000
id: fr, index: 1
timestamp: 00:00:02:000, filepos: 000000100
id: ja, index: 2
timestamp: 00:00:03:000, filepos: 000000200
";
    let entries = parse_idx_entries(txt);
    assert_eq!(
      entries,
      vec![
        IdxEntry {
          language: "en".into(),
          index: 0,
          num_entries: 1
        },
        IdxEntry {
          language: "fr".into(),
          index: 1,
          num_entries: 1
        },
        IdxEntry {
          language: "ja".into(),
          index: 2,
          num_entries: 1
        },
      ]
    );
  }

  #[test]
  fn parse_idx_entries_skips_unrelated_lines() {
    let txt = "size: 1920x1080\nid: de, index: 5\ntimestamp: 00:00:01:000, filepos: 0\nrandom: line\n";
    let entries = parse_idx_entries(txt);
    assert_eq!(
      entries,
      vec![IdxEntry {
        language: "de".into(),
        index: 5,
        num_entries: 1
      }]
    );
  }

  #[test]
  fn parse_idx_entries_skips_malformed_index() {
    let txt = "id: en, index: notanumber\n";
    assert!(parse_idx_entries(txt).is_empty());
  }

  #[test]
  fn parse_idx_entries_skips_missing_comma() {
    let txt = "id: en index: 0\n";
    assert!(parse_idx_entries(txt).is_empty());
  }

  #[test]
  fn parse_idx_entries_skips_empty_language() {
    let txt = "id: , index: 0\n";
    assert!(parse_idx_entries(txt).is_empty());
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
  fn parse_idx_at_path_records_sibling_sub_file() {
    use std::io::Write;
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = format!("bmm-vobsub-{pid}-{nanos}-{seq}");
    let idx_path = dir.join(format!("{stem}.idx"));
    let sub_path = dir.join(format!("{stem}.sub"));
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
  fn sibling_sub_path_returns_none_when_absent() {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = format!("bmm-vobsub-orphan-{pid}-{nanos}-{seq}");
    let idx_path = dir.join(format!("{stem}.idx"));
    assert!(sibling_sub_path(&idx_path).is_none());
  }
}
