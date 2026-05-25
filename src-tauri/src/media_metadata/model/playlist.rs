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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::duration::DurationValue;

/// Which STN group an MPLS playlist stream came from.  Mirrors the three
/// stream vectors retained on mkvtoolnix's `stn_t`
/// (`mkvtoolnix/src/common/bluray/mpls.h:103-108`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PlaylistStreamKind {
  #[default]
  Video,
  Audio,
  PresentationGraphics,
}

/// One STN-table stream of a Blu-ray playlist.  Mirrors mkvtoolnix's
/// `stream_t` (`mkvtoolnix/src/common/bluray/mpls.h:94-101`), parsed in
/// `mkvtoolnix/src/common/bluray/mpls.cpp:391-447`.  Surfaced so that the STN
/// coding type, format/rate, character code, and sub-path/sub-clip linkage are
/// no longer thrown away after parsing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistStream {
  /// Which STN group the stream belongs to (video / audio / PG).
  pub kind: PlaylistStreamKind,
  /// STN `stream_entry` stream_type (1 = play item, 2/3 = sub-path).
  pub stream_type: u8,
  /// BD stream coding type (0x02 MPEG-2, 0x1b AVC, 0x80 LPCM, 0x90 PGS,
  /// 0x92 TextST, ...).
  pub coding_type: u8,
  /// Elementary stream PID inside the referenced clip.
  pub pid: u16,
  /// Sub-path id (set only for stream_type 2/3).
  pub sub_path_id: u8,
  /// Sub-clip id (set only for stream_type 2).
  pub sub_clip_id: u8,
  /// video_format / audio_format nibble as parsed.
  pub format: u8,
  /// frame_rate / sample_rate nibble as parsed.
  pub rate: u8,
  /// Text-subtitle character code (set only for TextST coding type).
  pub character_code: u8,
  /// ISO 639-2 alpha-3 language, when the coding type carries one.
  pub language: Option<String>,
}

/// Blu-ray / multi-file playlist metadata.  Mirrors mkvmerge's
/// `playlist*` identification fields (sourced from
/// `mm_mpls_multi_file_io.cpp`).  Populated only when the input is a
/// `.mpls` playlist that demultiplexes to several `.m2ts` segment files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInfo {
  /// Total duration across all segment files in the playlist.
  pub duration: Option<DurationValue>,
  /// Number of chapter entries declared by the playlist.
  pub chapters: u32,
  /// Sum of segment-file sizes (bytes).
  #[specta(type = Number)]
  pub total_size: u64,
  /// File-system paths of the segment files in playback order.
  pub files: Vec<String>,
  /// PARSER-182: the STN-table streams the playlist declares (video, then
  /// audio, then PG), de-duplicated by PID across play items.  Mirrors the
  /// stream objects mkvtoolnix retains on `stn_t`
  /// (`mkvtoolnix/src/common/bluray/mpls.h:94-149`).
  pub streams: Vec<PlaylistStream>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_is_empty_playlist() {
    let p = PlaylistInfo::default();
    assert!(p.duration.is_none());
    assert_eq!(p.chapters, 0);
    assert_eq!(p.total_size, 0);
    assert!(p.files.is_empty());
    assert!(p.streams.is_empty());
  }

  #[test]
  fn round_trips_through_json() {
    let p = PlaylistInfo {
      duration: Some(DurationValue::from_ns(7_200_000_000_000)),
      chapters: 12,
      total_size: 24_000_000_000,
      files: vec!["00001.m2ts".to_owned(), "00002.m2ts".to_owned()],
      streams: vec![
        PlaylistStream {
          kind: PlaylistStreamKind::Video,
          stream_type: 1,
          coding_type: 0x1b,
          pid: 0x1011,
          sub_path_id: 0,
          sub_clip_id: 0,
          format: 6,
          rate: 1,
          character_code: 0,
          language: None,
        },
        PlaylistStream {
          kind: PlaylistStreamKind::Audio,
          stream_type: 1,
          coding_type: 0x81,
          pid: 0x1100,
          sub_path_id: 0,
          sub_clip_id: 0,
          format: 1,
          rate: 1,
          character_code: 0,
          language: Some("eng".to_owned()),
        },
      ],
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"chapters\":12"));
    assert!(s.contains("\"totalSize\":24000000000"));
    assert!(s.contains("\"files\":[\"00001.m2ts\",\"00002.m2ts\"]"));
    assert!(s.contains("\"kind\":\"video\""));
    assert!(s.contains("\"kind\":\"audio\""));
    assert!(s.contains("\"codingType\":129"));
    assert!(s.contains("\"characterCode\":0"));
    let back: PlaylistInfo = serde_json::from_str(&s).unwrap();
    assert_eq!(back, p);
  }
}
