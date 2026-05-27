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

//! Attachments parser.  Port of `r_matroska.cpp::handle_attachments`
//! (lines 885-940).
//!
//! Identification-time scope (we deliberately do not extract payload bytes):
//! - Read `KaxFileName`, `KaxFileDescription`, `KaxMimeType`, `KaxFileUID`.
//! - For `KaxFileData`, *only* record its declared byte size — we never
//!   read the payload itself.  The cursor seeks past it.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::attachment::Attachment;

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
  attachment_id: &mut u32,
) -> Result<(), ParseError> {
  // PARSER-140: mkvtoolnix increments `m_attachment_id` for *every*
  // AttachedFile element it encounters — including ones it later skips — and
  // only assigns that id after the filtering test passes
  // (r_matroska.cpp:914-934). Mirror that so emitted ids keep their original
  // 1-based positions even when intervening attachments are dropped.
  ebml::walk_children(src, parent, "matroska::attachments", deadline, |src, child| {
    if child.id != ids::ATTACHED_FILE {
      return Ok(ChildAction::Skip);
    }
    *attachment_id += 1;
    if let Some(att) = read_one(src, child, deadline, *attachment_id)? {
      out.attachments.push(att);
    }
    Ok(ChildAction::Consumed)
  })
}

fn read_one(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  id: u32,
) -> Result<Option<Attachment>, ParseError> {
  let mut name: Option<String> = None;
  let mut description: Option<String> = None;
  let mut mime: Option<String> = None;
  let mut uid: Option<u64> = None;
  let mut data_size: Option<u64> = None;
  ebml::walk_children(src, parent, "matroska::attached_file", deadline, |src, child| {
    match child.id {
      ids::FILE_NAME => {
        name = Some(ebml::read_string(src, child, deadline.max_element_size())?);
      }
      ids::FILE_DESCRIPTION => {
        description = Some(ebml::read_string(src, child, deadline.max_element_size())?);
      }
      ids::FILE_MIME_TYPE => {
        mime = Some(ebml::read_string(src, child, deadline.max_element_size())?);
      }
      ids::FILE_UID => {
        uid = Some(ebml::read_uint(src, child)?);
      }
      ids::FILE_DATA => {
        // Record declared size; never read the payload.  When
        // size is unknown (live-stream attachment) treat as 0.
        data_size = child.size;
        // Skip past so the walker continues to the next sibling.
        return Ok(ChildAction::Skip);
      }
      _ => return Ok(ChildAction::Skip),
    }
    Ok(ChildAction::Consumed)
  })?;
  // mkvtoolnix skips attachments missing FileData (r_matroska.cpp:917).
  let Some(data_size) = data_size else {
    return Ok(None);
  };
  // PARSER-140: drop empty payloads and empty MIME types
  // (r_matroska.cpp:929-931), and normalise legacy font MIME types to their
  // current `font/*` form via `mtx::mime::get_font_mime_type_to_use`.
  if data_size == 0 {
    return Ok(None);
  }
  let mime = mime.map(|m| normalize_font_mime_type(&m).to_string());
  match mime.as_deref() {
    None | Some("") => return Ok(None),
    _ => {}
  }
  Ok(Some(Attachment {
    id,
    file_name: name.unwrap_or_default(),
    mime_type: mime,
    description,
    size: data_size,
    uid_hex: uid.map(|u| format!("{:016x}", u)),
  }))
}

