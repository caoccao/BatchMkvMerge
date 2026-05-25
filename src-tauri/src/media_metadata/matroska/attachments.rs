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

const NAME_CAP: u64 = 4 * 1024;
const MIME_CAP: u64 = 512;
const DESCRIPTION_CAP: u64 = 4 * 1024;

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  let mut next_id: u32 = (out.attachments.len() as u32) + 1;
  ebml::walk_children(src, parent, "matroska::attachments", deadline, |src, child| {
    if child.id != ids::ATTACHED_FILE {
      return Ok(ChildAction::Skip);
    }
    if let Some(att) = read_one(src, child, deadline, next_id)? {
      out.attachments.push(att);
      next_id += 1;
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
        name = Some(ebml::read_string(src, child, NAME_CAP)?);
      }
      ids::FILE_DESCRIPTION => {
        description = Some(ebml::read_string(src, child, DESCRIPTION_CAP)?);
      }
      ids::FILE_MIME_TYPE => {
        mime = Some(ebml::read_string(src, child, MIME_CAP)?);
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
  let Some(_) = data_size else {
    return Ok(None);
  };
  Ok(Some(Attachment {
    id,
    file_name: name.unwrap_or_default(),
    mime_type: mime,
    description,
    size: data_size.unwrap_or(0),
    uid_hex: uid.map(|u| format!("{:016x}", u)),
  }))
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
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    out.attachments
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

  #[test]
  fn description_round_trips() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::FILE_NAME, 2, "x"));
    payload.extend(encode_element_string(ids::FILE_DESCRIPTION, 2, "Front cover"));
    payload.extend(encode_element(ids::FILE_DATA, 2, &[0u8; 1]));
    let bytes = encode_element(ids::ATTACHED_FILE, 2, &payload);
    let v = parse_attachments(bytes);
    assert_eq!(v[0].description.as_deref(), Some("Front cover"));
  }
}
