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

//! HDMV PGS (`.sup`) reader — Blu-ray graphic subtitles.
//!
//! Every PGS segment starts with the 2-byte ASCII magic `"PG"` followed by:
//!
//! ```text
//! 4 bytes  PTS (90 kHz)
//! 4 bytes  DTS (90 kHz)
//! 1 byte   segment_type (0x14 PDS, 0x15 ODS, 0x16 PCS, 0x17 WDS,
//!          0x18 ICS interactive composition, 0x80 END)
//! 2 bytes  segment_length (big-endian)
//! ...      segment payload
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 64 * 1024;
const SEGMENT_HEADER_LEN: usize = 13;
pub const MAGIC: [u8; 2] = *b"PG";

/// Walk segment headers and count well-formed ones.  Faithful to
/// `hdmv_pgs_reader_c::probe_file` (`../mkvtoolnix/src/input/r_hdmv_pgs.cpp:26-37`),
/// which only checks the `PG` magic, skips by the declared `segment_length`,
/// and verifies the next segment also starts with `PG` — it does **not**
/// validate `segment_type`.  Gating on a fixed set of segment types
/// (PARSER-219) turned valid chains carrying interactive-composition (`0x18`)
/// or future segment types near the start into false negatives, so the type
/// check has been dropped.  Returns `None` when fewer than two `PG`-magic
/// segment headers are observed within `bytes`.
pub fn count_segments(bytes: &[u8]) -> Option<usize> {
  let mut pos = 0usize;
  let mut count = 0usize;
  while pos + SEGMENT_HEADER_LEN <= bytes.len() {
    if bytes[pos..pos + 2] != MAGIC {
      break;
    }
    let seg_len = u16::from_be_bytes([bytes[pos + 11], bytes[pos + 12]]) as usize;
    pos += SEGMENT_HEADER_LEN + seg_len;
    count += 1;
  }
  if count >= 2 { Some(count) } else { None }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PgsReader;

impl Reader for PgsReader {
  fn name(&self) -> &'static str {
    "pgs"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read >= SEGMENT_HEADER_LEN && count_segments(&buf[..read]).is_some())
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
    if count_segments(&buf[..read]).is_none() {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::HdmvPgs;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_HDMV/PGS".to_string(),
        name: Some("HDMV PGS".to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: false,
          encoding: None,
          variant: Some("PGS".to_string()),
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

  fn build_segment(seg_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(SEGMENT_HEADER_LEN + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&[0u8; 4]); // PTS
    bytes.extend_from_slice(&[0u8; 4]); // DTS
    bytes.push(seg_type);
    let len = payload.len() as u16;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
  }

  fn build_two_segment_clip() -> Vec<u8> {
    let mut blob = build_segment(0x16, &[0u8; 11]);
    blob.extend(build_segment(0x17, &[0u8; 9]));
    blob
  }

  #[test]
  fn count_segments_rejects_single_segment() {
    let blob = build_segment(0x16, &[0u8; 11]);
    // mkvtoolnix requires two consecutive PG headers.
    assert!(count_segments(&blob).is_none());
  }

  #[test]
  fn count_segments_walks_multiple_segments() {
    let mut blob = build_segment(0x16, &[0u8; 11]);
    blob.extend(build_segment(0x17, &[0u8; 9]));
    blob.extend(build_segment(0x80, &[]));
    assert_eq!(count_segments(&blob), Some(3));
  }

  #[test]
  fn count_segments_rejects_wrong_magic() {
    let blob = b"XX\x00\x00\x00\x00\x00\x00\x00\x00\x16\x00\x00";
    assert!(count_segments(blob).is_none());
  }

  #[test]
  fn count_segments_accepts_interactive_composition_segment() {
    // PARSER-219: interactive-composition (0x18) and other segment types
    // near the start must not cause a false negative — upstream walks by
    // length and only checks the `PG` magic.
    let mut blob = build_segment(0x18, &[0u8; 5]);
    blob.extend(build_segment(0x16, &[0u8; 11]));
    blob.extend(build_segment(0x80, &[]));
    assert_eq!(count_segments(&blob), Some(3));
  }

  #[test]
  fn count_segments_walks_unknown_segment_type_by_length() {
    // An unknown type is followed by the declared length to the next PG
    // header, matching mkvtoolnix's probe which never inspects the type.
    let mut blob = build_segment(0x16, &[0u8; 4]);
    blob[10] = 0x42; // unknown type, but a valid PG-magic header
    blob.extend(build_segment(0x17, &[0u8; 6]));
    assert_eq!(count_segments(&blob), Some(2));
  }

  #[test]
  fn probe_accepts_blob_with_two_segments() {
    let blob = build_two_segment_clip();
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(PgsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_pgs_track() {
    let blob = build_two_segment_clip();
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.sup", 0);
    PgsReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::HdmvPgs);
    let sub = out.tracks[0].properties.subtitle.as_ref().unwrap();
    assert!(!sub.text_subtitles);
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!PgsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"PG".to_vec()));
    assert!(!PgsReader.probe(&mut s).unwrap());
  }
}
