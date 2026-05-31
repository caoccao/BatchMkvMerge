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

//! `ilst` (iTunes metadata list) walker.  Each child is a 4-byte tag box
//! wrapping one `data` atom:
//!
//! ```text
//! data {
//!   u32 size
//!   "data"
//!   u32 type_code     // 1 = UTF-8, 2 = UTF-16, 21 = signed int, ...
//!   u32 locale
//!   u8  value[..]
//! }
//! ```
//!
//! We map the same identification-time fields mkvtoolnix reads from `ilst`:
//! title (`©nam`), encoder/muxing app (`©too`), comment (`©cmt`) and cover art
//! (`covr`).  Other metadata atoms are skipped instead of being surfaced as
//! generic tags.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::attachment::Attachment;
use crate::media_metadata::model::container::ContainerProperties;
use crate::media_metadata::model::tag::TagEntry;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

const TYPE_JPEG: u32 = 13;
const TYPE_PNG: u32 = 14;
const TYPE_BMP: u32 = 27;
#[cfg(test)]
const TYPE_UTF8: u32 = 1;

pub fn parse(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::ilst", deadline, |src, child| {
    let key = child.kind.0;
    // mkvtoolnix consumes `----:iTunSMPB` internally for encoder delay/padding
    // and does not expose it in identify output.  We have no encoder-delay
    // model field, so all freeform atoms are skipped here rather than reported
    // as extra tags.
    if &key == b"----" {
      return Ok(ChildAction::Skip);
    }

    if matches!(&key, b"covr") {
      let value = match read_cover_value(src, child, deadline) {
        Ok(v) => v,
        Err(_) => return Ok(ChildAction::Skip),
      };
      if let Some(DataValue::Image { mime_type, length }) = value {
        let id = (out.attachments.len() as u32) + 1;
        let extension = primary_extension_for_mime(mime_type);
        let file_name = format!("cover.{extension}");
        out.attachments.push(Attachment {
          id,
          file_name,
          mime_type: Some(mime_type.to_string()),
          description: None,
          size: length as u64,
          uid_hex: None,
        });
      }
      return Ok(ChildAction::Consumed);
    }

    if !matches!(&key, b"\xA9nam" | b"\xA9too" | b"\xA9cmt") {
      return Ok(ChildAction::Skip);
    }

    let value = match read_text_value(src, child, deadline) {
      Ok(v) => v,
      Err(_) => return Ok(ChildAction::Skip),
    };
    if let Some(value) = value {
      route_text(&key, value, &mut out.container.properties, &mut out.tags.global);
    }
    Ok(ChildAction::Consumed)
  })
}

fn primary_extension_for_mime(mime: &str) -> &'static str {
  match mime {
    "image/jpeg" => "jpg",
    "image/png" => "png",
    "image/bmp" => "bmp",
    _ => "bin",
  }
}

#[derive(Debug, Clone)]
enum DataValue {
  /// PARSER-072: cover-art image payload.  We don't materialise the body;
  /// the declared length plus the detected MIME type are enough to expose
  /// it as an `Attachment`.
  Image { mime_type: &'static str, length: usize },
}

fn read_text_value(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
) -> Result<Option<String>, ParseError> {
  let mut result: Option<String> = None;
  atom::walk_children(src, parent, "mp4::ilst::tag", deadline, |src, child| {
    if !child.kind.eq_ascii(b"data") {
      return Ok(ChildAction::Skip);
    }
    let payload = atom::read_payload(src, child, deadline.max_element_size())?;
    if payload.len() < 8 {
      return Ok(ChildAction::Consumed);
    }
    result = Some(String::from_utf8_lossy(&payload[8..]).trim().to_string());
    Ok(ChildAction::Consumed)
  })?;
  Ok(result)
}

fn read_cover_value(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
) -> Result<Option<DataValue>, ParseError> {
  let mut result: Option<DataValue> = None;
  atom::walk_children(src, parent, "mp4::ilst::cover", deadline, |src, child| {
    if !child.kind.eq_ascii(b"data") {
      return Ok(ChildAction::Skip);
    }
    let payload_size = child.payload_size().unwrap_or(0);
    if payload_size < 8 {
      return Ok(ChildAction::Consumed);
    }
    let mut head = [0u8; 8];
    src.read_exact(&mut head)?;
    let type_code = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) & 0x00FF_FFFF;
    // PARSER-162: album art (`covr`) routinely exceeds small text caps. We only need
    // the MIME type and the declared body length, so for image payloads we
    // derive the length from the box size and never buffer the bytes — the
    // walker re-aligns to the child's end. mkvtoolnix likewise records the
    // full `covr` payload size as an attachment (r_qtmp4.cpp:1087-1120).
    let body_len = (payload_size - 8) as usize;
    result = match type_code {
      TYPE_JPEG => Some(DataValue::Image {
        mime_type: "image/jpeg",
        length: body_len,
      }),
      TYPE_PNG => Some(DataValue::Image {
        mime_type: "image/png",
        length: body_len,
      }),
      TYPE_BMP => Some(DataValue::Image {
        mime_type: "image/bmp",
        length: body_len,
      }),
      _ => None,
    };
    Ok(ChildAction::Consumed)
  })?;
  Ok(result)
}

