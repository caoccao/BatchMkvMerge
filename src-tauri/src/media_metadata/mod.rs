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

// `unsafe` is forbidden throughout the parser sub-tree.
#![forbid(unsafe_code)]

pub mod audio;
pub mod avi;
pub mod codec;
pub mod coreaudio;
pub mod deadline;
pub mod elementary;
pub mod error;
pub mod flv;
pub mod io;
pub mod ivf;
pub mod language;
pub mod matroska;
pub mod model;
pub mod mp4;
pub mod mpeg_ps;
pub mod mpeg_ts;
pub mod mpls;
pub mod ogg;
pub mod probe;
pub mod reader;
pub mod realmedia;
pub mod subtitles;

pub use deadline::Deadline;
pub use error::ParseError;
pub use model::{MediaMetadata, PARSER_PROTOCOL_VERSION};
pub use reader::Reader;

use std::path::Path;

use crate::media_metadata::io::file_source::FileSource;

/// Tuning knobs for a single parse call. Built per-invocation from the user's
/// persisted config; never global. See [[feedback-parser-timeout]].
#[derive(Debug, Clone)]
pub struct ParseOptions {
  pub timeout_ms: u64,
  pub max_element_size: u64,
  /// Fallback charset name used by text subtitle readers when the source
  /// carries no BOM.  Empty string means "auto" (the readers keep using
  /// UTF-8 lossy decode).  Mirrors mkvtoolnix's `--sub-charset` knob —
  /// PARSER-089.  See [`subtitle_charset`].
  pub subtitle_charset: String,
}

impl Default for ParseOptions {
  fn default() -> Self {
    Self {
      timeout_ms: 1000,
      max_element_size: 16 * 1024 * 1024,
      subtitle_charset: String::new(),
    }
  }
}

/// Public entry point.  Opens `path`, builds a `FileSource`, runs the probe
/// cascade and returns a populated `MediaMetadata` on success.
///
/// As of Phase 3 the Matroska reader is the only registered format reader.
/// Files of types whose reader has not yet landed return
/// `Err(ParseError::Unrecognised)`.
pub fn parse<P: AsRef<Path>>(path: P, options: ParseOptions) -> Result<MediaMetadata, ParseError> {
  let path_ref = path.as_ref();
  let mut src = FileSource::open(path_ref)?;
  let file_name = path_ref.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
  let file_size = src.length().unwrap_or(0);
  let deadline = Deadline::new(options.timeout_ms).with_max_element_size(options.max_element_size);
  let mut metadata = MediaMetadata::new(file_name, file_size);
  // PARSER-089: install the subtitle-charset hint for the duration of
  // this parse so the encoding helper can consult it when no BOM is
  // present.  Restored on exit (including the error path).
  let previous_hint = subtitles::encoding::set_subtitle_charset_hint(options.subtitle_charset.clone());
  let result = parse_with_extension_fallback(&mut src, &deadline, &mut metadata, path_ref);
  subtitles::encoding::set_subtitle_charset_hint(previous_hint);
  result?;
  Ok(metadata)
}

fn parse_with_extension_fallback(
  src: &mut FileSource,
  deadline: &Deadline,
  metadata: &mut MediaMetadata,
  path: &Path,
) -> Result<(), ParseError> {
  // PARSER-062: feed extension hints into the dispatcher so ambiguous
  // formats (`.mp4`, `.ogg`, `.m4a`, ...) are tried in the order the
  // extension implies — mirrors mkvtoolnix's
  // `reader_detection_and_creation.cpp:302-310` extension-hinted phase.
  let hints = probe::extension_hint::hints_for_path(path);

  // PARSER-142: Blu-ray `.mpls` playlists are opened as MPEG-TS playlists
  // before the normal probe cascade, mirroring
  // `reader_detection_and_creation.cpp:97-107` →
  // `mm_mpls_multi_file_io_c::open_multi`.
  if hints.contains(&probe::extension_hint::FileTypeHint::BlurayPlaylist)
    && mpls::try_open(src, path, deadline, metadata)?
  {
    return Ok(());
  }

  match probe::dispatch::dispatch_with_hints(src, deadline, metadata, &hints) {
    Ok(_) => Ok(()),
    Err(ParseError::Unrecognised) => {
      // PARSER-088: mkvtoolnix accepts text-subtitle files of size <= 1
      // when the extension matches (`reader_detection_and_creation.cpp`
      // §210-237).  Mirror the SRT branch here — without it an empty
      // `.srt` file is reported as unrecognised even though mkvmerge
      // would happily mux it as an empty subtitle track.
      if accept_empty_text_subtitle_by_extension(metadata, path)? {
        Ok(())
      } else {
        Err(ParseError::Unrecognised)
      }
    }
    Err(other) => Err(other),
  }
}

