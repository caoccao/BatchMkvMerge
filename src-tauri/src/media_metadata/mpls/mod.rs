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

use std::path::{Path, PathBuf};

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::FileSource;
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

  // Identify the segment streams by reading the first segment as MPEG-TS.
  let mut read_tracks = false;
  if let Some(first) = segment_files.first() {
    if let Ok(mut seg_src) = FileSource::open(first) {
      if seg_src.length().unwrap_or(0) > 0 && MpegTsReader.probe(&mut seg_src).unwrap_or(false) {
        seg_src.seek_to(0)?;
        if MpegTsReader.read_headers(&mut seg_src, deadline, out).is_ok() {
          read_tracks = true;
        }
      }
    }
  }
  if !read_tracks {
    out.container.format = ContainerFormat::MpegTs;
  }
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
    let root = std::env::temp_dir().join(format!(
      "bmm-mpls-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
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

  #[test]
  fn find_base_dir_locates_bdmv_root() {
    let mpls = build_simple_mpls();
    let (root, mpls_path) = build_bd_tree(&mpls, &[("00001.m2ts", &[0u8; 8])]);
    let base = find_base_dir(&mpls_path).unwrap();
    assert!(base.join("index.bdmv").is_file());
    let _ = std::fs::remove_dir_all(&root);
  }
}