fn route_text(key: &[u8; 4], text: String, container: &mut ContainerProperties, global_tags: &mut Vec<TagEntry>) {
  match key {
    b"\xA9nam" => container.title = Some(text),
    b"\xA9too" => container.muxing_app = Some(text),
    b"\xA9cmt" => {
      global_tags.push(TagEntry {
        name: "comment".to_string(),
        value: text,
        language: None,
      });
    }
    _ => {}
  }
}

#[cfg(test)]
pub(crate) fn build_data_box(type_code: u32, value: &[u8]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&type_code.to_be_bytes());
  p.extend_from_slice(&0u32.to_be_bytes()); // locale
  p.extend_from_slice(value);
  crate::media_metadata::mp4::atom::encode_box(b"data", &p)
}

#[cfg(test)]
pub(crate) fn build_ilst_tag(key: &[u8; 4], data_box: Vec<u8>) -> Vec<u8> {
  crate::media_metadata::mp4::atom::encode_box(key, &data_box)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn run(payload: Vec<u8>) -> MediaMetadata {
    let bytes = encode_box(b"ilst", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut m = MediaMetadata::new("clip.mp4", 0);
    parse(&mut s, &h, &dl(), &mut m).unwrap();
    m
  }

  #[test]
  fn title_extracted_into_container() {
    let payload = build_ilst_tag(b"\xA9nam", build_data_box(TYPE_UTF8, b"My Movie"));
    let m = run(payload);
    assert_eq!(m.container.properties.title.as_deref(), Some("My Movie"));
  }

  #[test]
  fn encoder_into_muxing_app() {
    let payload = build_ilst_tag(b"\xA9too", build_data_box(TYPE_UTF8, b"HandBrake 1.6.1"));
    let m = run(payload);
    assert_eq!(m.container.properties.muxing_app.as_deref(), Some("HandBrake 1.6.1"),);
  }

  #[test]
  fn comment_extracted_as_supported_global_tag() {
    let payload = build_ilst_tag(b"\xA9cmt", build_data_box(TYPE_UTF8, b"  Director's cut  "));
    let m = run(payload);
    assert_eq!(m.tags.global.len(), 1);
    assert_eq!(m.tags.global[0].name, "comment");
    assert_eq!(m.tags.global[0].value, "Director's cut");
  }

  #[test]
  fn unsupported_text_atoms_are_ignored() {
    let mut payload = build_ilst_tag(b"\xA9day", build_data_box(TYPE_UTF8, b"2024-03-14"));
    payload.extend(build_ilst_tag(b"\xA9ART", build_data_box(TYPE_UTF8, b"Hans Zimmer")));
    let m = run(payload);
    assert!(m.container.properties.date_utc.is_none());
    assert!(m.tags.global.is_empty());
  }

  #[test]
  fn long_title_is_not_truncated_by_old_text_cap() {
    let mut title = vec![b'A'; 20 * 1024];
    title.extend_from_slice(b"tail");
    let payload = build_ilst_tag(b"\xA9nam", build_data_box(TYPE_UTF8, &title));
    let m = run(payload);
    assert_eq!(m.container.properties.title.as_ref().unwrap().len(), 20 * 1024 + 4);
    assert!(m.container.properties.title.as_ref().unwrap().ends_with("tail"));
  }

  // ---- PARSER-072: covr → attachment --------------------------------

  #[test]
  fn covr_jpeg_becomes_attachment() {
    let payload = build_ilst_tag(b"covr", build_data_box(TYPE_JPEG, &[0u8; 256]));
    let m = run(payload);
    assert!(m.tags.global.is_empty());
    assert_eq!(m.attachments.len(), 1);
    let att = &m.attachments[0];
    assert_eq!(att.file_name, "cover.jpg");
    assert_eq!(att.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(att.size, 256);
  }

  #[test]
  fn covr_larger_than_16kib_is_not_dropped() {
    // PARSER-162: a 64 KiB JPEG cover must still be exposed as an attachment
    // (the old 16 KiB payload cap silently discarded it).
    let payload = build_ilst_tag(b"covr", build_data_box(TYPE_JPEG, &vec![0u8; 64 * 1024]));
    let m = run(payload);
    assert_eq!(m.attachments.len(), 1);
    let att = &m.attachments[0];
    assert_eq!(att.file_name, "cover.jpg");
    assert_eq!(att.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(att.size, 64 * 1024);
  }

  #[test]
  fn covr_png_becomes_attachment() {
    let payload = build_ilst_tag(b"covr", build_data_box(TYPE_PNG, &[0u8; 64]));
    let m = run(payload);
    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.attachments[0].file_name, "cover.png");
  }

  // ---- Freeform metadata is consumed but not exposed ------------------

  #[test]
  fn freeform_itunsmpb_is_not_exposed_as_global_tag() {
    let mean = encode_box(b"mean", &{
      let mut p = vec![0u8; 4]; // version + flags
      p.extend_from_slice(b"com.apple.iTunes");
      p
    });
    let name = encode_box(b"name", &{
      let mut p = vec![0u8; 4];
      p.extend_from_slice(b"iTunSMPB");
      p
    });
    let data = build_data_box(TYPE_UTF8, b" 00000000 00000840 00000000 00000000");
    let mut freeform = mean;
    freeform.extend(name);
    freeform.extend(data);
    let payload = encode_box(b"----", &freeform);
    let m = run(payload);
    assert!(m.tags.global.is_empty());
  }

  #[test]
  fn malformed_data_box_skipped() {
    // Tag with no data child
    let tag = encode_box(b"\xA9nam", &[]);
    let m = run(tag);
    assert!(m.container.properties.title.is_none());
  }
}
