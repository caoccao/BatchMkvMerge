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
  /// The config's defaults: the documented 10 s budget
  /// ([`crate::config::ConfigParser::DEFAULT_TIMEOUT_MS`]), a 16 MiB element
  /// cap and no subtitle charset, so a caller without a config (a test, a
  /// tool) parses as the app does by default.
  fn default() -> Self {
    Self {
      timeout_ms: crate::config::ConfigParser::DEFAULT_TIMEOUT_MS,
      max_element_size: 16 * 1024 * 1024,
      subtitle_charset: String::new(),
    }
  }
}

/// Public entry point.  Opens `path`, builds a `FileSource`, runs the
/// path-aware openers (Blu-ray playlists, VobSub `.sub` → `.idx`) and then the
/// probe cascade of every registered reader (`probe::registered_readers`),
/// and returns a populated `MediaMetadata` on success. A file no reader
/// claims returns `Err(ParseError::Unrecognised)`; a reader that panics
/// returns `Err(ParseError::Internal)`.
pub fn parse<P: AsRef<Path>>(path: P, options: ParseOptions) -> Result<MediaMetadata, ParseError> {
  let path_ref = path.as_ref();
  let mut src = FileSource::open(path_ref)?;
  let file_name = path_ref.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
  let file_size = src.length().unwrap_or(0);
  let deadline = Deadline::new(options.timeout_ms).with_max_element_size(options.max_element_size);
  let mut metadata = MediaMetadata::new(file_name, file_size);
  // PARSER-089: install the subtitle-charset hint for the duration of
  // this parse so the encoding helper can consult it when no BOM is
  // present.  Restored on exit (including the error path and a panic).
  let previous_hint = subtitles::encoding::set_subtitle_charset_hint(options.subtitle_charset.clone());
  let result = contain_panics(|| parse_with_extension_fallback(&mut src, &deadline, &mut metadata, path_ref));
  subtitles::encoding::set_subtitle_charset_hint(previous_hint);
  result?;
  Ok(metadata)
}

/// Runs a parse step and turns a panic in it into [`ParseError::Internal`]:
/// the readers walk untrusted bytes, and one malformed file must fail on its
/// own instead of unwinding into the caller's thread (the app's background
/// executor). The panic hook still sees the panic first.
fn contain_panics(parse: impl FnOnce() -> Result<(), ParseError>) -> Result<(), ParseError> {
  std::panic::catch_unwind(std::panic::AssertUnwindSafe(parse)).unwrap_or_else(|payload| {
    let message = payload
      .downcast_ref::<&str>()
      .map(|message| (*message).to_owned())
      .or_else(|| payload.downcast_ref::<String>().cloned())
      .unwrap_or_else(|| "unknown cause".to_owned());
    Err(ParseError::Internal {
      reason: format!("the parser panicked: {message}"),
    })
  })
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

  // PARSER-142 / PARSER-369: Blu-ray playlists are opened as MPEG-TS
  // playlists before the normal probe cascade for every input, mirroring
  // `reader_detection_and_creation.cpp:97-107` →
  // `mm_mpls_multi_file_io_c::open_multi`.
  if mpls::try_open(src, path, deadline, metadata)? {
    return Ok(());
  }

  // PARSER-210: VobSub is resolved by *path* before the content cascade so a
  // `.sub` input maps to the sibling `.idx` and opens the `.sub` data file,
  // mirroring mkvtoolnix's `vobsub_reader_c::probe_file` accepting both
  // `.idx`/`.sub` extensions and always resolving the `.idx`
  // (r_vobsub.cpp:82-100).  `try_open_by_path` only claims when the resolved
  // `.idx` exists and carries the VobSub banner, so non-VobSub `.sub` files
  // (e.g. MicroDVD text) fall through to the normal cascade unchanged.
  if is_vobsub_candidate_path(path) && subtitles::vobsub::try_open_by_path(path, deadline, metadata)? {
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
      if accept_empty_text_subtitle_by_extension(src, metadata, path)? {
        Ok(())
      } else {
        Err(ParseError::Unrecognised)
      }
    }
    Err(other) => Err(other),
  }
}

/// True for paths whose extension makes them a VobSub candidate: `.idx`
/// (the canonical manifest extension) or `.sub` (the data file, which mkvmerge
/// also accepts as a VobSub input and resolves to the sibling `.idx`).
fn is_vobsub_candidate_path(path: &Path) -> bool {
  path
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| e.eq_ignore_ascii_case("idx") || e.eq_ignore_ascii_case("sub"))
    .unwrap_or(false)
}

