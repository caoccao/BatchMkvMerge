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

//! HDMV TextST reader.
//!
//! mkvtoolnix's `r_hdmv_textst.cpp` recognises these files by a 6-byte ASCII
//! magic `"TextST"` followed by a Dialog Style segment (0x81).  Each segment
//! has the layout
//!
//! ```text
//! 1 byte   segment_type (0x81 Dialog Style, 0x82 Dialog Presentation, 0x80 END)
//! 2 bytes  segment_length (big-endian)
//! ...      segment payload
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

const SEGMENT_HEADER_LEN: usize = 3;
pub const MAGIC: [u8; 6] = *b"TextST";

const SEG_DIALOG_STYLE: u8 = 0x81;
const SEG_DIALOG_PRESENTATION: u8 = 0x82;
const SEG_END: u8 = 0x80;

fn is_valid_segment_type(b: u8) -> bool {
  matches!(b, SEG_DIALOG_STYLE | SEG_DIALOG_PRESENTATION | SEG_END)
}

/// Walks the segment chain.  Returns the count of valid segments when the
/// file starts with the `"TextST"` magic followed by a Dialog Style header.
pub fn count_segments(bytes: &[u8]) -> Option<usize> {
  if bytes.len() < MAGIC.len() + SEGMENT_HEADER_LEN {
    return None;
  }
  if bytes[..MAGIC.len()] != MAGIC {
    return None;
  }
  if bytes[MAGIC.len()] != SEG_DIALOG_STYLE {
    return None;
  }
  let mut pos = MAGIC.len();
  let mut count = 0usize;
  let first_len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
  let first_end = pos.checked_add(SEGMENT_HEADER_LEN)?.checked_add(first_len)?;
  if first_end > bytes.len() {
    return None;
  }
  count += 1;
  pos = first_end;
  // Between the Dialog Style segment and presentation segments, TextST stores
  // a two-byte frame count. Probe only requires the first segment, so tolerate
  // files whose bounded probe window ends here, but do not walk those bytes as
  // a segment descriptor.
  if pos + 2 <= bytes.len() {
    pos += 2;
  } else {
    return Some(count);
  }
  while pos + SEGMENT_HEADER_LEN <= bytes.len() {
    let seg_type = bytes[pos];
    if !is_valid_segment_type(seg_type) {
      break;
    }
    let seg_len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
    let next = pos.checked_add(SEGMENT_HEADER_LEN)?.checked_add(seg_len)?;
    if next > bytes.len() {
      break;
    }
    pos = next;
    count += 1;
  }
  if count == 0 { None } else { Some(count) }
}

pub fn dialog_style_segment(bytes: &[u8]) -> Option<&[u8]> {
  if bytes.len() < MAGIC.len() + SEGMENT_HEADER_LEN || bytes[..MAGIC.len()] != MAGIC {
    return None;
  }
  let pos = MAGIC.len();
  if bytes[pos] != SEG_DIALOG_STYLE {
    return None;
  }
  let seg_len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
  bytes.get(pos..pos + SEGMENT_HEADER_LEN + seg_len)
}

