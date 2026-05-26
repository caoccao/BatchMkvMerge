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

//! VorbisComment block decoder.
//!
//! Layout (Vorbis I §5):
//!
//! ```text
//! u32 vendor_length (LE)
//! [vendor_length bytes vendor_string (UTF-8)]
//! u32 user_comment_list_length (LE)
//! repeat user_comment_list_length:
//!   u32 length (LE)
//!   [length bytes "KEY=VALUE" (UTF-8)]
//! ```
//!
//! VorbisComment is shared by Vorbis (with packet type 0x03 + "vorbis"
//! prefix), Opus (with "OpusTags" prefix), and Theora (with packet type
//! 0x81 + "theora" prefix).  We hand off the prefix stripping to the caller.

use crate::media_metadata::model::attachment::Attachment;
use crate::media_metadata::model::tag::TagEntry;

#[derive(Debug, Clone)]
pub struct VorbisComments {
  pub vendor: String,
  pub entries: Vec<TagEntry>,
}

/// Decode a VorbisComment block starting at byte 0 of `bytes`.  Returns
/// `None` if the buffer is malformed.
pub fn parse(bytes: &[u8]) -> Option<VorbisComments> {
  if bytes.len() < 4 {
    return None;
  }
  let vendor_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
  let mut pos = 4usize;
  if pos + vendor_len > bytes.len() {
    return None;
  }
  let vendor = String::from_utf8_lossy(&bytes[pos..pos + vendor_len]).into_owned();
  pos += vendor_len;
  if pos + 4 > bytes.len() {
    return None;
  }
  let comments_count = u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
  pos += 4;

  let mut entries = Vec::with_capacity(comments_count.min(1024));
  for _ in 0..comments_count {
    // PARSER-261: mkvtoolnix reads every declared comment inside a try block
    // (`parse_vorbis_comments_from_packet`, `../mkvtoolnix/src/common/tags/vorbis.cpp:221-279`);
    // a short read throws and the whole comment object is discarded.  A
    // truncated count/length/body must therefore yield `None`, not a partial
    // list, so we don't surface tags / language / chapters / cover art that
    // mkvmerge treats as invalid and ignores.
    if pos + 4 > bytes.len() {
      return None;
    }
    let len = u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
    pos += 4;
    if pos + len > bytes.len() {
      return None;
    }
    let entry_str = std::str::from_utf8(&bytes[pos..pos + len]).ok()?;
    pos += len;
    if let Some((name, value)) = entry_str.split_once('=') {
      entries.push(TagEntry {
        name: name.to_string(),
        value: value.to_string(),
        language: None,
      });
    }
  }
  Some(VorbisComments { vendor, entries })
}

/// Convert a `METADATA_BLOCK_PICTURE` Vorbis comment value (base64-encoded
/// FLAC PICTURE block, per Xiph spec) into an [`Attachment`].  Returns `None`
/// when the base64 decode fails or the embedded PICTURE block is truncated.
/// PARSER-083.
pub fn metadata_block_picture_to_attachment(base64_value: &str, id: u32) -> Option<Attachment> {
  let bytes = decode_base64(base64_value)?;
  // PICTURE block layout matches the FLAC spec §8.4: picture-type (u32 BE),
  // MIME length + bytes, description length + bytes, four u32 dimension
  // fields, then declared data length.  We don't need the actual image
  // body — only the declared metadata.
  let mut pos = 0usize;
  let _picture_type = read_be_u32(&bytes, &mut pos)?;
  let mime_len = read_be_u32(&bytes, &mut pos)? as usize;
  let mime_type = read_utf8(&bytes, &mut pos, mime_len)?;
  let desc_len = read_be_u32(&bytes, &mut pos)? as usize;
  let description = read_utf8(&bytes, &mut pos, desc_len)?;
  for _ in 0..4 {
    let _ = read_be_u32(&bytes, &mut pos)?;
  }
  let data_length = read_be_u32(&bytes, &mut pos)?;
  if mime_type.is_empty() {
    return None;
  }
  let extension = primary_extension_for_mime(&mime_type);
  let file_name = if extension.is_empty() {
    "cover".to_string()
  } else {
    format!("cover.{extension}")
  };
  Some(Attachment {
    id,
    file_name,
    mime_type: Some(mime_type),
    description: if description.is_empty() {
      None
    } else {
      Some(description)
    },
    size: data_length as u64,
    uid_hex: None,
  })
}