fn accept_empty_text_subtitle_by_extension(
  src: &mut FileSource,
  metadata: &mut MediaMetadata,
  path: &Path,
) -> Result<bool, ParseError> {
  use probe::extension_hint::{FileTypeHint, hints_for_path};
  let hints = hints_for_path(path);
  if hints.contains(&FileTypeHint::Srt) {
    let payload_size = metadata.file_size.saturating_sub(subtitle_bom_length(src)? as u64);
    if payload_size > 1 {
      return Ok(false);
    }
    subtitles::srt::populate_empty_srt(metadata);
    return Ok(true);
  }
  Ok(false)
}

fn subtitle_bom_length(src: &mut FileSource) -> Result<usize, ParseError> {
  let pos = src.position();
  src.seek_to(0)?;
  let mut prefix = [0u8; 3];
  let read = src.read_at_most(&mut prefix)?;
  src.seek_to(pos)?;
  Ok(subtitles::encoding::detect(&prefix[..read]).bom_length)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_panicking_reader_fails_the_file_and_restores_the_charset_hint() {
    let outer = subtitles::encoding::set_subtitle_charset_hint("windows-1250".to_owned());
    let previous = subtitles::encoding::set_subtitle_charset_hint("windows-1252".to_owned());
    let result = contain_panics(|| {
      // An offset read from a file, past the end of what was read.
      let bytes = [0u8; 4];
      let offset = std::hint::black_box(7usize);
      let _ = bytes[offset];
      Ok(())
    });
    subtitles::encoding::set_subtitle_charset_hint(previous);
    match result {
      Err(ParseError::Internal { reason }) => {
        assert!(
          reason.starts_with("the parser panicked: index out of bounds"),
          "{reason}"
        )
      }
      other => panic!("expected Internal, got {other:?}"),
    }
    let payload = crate::protocol::MediaMetadataErrorPayload::from_parse_error(&ParseError::Internal {
      reason: "the parser panicked: boom".to_owned(),
    });
    assert!(matches!(
      payload,
      crate::protocol::MediaMetadataErrorPayload::Internal { detail } if detail == "the parser panicked: boom"
    ));
    assert_eq!(
      subtitles::encoding::set_subtitle_charset_hint(outer),
      "windows-1250",
      "the hint around the parse is back"
    );
    assert!(contain_panics(|| Ok(())).is_ok());
  }

  #[test]
  fn default_options_are_the_config_default_and_sixteen_mib() {
    let opts = ParseOptions::default();
    assert_eq!(opts.timeout_ms, crate::config::ConfigParser::DEFAULT_TIMEOUT_MS);
    assert_eq!(opts.timeout_ms, 10_000);
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

  #[test]
  fn bom_only_srt_file_is_accepted_by_extension() {
    for (label, bytes) in [
      ("utf8", vec![0xEFu8, 0xBB, 0xBF]),
      ("utf16le", vec![0xFFu8, 0xFE]),
      ("utf16be", vec![0xFEu8, 0xFF]),
    ] {
      let dir = std::env::temp_dir();
      let path = dir.join(format!("bmm-bom-only-srt-{label}-{}.srt", std::process::id()));
      std::fs::write(&path, bytes).unwrap();
      let result = parse(&path, ParseOptions::default());
      let _ = std::fs::remove_file(&path);
      let m = result.unwrap();
      assert_eq!(
        m.container.format,
        crate::media_metadata::model::container::ContainerFormat::Srt
      );
      assert_eq!(m.tracks.len(), 1);
    }
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
    let playlist_dir = bdmv.join("PLAYLIST");
    let mpls_path = playlist_dir.join("00000.mpls");
    let renamed_path = playlist_dir.join("renamed-playlist.bin");
    std::fs::write(&mpls_path, &mpls).unwrap();
    std::fs::write(&renamed_path, &mpls).unwrap();

    let result = parse(&mpls_path, ParseOptions::default());
    let renamed_result = parse(&renamed_path, ParseOptions::default());
    let _ = std::fs::remove_dir_all(&root);

    let m = result.unwrap();
    assert!(m.container.recognized);
    assert_eq!(m.container.format, ContainerFormat::MpegTs);
    let pl = m.container.properties.playlist.unwrap();
    assert_eq!(pl.files.len(), 1);
    assert_eq!(pl.duration.unwrap().ns, 1_000_000_000);

    let renamed = renamed_result.unwrap();
    assert!(renamed.container.recognized);
    assert_eq!(renamed.container.format, ContainerFormat::MpegTs);
    assert_eq!(renamed.container.properties.playlist.unwrap().files.len(), 1);
  }

  #[test]
  fn renamed_vobsub_manifest_without_vobsub_extension_is_unrecognised() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("bmm-renamed-vobsub-{}.txt", std::process::id()));
    std::fs::write(
      &path,
      b"# VobSub index file, v7\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 0\n",
    )
    .unwrap();
    let result = parse(&path, ParseOptions::default());
    let _ = std::fs::remove_file(&path);
    assert!(matches!(result, Err(ParseError::Unrecognised)));
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