fn read_dialog_style_segment(src: &mut FileSource) -> Result<Option<Vec<u8>>, ParseError> {
  src.seek_to(0)?;
  let mut prefix = [0u8; MAGIC.len() + SEGMENT_HEADER_LEN];
  if src.read_at_most(&mut prefix)? < prefix.len() {
    return Ok(None);
  }
  if prefix[..MAGIC.len()] != MAGIC {
    return Ok(None);
  }
  let segment_header = &prefix[MAGIC.len()..];
  if segment_header[0] != SEG_DIALOG_STYLE {
    return Ok(None);
  }
  let seg_len = u16::from_be_bytes([segment_header[1], segment_header[2]]) as usize;
  let mut private = segment_header.to_vec();
  let old_len = private.len();
  private.resize(old_len + seg_len, 0);
  if seg_len != 0 {
    let read = src.read_at_most(&mut private[old_len..])?;
    if read != seg_len {
      return Ok(None);
    }
  }
  Ok(Some(private))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HdmvTextStReader;

impl Reader for HdmvTextStReader {
  fn name(&self) -> &'static str {
    "hdmv_textst"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let recognised = read_dialog_style_segment(src)?.is_some();
    src.seek_to(0)?;
    Ok(recognised)
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let Some(private) = read_dialog_style_segment(src)? else {
      return Err(ParseError::Unrecognised);
    };

    out.container.format = ContainerFormat::HdmvTextSt;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_HDMV/TEXTST".to_string(),
        name: Some("HDMV TextST".to_string()),
        codec_private: Some(CodecPrivate::from_bytes(&private)),
      },
      properties: TrackProperties {
        common,
        // mkvtoolnix identifies S_HDMV/TEXTST as a subtitle track carrying the
        // Dialog Style segment as codec private; its `identify()`
        // (`r_hdmv_textst.cpp`) does not set `text_subtitles` and exposes no
        // character encoding.  The TextST character coding is part of the
        // Blu-ray data model and is not necessarily UTF-8, so labelling it as
        // plain UTF-8 text was misleading (PARSER-248).
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: false,
          encoding: None,
          variant: Some("HDMV TextST".to_string()),
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
    bytes.push(seg_type);
    let len = payload.len() as u16;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
  }

  fn build_clip(segments: Vec<Vec<u8>>) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for seg in segments {
      bytes.extend(seg);
    }
    bytes
  }

  #[test]
  fn count_segments_accepts_magic_then_style_then_presentation() {
    let mut blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &[0u8; 8])]);
    blob.extend_from_slice(&2u16.to_be_bytes()); // number of frames
    blob.extend(build_segment(SEG_DIALOG_PRESENTATION, &[0u8; 16]));
    blob.extend(build_segment(SEG_END, &[]));
    assert_eq!(count_segments(&blob), Some(3));
  }

  #[test]
  fn count_segments_rejects_truncated_style_payload() {
    let mut blob = MAGIC.to_vec();
    blob.push(SEG_DIALOG_STYLE);
    blob.extend_from_slice(&8u16.to_be_bytes());
    blob.extend_from_slice(&[0u8; 2]);
    assert!(count_segments(&blob).is_none());
  }

  #[test]
  fn count_segments_rejects_without_textst_magic() {
    let blob = build_segment(SEG_DIALOG_STYLE, &[0u8; 8]);
    assert!(count_segments(&blob).is_none());
  }

  #[test]
  fn count_segments_requires_style_first() {
    let blob = build_clip(vec![build_segment(SEG_DIALOG_PRESENTATION, &[0u8; 8])]);
    assert!(count_segments(&blob).is_none());
  }

  #[test]
  fn count_segments_rejects_invalid_type() {
    let blob = build_clip(vec![build_segment(0x42, &[])]);
    assert!(count_segments(&blob).is_none());
  }

  #[test]
  fn probe_accepts_textst_blob() {
    let blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &[0u8; 8])]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(HdmvTextStReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_preserves_max_length_dialog_style_segment() {
    let blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &vec![0x7bu8; u16::MAX as usize])]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.clone()));
    assert!(HdmvTextStReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.textst", 0);
    HdmvTextStReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(
      out.tracks[0].codec.codec_private.as_ref().unwrap().length,
      (SEGMENT_HEADER_LEN + u16::MAX as usize) as u64
    );
  }

  #[test]
  fn probe_rejects_truncated_style_segment() {
    let mut blob = MAGIC.to_vec();
    blob.push(SEG_DIALOG_STYLE);
    blob.extend_from_slice(&8u16.to_be_bytes());
    blob.extend_from_slice(&[0u8; 2]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!HdmvTextStReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_textst_track() {
    let blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &[0u8; 8])]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.textst", 0);
    HdmvTextStReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::HdmvTextSt);
    let sub = out.tracks[0].properties.subtitle.as_ref().unwrap();
    // PARSER-248: HDMV TextST is not exposed as plain UTF-8 text; the Dialog
    // Style segment is carried as codec private instead.
    assert!(!sub.text_subtitles);
    assert!(sub.encoding.is_none());
    assert_eq!(sub.variant.as_deref(), Some("HDMV TextST"));
    assert_eq!(out.tracks[0].codec.codec_private.as_ref().unwrap().length, 11);
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!HdmvTextStReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0x81u8]));
    assert!(!HdmvTextStReader.probe(&mut s).unwrap());
  }
}