fn read_be_u32(body: &[u8], pos: &mut usize) -> Option<u32> {
  if *pos + 4 > body.len() {
    return None;
  }
  let v = u32::from_be_bytes([body[*pos], body[*pos + 1], body[*pos + 2], body[*pos + 3]]);
  *pos += 4;
  Some(v)
}

fn read_utf8(body: &[u8], pos: &mut usize, len: usize) -> Option<String> {
  if *pos + len > body.len() {
    return None;
  }
  let s = String::from_utf8_lossy(&body[*pos..*pos + len]).into_owned();
  *pos += len;
  Some(s)
}

fn primary_extension_for_mime(mime: &str) -> &'static str {
  match mime.to_ascii_lowercase().as_str() {
    "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/jfif" => "jpg",
    "image/png" => "png",
    "image/gif" => "gif",
    "image/bmp" | "image/x-bmp" => "bmp",
    "image/webp" => "webp",
    "image/tiff" => "tiff",
    "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
    _ => "",
  }
}

/// Tiny base64 decoder — sufficient for the
/// `METADATA_BLOCK_PICTURE` payloads emitted by encoders that follow the
/// Xiph spec.  Returns `None` on any non-alphabet byte (whitespace skipped).
fn decode_base64(input: &str) -> Option<Vec<u8>> {
  fn value(b: u8) -> Option<u8> {
    match b {
      b'A'..=b'Z' => Some(b - b'A'),
      b'a'..=b'z' => Some(b - b'a' + 26),
      b'0'..=b'9' => Some(b - b'0' + 52),
      b'+' => Some(62),
      b'/' => Some(63),
      _ => None,
    }
  }
  let mut buf = [0u8; 4];
  let mut filled = 0usize;
  let mut out = Vec::with_capacity(input.len() * 3 / 4);
  for &b in input.as_bytes() {
    if b.is_ascii_whitespace() {
      continue;
    }
    if b == b'=' {
      buf[filled] = 0;
      filled += 1;
    } else {
      buf[filled] = value(b)?;
      filled += 1;
    }
    if filled == 4 {
      out.push((buf[0] << 2) | (buf[1] >> 4));
      out.push((buf[1] << 4) | (buf[2] >> 2));
      out.push((buf[2] << 6) | buf[3]);
      filled = 0;
    }
  }
  // Trim padding bytes implied by trailing `=`.
  let pad_count = input.bytes().rev().take_while(|b| *b == b'=').count();
  for _ in 0..pad_count {
    out.pop();
  }
  Some(out)
}

/// Pull the language out of the comment list, if any (`LANGUAGE=xx` is the
/// VorbisComment convention used by Ogg/OGM for per-stream language).
pub fn extract_language(entries: &[TagEntry]) -> Option<String> {
  entries.iter().find_map(|e| {
    if e.name.eq_ignore_ascii_case("LANGUAGE") {
      Some(e.value.clone())
    } else {
      None
    }
  })
}