/// Map a legacy font MIME type to the current `font/*` equivalent, leaving
/// every other MIME type untouched.  Port of the `legacy → current` branch of
/// `mtx::mime::get_font_mime_type_to_use` (common/mime.cpp:114-139); mkvtoolnix
/// defaults to the current mapping unless `--engage use_legacy_font_mime_types`
/// is set, which the identification path never does.
fn normalize_font_mime_type(mime: &str) -> &str {
  match mime {
    "application/vnd.ms-opentype" | "application/x-font-otf" => "font/otf",
    "application/x-font-ttf" | "application/x-truetype-font" => "font/ttf",
    other => other,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_string, encode_element_uint};
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn parse_attachments(parent_payload: Vec<u8>) -> Vec<Attachment> {
    let bytes = encode_element(ids::ATTACHMENTS, 4, &parent_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    let mut out = MediaMetadata::new("clip.mkv", 0);
    let mut attachment_id = 0;
    parse(&mut s, &header, &no_deadline(), &mut out, &mut attachment_id).unwrap();
    out.attachments
  }

  fn parse_attachments_into(parent_payload: Vec<u8>, out: &mut MediaMetadata, attachment_id: &mut u32) {
    let bytes = encode_element(ids::ATTACHMENTS, 4, &parent_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    parse(&mut s, &header, &no_deadline(), out, attachment_id).unwrap();
  }

  fn build_attached_file(name: &str, mime: &str, uid: u64, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::FILE_NAME, 2, name));
    payload.extend(encode_element_string(ids::FILE_MIME_TYPE, 2, mime));
    payload.extend(encode_element_uint(ids::FILE_UID, 2, uid));
    payload.extend(encode_element(ids::FILE_DATA, 2, data));
    encode_element(ids::ATTACHED_FILE, 2, &payload)
  }

  #[test]
  fn single_attachment_extracted() {
    let payload = build_attached_file("cover.jpg", "image/jpeg", 0xDEADBEEF, &[0u8; 32]);
    let v = parse_attachments(payload);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].file_name, "cover.jpg");
    assert_eq!(v[0].mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(v[0].size, 32);
    assert_eq!(v[0].uid_hex.as_deref(), Some("00000000deadbeef"));
    assert_eq!(v[0].id, 1);
  }

  #[test]
  fn attachment_strings_use_shared_element_budget() {
    let long_name = "cover".repeat(1024);
    let payload = build_attached_file(&long_name, "application/octet-stream", 1, &[0u8; 4]);
    let v = parse_attachments(payload);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].file_name, long_name);
  }

  #[test]
  fn multiple_attachments_get_sequential_ids() {
    let mut payload = Vec::new();
    payload.extend(build_attached_file("a.bin", "application/octet-stream", 1, &[0u8; 1]));
    payload.extend(build_attached_file("b.bin", "application/octet-stream", 2, &[0u8; 1]));
    let v = parse_attachments(payload);
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].id, 1);
    assert_eq!(v[1].id, 2);
  }

  #[test]
  fn attachment_without_file_data_is_skipped() {
    let mut bad = Vec::new();
    bad.extend(encode_element_string(ids::FILE_NAME, 2, "noop.txt"));
    bad.extend(encode_element_string(ids::FILE_MIME_TYPE, 2, "text/plain"));
    let bad = encode_element(ids::ATTACHED_FILE, 2, &bad);
    let v = parse_attachments(bad);
    assert!(v.is_empty());
  }

  #[test]
  fn description_optional() {
    let payload = build_attached_file("cover.jpg", "image/jpeg", 1, &[0u8; 4]);
    let v = parse_attachments(payload);
    assert!(v[0].description.is_none());
  }

  #[test]
  fn attachment_data_is_not_read_into_memory() {
    // Build a 1 MiB attachment payload and parse it — should be cheap
    // because we only seek past the FileData payload.
    let payload = build_attached_file("big.bin", "application/octet-stream", 1, &vec![0u8; 1024 * 1024]);
    let v = parse_attachments(payload);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].size, 1024 * 1024);
  }

  #[test]
  fn empty_name_remains_empty_string() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::FILE_MIME_TYPE, 2, "text/plain"));
    payload.extend(encode_element(ids::FILE_DATA, 2, &[0u8; 4]));
    let bytes = encode_element(ids::ATTACHED_FILE, 2, &payload);
    let v = parse_attachments(bytes);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].file_name, "");
  }

  // ---- PARSER-140: filtering, IDs, font MIME normalization --------------

  #[test]
  fn skipped_attachment_still_consumes_an_id() {
    // First AttachedFile has empty data → skipped, but it still claims id 1,
    // so the second (valid) attachment is reported as id 2 (matching
    // mkvtoolnix's m_attachment_id bookkeeping).
    let empty = build_attached_file("empty.bin", "application/octet-stream", 1, &[]);
    let valid = build_attached_file("real.bin", "application/octet-stream", 2, &[0u8; 8]);
    let mut payload = Vec::new();
    payload.extend(empty);
    payload.extend(valid);
    let v = parse_attachments(payload);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].file_name, "real.bin");
    assert_eq!(v[0].id, 2);
  }

  #[test]
  fn skipped_attachment_consumes_id_across_attachment_elements() {
    // PARSER-277: `m_attachment_id` is reader-level in mkvtoolnix.  A skipped
    // attachment in one Attachments element must still consume an id before a
    // later Attachments element is parsed.
    let mut out = MediaMetadata::new("clip.mkv", 0);
    let mut attachment_id = 0;
    parse_attachments_into(
      build_attached_file("empty.bin", "application/octet-stream", 1, &[]),
      &mut out,
      &mut attachment_id,
    );
    parse_attachments_into(
      build_attached_file("real.bin", "application/octet-stream", 2, &[0u8; 8]),
      &mut out,
      &mut attachment_id,
    );
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].file_name, "real.bin");
    assert_eq!(out.attachments[0].id, 2);
  }

  #[test]
  fn empty_payload_attachment_is_skipped() {
    let payload = build_attached_file("zero.bin", "application/octet-stream", 1, &[]);
    let v = parse_attachments(payload);
    assert!(v.is_empty());
  }

  #[test]
  fn empty_mime_type_attachment_is_skipped() {
    let payload = build_attached_file("nomime.bin", "", 1, &[0u8; 4]);
    let v = parse_attachments(payload);
    assert!(v.is_empty());
  }

  #[test]
  fn missing_mime_type_attachment_is_skipped() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::FILE_NAME, 2, "nomime.bin"));
    payload.extend(encode_element(ids::FILE_DATA, 2, &[0u8; 4]));
    let bytes = encode_element(ids::ATTACHED_FILE, 2, &payload);
    let v = parse_attachments(bytes);
    assert!(v.is_empty());
  }

  #[test]
  fn legacy_font_mime_types_are_normalized() {
    for (legacy, current) in [
      ("application/vnd.ms-opentype", "font/otf"),
      ("application/x-font-otf", "font/otf"),
      ("application/x-font-ttf", "font/ttf"),
      ("application/x-truetype-font", "font/ttf"),
    ] {
      let payload = build_attached_file("font.bin", legacy, 1, &[0u8; 16]);
      let v = parse_attachments(payload);
      assert_eq!(v.len(), 1, "{legacy} should be kept");
      assert_eq!(v[0].mime_type.as_deref(), Some(current), "{legacy} → {current}");
    }
  }

  #[test]
  fn current_font_mime_type_passes_through_unchanged() {
    let payload = build_attached_file("font.ttf", "font/ttf", 1, &[0u8; 16]);
    let v = parse_attachments(payload);
    assert_eq!(v[0].mime_type.as_deref(), Some("font/ttf"));
  }

  #[test]
  fn description_round_trips() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::FILE_NAME, 2, "x"));
    payload.extend(encode_element_string(ids::FILE_MIME_TYPE, 2, "image/jpeg"));
    payload.extend(encode_element_string(ids::FILE_DESCRIPTION, 2, "Front cover"));
    payload.extend(encode_element(ids::FILE_DATA, 2, &[0u8; 1]));
    let bytes = encode_element(ids::ATTACHED_FILE, 2, &payload);
    let v = parse_attachments(bytes);
    assert_eq!(v[0].description.as_deref(), Some("Front cover"));
  }
}
