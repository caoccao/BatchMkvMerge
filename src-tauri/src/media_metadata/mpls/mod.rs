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

//! Blu-ray `.mpls` playlist support.  Port of the playlist branch of
//! `mkvtoolnix/src/merge/reader_detection_and_creation.cpp` +
//! `mm_mpls_multi_file_io_c::open_multi`.
//!
//! A `.mpls` file references a chain of `STREAM/<clip>.m2ts` segments. We parse
//! the playlist ([`parser`]), resolve the referenced segment files relative to
//! the Blu-ray base directory (the one holding `index.bdmv` + `STREAM` +
//! `PLAYLIST`), read the first segment as MPEG-TS to recover the track list,
//! and attach the playlist's duration / chapter count / segment list / total
//! size as [`PlaylistInfo`].

pub mod parser;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::FileSource;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::playlist::PlaylistInfo;
use crate::media_metadata::mpeg_ts::MpegTsReader;
use crate::media_metadata::reader::Reader;

/// Largest `.mpls` mkvtoolnix will parse (`mpls.cpp:204`).
const MAX_MPLS_SIZE: u64 = 10 * 1024 * 1024;

/// Try to open `path` as a Blu-ray playlist.  Returns `Ok(true)` when the
/// input was recognised and handled as an MPLS playlist (with at least one
/// resolvable segment file), `Ok(false)` to fall through to the normal probe
/// cascade.  Mirrors `mm_mpls_multi_file_io_c::open_multi`, which only handles
/// the input when the playlist parses and resolves to segment files.
pub fn try_open(
  src: &mut FileSource,
  path: &Path,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<bool, ParseError> {
  let len = src.length().unwrap_or(0);
  if !(20..=MAX_MPLS_SIZE).contains(&len) {
    return Ok(false);
  }
  src.seek_to(0)?;
  let mut buf = vec![0u8; len as usize];
  let read = src.read_at_most(&mut buf)?;
  buf.truncate(read);

  let playlist = match parser::parse(&buf) {
    Ok(p) if !p.items.is_empty() => p,
    _ => return Ok(false),
  };

  let segment_files = resolve_segment_files(path, &playlist.items);
  if segment_files.is_empty() {
    // mkvtoolnix returns a null IO (input not handled) when no segment file
    // resolves — fall through so the file is reported as unrecognised.
    return Ok(false);
  }

  let total_size: u64 = segment_files
    .iter()
    .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
    .sum();

  // PARSER-155 / PARSER-156: identify the streams by reading the headers of
  // *every* play-item segment, plus any text-subtitle-presentation sub-path
  // clips, merging in tracks that first appear in a later file (keyed on PID).
  // mkvtoolnix's `read_headers_for_file` runs once per file before merging the
  // probed tracks (`r_mpeg_ts.cpp:1532-1535`).
  let mut sources: Vec<PathBuf> = segment_files.clone();
  sources.extend(resolve_sub_path_files(path, &playlist));

  let mut seen_pids: HashSet<u32> = HashSet::new();
  let mut primary_set = false;
  for source in &sources {
    let Some(seg) = read_segment(source, deadline) else {
      continue;
    };
    if !primary_set {
      adopt_primary(out, seg, &mut seen_pids);
      primary_set = true;
    } else {
      merge_new_tracks(out, &seg, &mut seen_pids);
    }
  }

  if !primary_set {
    out.container.format = ContainerFormat::MpegTs;
  }

  // PARSER-157: apply the playlist STN languages to tracks (matched by PID)
  // that the TS headers did not already supply one for.
  apply_stn_languages(out, &playlist);

  out.container.recognized = true;
  out.container.supported = true;

  let duration = DurationValue::from_ns(playlist.duration_ns.max(0) as u64);
  if out.container.properties.duration.is_none() {
    out.container.properties.duration = Some(duration.clone());
  }
  out.container.properties.playlist = Some(PlaylistInfo {
    duration: Some(duration),
    chapters: playlist.chapter_count,
    total_size,
    files: segment_files.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
  });
  Ok(true)
}

/// Read one `.m2ts` segment's headers into a fresh [`MediaMetadata`].  Returns
/// `None` when the file is missing, empty, not MPEG-TS, or fails to parse — a
/// single bad segment must not sink the whole playlist.
fn read_segment(path: &Path, deadline: &Deadline) -> Option<MediaMetadata> {
  let mut src = FileSource::open(path).ok()?;
  if src.length().unwrap_or(0) == 0 || !MpegTsReader.probe(&mut src).unwrap_or(false) {
    return None;
  }
  src.seek_to(0).ok()?;
  let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
  let mut seg = MediaMetadata::new(&name, src.length().unwrap_or(0));
  MpegTsReader.read_headers(&mut src, deadline, &mut seg).ok()?;
  Some(seg)
}

/// Adopt the first successfully-read segment as the container baseline: copy
/// its format, programs, fragmentation flag and tracks into `out`.
fn adopt_primary(out: &mut MediaMetadata, seg: MediaMetadata, seen_pids: &mut HashSet<u32>) {
  out.container.format = seg.container.format;
  out.container.properties.programs = seg.container.properties.programs;
  out.container.properties.is_fragmented = seg.container.properties.is_fragmented;
  out.tracks = seg.tracks;
  for track in &out.tracks {
    if let Some(pid) = track.properties.common.stream_id {
      seen_pids.insert(pid);
    }
  }
}

/// Merge tracks from a later segment / sub-path clip that introduce a PID not
/// already present, re-numbering ids compactly so they stay contiguous.
fn merge_new_tracks(out: &mut MediaMetadata, seg: &MediaMetadata, seen_pids: &mut HashSet<u32>) {
  for track in &seg.tracks {
    if let Some(pid) = track.properties.common.stream_id {
      if !seen_pids.insert(pid) {
        continue; // already have this PID from an earlier file
      }
    }
    let mut merged = track.clone();
    let id = out.tracks.len() as i64;
    merged.id = id;
    merged.properties.common.number = Some((id as u64) + 1);
    out.tracks.push(merged);
  }
}

/// PARSER-157: copy ISO-639 languages from the playlist STN tables onto tracks
/// (matched by PID) that have no language from the TS headers.
fn apply_stn_languages(out: &mut MediaMetadata, playlist: &parser::Playlist) {
  let mut lang_by_pid: std::collections::HashMap<u16, String> = std::collections::HashMap::new();
  for item in &playlist.items {
    for stream in item.stn.audio.iter().chain(&item.stn.pg).chain(&item.stn.video) {
      if let Some(lang) = &stream.language {
        if !lang.is_empty() {
          lang_by_pid.entry(stream.pid).or_insert_with(|| lang.clone());
        }
      }
    }
  }
  if lang_by_pid.is_empty() {
    return;
  }
  for track in &mut out.tracks {
    if track.properties.common.language.is_some() {
      continue;
    }
    let Some(pid) = track.properties.common.stream_id else {
      continue;
    };
    if let Some(lang) = lang_by_pid.get(&(pid as u16)) {
      track.properties.common.language = Some(Language::resolve(None, Some(lang), false));
    }
  }
}

/// PARSER-156: resolve the first clip of each text-subtitle-presentation
/// sub-path to an existing `STREAM/<clip>.m2ts` file (mirrors
/// `r_mpeg_ts.cpp::add_external_files_from_mpls`).
fn resolve_sub_path_files(mpls_path: &Path, playlist: &parser::Playlist) -> Vec<PathBuf> {
  let Some(base) = find_base_dir(mpls_path) else {
    return Vec::new();
  };
  let stream_dir = base.join("STREAM");
  let mut files = Vec::new();
  for sub_path in &playlist.sub_paths {
    if sub_path.sub_path_type != parser::SUB_PATH_TYPE_TEXT_SUBTITLE_PRESENTATION {
      continue;
    }
    if let Some(item) = sub_path.items.first() {
      let candidate = stream_dir.join(format!("{}.m2ts", item.clip_id));
      if candidate.is_file() {
        files.push(candidate);
      }
    }
  }
  files
}

/// Resolve each play item's clip id to an existing `STREAM/<clip>.<ext>` file
/// under the Blu-ray base directory.  Mirrors
/// `mm_mpls_multi_file_io_c::open_multi` + `mtx::bluray::find_other_file`:
/// non-existent segments are dropped.
fn resolve_segment_files(mpls_path: &Path, items: &[parser::PlayItem]) -> Vec<PathBuf> {
  let Some(base) = find_base_dir(mpls_path) else {
    return Vec::new();
  };
  let stream_dir = base.join("STREAM");
  let mut files = Vec::new();
  for item in items {
    let by_codec = stream_dir.join(format!("{}.{}", item.clip_id, item.codec_id.to_lowercase()));
    let candidate = if by_codec.is_file() {
      Some(by_codec)
    } else {
      let by_m2ts = stream_dir.join(format!("{}.m2ts", item.clip_id));
      by_m2ts.is_file().then_some(by_m2ts)
    };
    if let Some(file) = candidate {
      files.push(file);
    }
  }
  files
}

/// Walk up from the `.mpls` file looking for the Blu-ray base directory — the
/// one containing `index.bdmv`, `STREAM/`, and `PLAYLIST/`.  Port of
/// `mtx::bluray::find_base_dir_impl`.
fn find_base_dir(mpls_path: &Path) -> Option<PathBuf> {
  let mut dir = mpls_path.parent()?.to_path_buf();
  loop {
    if dir.join("index.bdmv").is_file() && dir.join("STREAM").is_dir() && dir.join("PLAYLIST").is_dir() {
      return Some(dir);
    }
    match dir.parent() {
      Some(parent) if parent != dir => dir = parent.to_path_buf(),
      _ => return None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  /// Lay down a fake BDMV tree with `index.bdmv`, the given `.mpls` under
  /// PLAYLIST, and STREAM segment files. Returns the temp dir + mpls path.
  fn build_bd_tree(mpls_bytes: &[u8], segments: &[(&str, &[u8])]) -> (PathBuf, PathBuf) {
    // A per-call atomic counter guarantees uniqueness even when two tests run
    // in parallel and read the clock within the same nanosecond.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
      "bmm-mpls-{}-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos(),
      seq
    ));
    let bdmv = root.join("BDMV");
    std::fs::create_dir_all(bdmv.join("PLAYLIST")).unwrap();
    std::fs::create_dir_all(bdmv.join("STREAM")).unwrap();
    std::fs::write(bdmv.join("index.bdmv"), b"INDX0200").unwrap();
    let mpls_path = bdmv.join("PLAYLIST").join("00000.mpls");
    std::fs::write(&mpls_path, mpls_bytes).unwrap();
    for (name, bytes) in segments {
      std::fs::write(bdmv.join("STREAM").join(name), bytes).unwrap();
    }
    (root, mpls_path)
  }

  fn build_simple_mpls() -> Vec<u8> {
    // Reuse the parser test fixture shape via its public parse() expectations:
    // two clips 00001 (1s) and 00002 (2s), no chapter marks.
    // Header + playlist + empty chapter block.
    let mut playlist = Vec::new();
    playlist.extend(0u32.to_be_bytes());
    playlist.extend(0u16.to_be_bytes());
    playlist.extend(2u16.to_be_bytes()); // list_count
    playlist.extend(0u16.to_be_bytes()); // sub_count
    for (clip, in_t, out_t) in [("00001", 0u32, 45_000u32), ("00002", 0, 90_000)] {
      let mut item = Vec::new();
      item.extend(clip.as_bytes());
      item.extend(b"M2TS");
      item.push(0x00);
      item.push(0x00);
      item.push(0x00);
      item.extend(in_t.to_be_bytes());
      item.extend(out_t.to_be_bytes());
      item.extend([0u8; 12]); // UO mask + flags
      item.extend([0u8; 4]); // STN length + reserved
      item.extend([0u8; 12]); // STN: 7 count bytes (no streams) + 5 reserved
      let mut framed = (item.len() as u16).to_be_bytes().to_vec();
      framed.extend(item);
      playlist.extend(framed);
    }
    let mut chapters = Vec::new();
    chapters.extend(0u32.to_be_bytes());
    chapters.extend(0u16.to_be_bytes());

    let playlist_pos = 40u32;
    let chapter_pos = playlist_pos + playlist.len() as u32;
    let mut buf = Vec::new();
    buf.extend(b"MPLS");
    buf.extend(b"0200");
    buf.extend(playlist_pos.to_be_bytes());
    buf.extend(chapter_pos.to_be_bytes());
    buf.extend(0u32.to_be_bytes());
    while (buf.len() as u32) < playlist_pos {
      buf.push(0);
    }
    buf.extend(playlist);
    buf.extend(chapters);
    buf
  }

  #[test]
  fn populates_playlist_info_from_bd_tree() {
    let mpls = build_simple_mpls();
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &[0u8; 64]), ("00002.m2ts", &[0u8; 128])]);

    let mut src = FileSource::from_reader_for_test(Cursor::new(mpls.clone()));
    let mut out = MediaMetadata::new("00000.mpls", mpls.len() as u64);
    let handled = try_open(&mut src, &mpls_path, &no_deadline(), &mut out).unwrap();
    let _ = std::fs::remove_dir_all(&root);

    assert!(handled);
    assert!(out.container.recognized);
    assert!(out.container.supported);
    assert_eq!(out.container.format, ContainerFormat::MpegTs);
    let pl = out.container.properties.playlist.unwrap();
    assert_eq!(pl.files.len(), 2);
    assert_eq!(pl.total_size, 64 + 128);
    assert_eq!(pl.duration.unwrap().ns, 3_000_000_000);
    assert_eq!(pl.chapters, 0);
  }

  #[test]
  fn falls_through_when_no_segment_files_resolve() {
    // Valid playlist bytes, but no BD tree on disk → not handled.
    let mpls = build_simple_mpls();
    let mut src = FileSource::from_reader_for_test(Cursor::new(mpls.clone()));
    let path = std::env::temp_dir().join("nonexistent-bd").join("00000.mpls");
    let mut out = MediaMetadata::new("00000.mpls", mpls.len() as u64);
    let handled = try_open(&mut src, &path, &no_deadline(), &mut out).unwrap();
    assert!(!handled);
  }

  #[test]
  fn falls_through_on_non_mpls_bytes() {
    let bytes = vec![0u8; 64];
    let mut src = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    let path = std::env::temp_dir().join("x.mpls");
    let mut out = MediaMetadata::new("x.mpls", bytes.len() as u64);
    assert!(!try_open(&mut src, &path, &no_deadline(), &mut out).unwrap());
  }

  /// Build a minimal single-program TS segment (PAT + PMT + padding) carrying
  /// the given `(stream_type, elementary_pid, descriptors)` streams.
  fn build_ts_segment(streams: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
    use crate::media_metadata::mpeg_ts::{packet, pat, pmt};
    let pmt_pid = 0x100u16;
    let pat_section = pat::build_section(1, &[(1, pmt_pid)]);
    let mut bytes = packet::build_packet_with_pointer(0, &pat_section);
    let pmt_section = pmt::build_section(1, pmt_pid, &[], streams);
    bytes.extend(packet::build_packet_with_pointer(pmt_pid, &pmt_section));
    for _ in 0..6 {
      bytes.extend(packet::build_packet(0x1FFF, false, &[]));
    }
    bytes
  }

  #[test]
  fn merges_tracks_across_segments() {
    // PARSER-155: an audio PID that only appears in the second segment must be
    // merged into the track list.
    let mpls = build_simple_mpls();
    let seg1 = build_ts_segment(&[(0x1B, 0x110, vec![]), (0x0F, 0x111, vec![])]);
    let seg2 = build_ts_segment(&[(0x1B, 0x110, vec![]), (0x0F, 0x112, vec![])]);
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &seg1), ("00002.m2ts", &seg2)]);

    let mut src = FileSource::from_reader_for_test(Cursor::new(mpls.clone()));
    let mut out = MediaMetadata::new("00000.mpls", mpls.len() as u64);
    let handled = try_open(&mut src, &mpls_path, &no_deadline(), &mut out).unwrap();
    let _ = std::fs::remove_dir_all(&root);

    assert!(handled);
    assert_eq!(out.tracks.len(), 3, "video + audio(seg1) + audio(seg2)");
    let pids: Vec<u32> = out.tracks.iter().filter_map(|t| t.properties.common.stream_id).collect();
    assert!(pids.contains(&0x112), "PID introduced by the second segment");
    // Ids stay compact across the merge.
    assert_eq!(out.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![0, 1, 2]);
  }

  #[test]
  fn sub_path_subtitle_clip_contributes_tracks() {
    // PARSER-156: a text-subtitle-presentation sub-path clip is read as an
    // external input and its subtitle track merged in.
    let item = parser::build_item_with_stn("00001", 0, 90_000, &[]);
    let sp = parser::build_subpath(parser::SUB_PATH_TYPE_TEXT_SUBTITLE_PRESENTATION, "00100");
    let mpls = parser::build_mpls_with(&[item], &[sp]);

    let main_seg = build_ts_segment(&[(0x1B, 0x110, vec![])]);
    let sub_seg = build_ts_segment(&[(0x90, 0x1200, vec![])]); // PGS subtitle
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &main_seg), ("00100.m2ts", &sub_seg)]);

    let mut src = FileSource::from_reader_for_test(Cursor::new(mpls.clone()));
    let mut out = MediaMetadata::new("00000.mpls", mpls.len() as u64);
    let handled = try_open(&mut src, &mpls_path, &no_deadline(), &mut out).unwrap();
    let _ = std::fs::remove_dir_all(&root);

    assert!(handled);
    let has_subtitle = out
      .tracks
      .iter()
      .any(|t| t.track_type == crate::media_metadata::model::track::TrackType::Subtitles);
    assert!(has_subtitle, "PGS subtitle from the sub-path clip merged in");
  }

  #[test]
  fn stn_language_applied_to_track_by_pid() {
    // PARSER-157: the playlist STN supplies a language the TS headers lack.
    let item = parser::build_item_with_stn("00001", 0, 90_000, &[parser::build_audio_stn_stream(0x111, b"jpn")]);
    let mpls = parser::build_mpls_with(&[item], &[]);
    // The segment's audio PID 0x111 carries no ISO-639 descriptor.
    let seg = build_ts_segment(&[(0x0F, 0x111, vec![])]);
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &seg)]);

    let mut src = FileSource::from_reader_for_test(Cursor::new(mpls.clone()));
    let mut out = MediaMetadata::new("00000.mpls", mpls.len() as u64);
    let handled = try_open(&mut src, &mpls_path, &no_deadline(), &mut out).unwrap();
    let _ = std::fs::remove_dir_all(&root);

    assert!(handled);
    let track = out.tracks.iter().find(|t| t.properties.common.stream_id == Some(0x111)).unwrap();
    assert_eq!(track.properties.common.language.as_ref().map(|l| l.iso639_2.as_str()), Some("jpn"));
  }

  #[test]
  fn find_base_dir_locates_bdmv_root() {
    let mpls = build_simple_mpls();
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &[0u8; 8])]);
    let base = find_base_dir(&mpls_path).unwrap();
    assert!(base.join("index.bdmv").is_file());
    let _ = std::fs::remove_dir_all(&root);
  }
}