#[cfg(test)]
pub(crate) fn build_block(vendor: &str, entries: &[(&str, &str)]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
  p.extend_from_slice(vendor.as_bytes());
  p.extend_from_slice(&(entries.len() as u32).to_le_bytes());
  for (k, v) in entries {
    let entry = format!("{}={}", k, v);
    p.extend_from_slice(&(entry.len() as u32).to_le_bytes());
    p.extend_from_slice(entry.as_bytes());
  }
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_vendor_and_entries() {
    let block = build_block("libvorbis 1.3.7", &[("TITLE", "Track"), ("ARTIST", "Hans Zimmer")]);
    let v = parse(&block).unwrap();
    assert_eq!(v.vendor, "libvorbis 1.3.7");
    assert_eq!(v.entries.len(), 2);
    assert_eq!(v.entries[0].name, "TITLE");
    assert_eq!(v.entries[1].value, "Hans Zimmer");
  }

  #[test]
  fn returns_none_on_truncated_vendor_length() {
    assert!(parse(&[0xFF, 0xFF, 0xFF, 0xFF]).is_none());
  }

  #[test]
  fn returns_none_on_truncated_count() {
    let mut bytes = 0u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 1]); // missing 3 bytes of count
    assert!(parse(&bytes).is_none());
  }

  #[test]
  fn truncated_entry_body_is_rejected() {
    // PARSER-261: a comment whose declared length runs past the buffer makes
    // the whole block invalid (mkvtoolnix discards it), so we return None
    // rather than a partial entry list.
    let mut bytes = build_block("v", &[("TITLE", "x")]);
    bytes.truncate(bytes.len() - 1); // chop the last byte of the value
    assert!(parse(&bytes).is_none());
  }

  #[test]
  fn truncated_later_entry_discards_earlier_entries() {
    // First entry is complete; the second's length runs past the buffer. The
    // earlier (valid) entry must NOT survive — mkvmerge ignores the lot.
    let mut bytes = build_block("v", &[("TITLE", "Track"), ("ARTIST", "Hans")]);
    bytes.truncate(bytes.len() - 2); // chop into the second entry's body
    assert!(parse(&bytes).is_none());
  }

  #[test]
  fn comment_count_larger_than_payload_is_rejected() {
    // A count of 3 with only one entry present → the missing entries' length
    // prefix runs off the end → None.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_le_bytes()); // empty vendor
    bytes.extend_from_slice(&3u32.to_le_bytes()); // claims three comments
    let entry = "TITLE=x";
    bytes.extend_from_slice(&(entry.len() as u32).to_le_bytes());
    bytes.extend_from_slice(entry.as_bytes());
    assert!(parse(&bytes).is_none());
  }

  #[test]
  fn entries_without_equal_sign_are_dropped() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_le_bytes()); // empty vendor
    bytes.extend_from_slice(&1u32.to_le_bytes()); // one entry
    let entry = "NOEQUALSIGN";
    bytes.extend_from_slice(&(entry.len() as u32).to_le_bytes());
    bytes.extend_from_slice(entry.as_bytes());
    let v = parse(&bytes).unwrap();
    assert!(v.entries.is_empty());
  }

  #[test]
  fn extract_language_finds_language_tag() {
    let v = parse(&build_block(
      "v",
      &[("ARTIST", "A"), ("LANGUAGE", "fr"), ("TITLE", "T")],
    ))
    .unwrap();
    assert_eq!(extract_language(&v.entries).as_deref(), Some("fr"));
  }

  #[test]
  fn extract_language_is_case_insensitive() {
    let v = parse(&build_block("v", &[("language", "de")])).unwrap();
    assert_eq!(extract_language(&v.entries).as_deref(), Some("de"));
  }

  #[test]
  fn extract_language_returns_none_when_missing() {
    let v = parse(&build_block("v", &[("ARTIST", "A")])).unwrap();
    assert!(extract_language(&v.entries).is_none());
  }

  // ---- PARSER-083: METADATA_BLOCK_PICTURE → attachment -----------------

  fn encode_base64(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut iter = bytes.chunks_exact(3);
    for chunk in &mut iter {
      let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
      out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
      out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
      out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
      out.push(ALPHA[(n & 0x3F) as usize] as char);
    }
    let rem = iter.remainder();
    match rem.len() {
      1 => {
        let n = (rem[0] as u32) << 16;
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
      }
      2 => {
        let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
        out.push(ALPHA[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHA[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
      }
      _ => {}
    }
    out
  }

  fn build_picture_block(mime: &str, desc: &str, data_length: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&3u32.to_be_bytes()); // front cover
    b.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    b.extend_from_slice(mime.as_bytes());
    b.extend_from_slice(&(desc.len() as u32).to_be_bytes());
    b.extend_from_slice(desc.as_bytes());
    b.extend_from_slice(&0u32.to_be_bytes()); // width
    b.extend_from_slice(&0u32.to_be_bytes()); // height
    b.extend_from_slice(&0u32.to_be_bytes()); // depth
    b.extend_from_slice(&0u32.to_be_bytes()); // colours used
    b.extend_from_slice(&data_length.to_be_bytes());
    b
  }

  #[test]
  fn metadata_block_picture_decodes_to_attachment() {
    let block = build_picture_block("image/jpeg", "Front", 1024);
    let value = encode_base64(&block);
    let att = metadata_block_picture_to_attachment(&value, 1).unwrap();
    assert_eq!(att.file_name, "cover.jpg");
    assert_eq!(att.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(att.description.as_deref(), Some("Front"));
    assert_eq!(att.size, 1024);
  }

  #[test]
  fn metadata_block_picture_empty_mime_is_rejected() {
    let block = build_picture_block("", "", 0);
    let value = encode_base64(&block);
    assert!(metadata_block_picture_to_attachment(&value, 1).is_none());
  }

  #[test]
  fn invalid_utf8_payload_returns_none() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u32.to_le_bytes()); // empty vendor
    bytes.extend_from_slice(&1u32.to_le_bytes()); // one entry
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&[0xFFu8, 0xFE]); // invalid UTF-8
    assert!(parse(&bytes).is_none());
  }
}