fn accept_empty_text_subtitle_by_extension(metadata: &mut MediaMetadata, path: &Path) -> Result<bool, ParseError> {
  use probe::extension_hint::{FileTypeHint, hints_for_path};
  if metadata.file_size > 1 {
    return Ok(false);
  }
  let hints = hints_for_path(path);
  if hints.contains(&FileTypeHint::Srt) {
    subtitles::srt::populate_empty_srt(metadata);
    return Ok(true);
  }
  Ok(false)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_options_are_one_second_and_sixteen_mib() {
    let opts = ParseOptions::default();
    assert_eq!(opts.timeout_ms, 1000);
    assert_eq!(opts.max_element_size, 16 * 1024 * 1024);
  }

  #[test]
  fn parse_returns_io_error_when_file_missing() {
    let err = parse("does-not-exist-12345.mkv", ParseOptions::default()).unwrap_err();
    assert!(matches!(err, ParseError::Io { .. }));
  }

  // ---- PARSER-088: empty .srt accepted by extension ----------------

  #[test]
  fn empty_srt_file_is_accepted_by_extension() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("bmm-empty-srt-{}.srt", std::process::id()));
    std::fs::write(&path, b"").unwrap();
    let result = parse(&path, ParseOptions::default());
    let _ = std::fs::remove_file(&path);
    let m = result.unwrap();
    assert_eq!(
      m.container.format,
      crate::media_metadata::model::container::ContainerFormat::Srt
    );
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(
      m.tracks[0].track_type,
      crate::media_metadata::model::track::TrackType::Subtitles
    );
  }

  // ---- PARSER-142: .mpls playlist parsed end-to-end -------------------

  #[test]
  fn mpls_playlist_is_recognised_through_parse() {
    use crate::media_metadata::model::container::ContainerFormat;
    // Minimal one-item MPLS (clip 00001, 0..45000 ticks = 1 s), no chapters.
    let mpls = {
      let mut playlist = Vec::new();
      playlist.extend(0u32.to_be_bytes());
      playlist.extend(0u16.to_be_bytes());
      playlist.extend(1u16.to_be_bytes());
      playlist.extend(0u16.to_be_bytes());
      let mut item = Vec::new();
      item.extend(b"00001");
      item.extend(b"M2TS");
      item.extend([0u8; 3]);
      item.extend(0u32.to_be_bytes());
      item.extend(45_000u32.to_be_bytes());
      item.extend([0u8; 12]); // UO mask + flags
      item.extend([0u8; 4]); // STN length + reserved
      item.extend([0u8; 12]); // STN: 7 count bytes (no streams) + 5 reserved
      let mut framed = (item.len() as u16).to_be_bytes().to_vec();
      framed.extend(item);
      playlist.extend(framed);
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
    };

    let root = std::env::temp_dir().join(format!("bmm-mpls-parse-{}", std::process::id()));
    let bdmv = root.join("BDMV");
    std::fs::create_dir_all(bdmv.join("PLAYLIST")).unwrap();
    std::fs::create_dir_all(bdmv.join("STREAM")).unwrap();
    std::fs::write(bdmv.join("index.bdmv"), b"INDX0200").unwrap();
    std::fs::write(bdmv.join("STREAM").join("00001.m2ts"), [0u8; 64]).unwrap();
    let mpls_path = bdmv.join("PLAYLIST").join("00000.mpls");
    std::fs::write(&mpls_path, &mpls).unwrap();

    let result = parse(&mpls_path, ParseOptions::default());
    let _ = std::fs::remove_dir_all(&root);

    let m = result.unwrap();
    assert!(m.container.recognized);
    assert_eq!(m.container.format, ContainerFormat::MpegTs);
    let pl = m.container.properties.playlist.unwrap();
    assert_eq!(pl.files.len(), 1);
    assert_eq!(pl.duration.unwrap().ns, 1_000_000_000);
  }

  #[test]
  fn empty_non_subtitle_extension_is_still_unrecognised() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("bmm-empty-other-{}.mkv", std::process::id()));
    std::fs::write(&path, b"").unwrap();
    let result = parse(&path, ParseOptions::default());
    let _ = std::fs::remove_file(&path);
    assert!(matches!(result, Err(ParseError::Unrecognised)));
  }
}
