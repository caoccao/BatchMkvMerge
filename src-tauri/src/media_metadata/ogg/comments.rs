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
    let comments_count = u32::from_le_bytes([
        bytes[pos],
        bytes[pos + 1],
        bytes[pos + 2],
        bytes[pos + 3],
    ]) as usize;
    pos += 4;

    let mut entries = Vec::with_capacity(comments_count.min(1024));
    for _ in 0..comments_count {
        if pos + 4 > bytes.len() {
            break;
        }
        let len = u32::from_le_bytes([
            bytes[pos],
            bytes[pos + 1],
            bytes[pos + 2],
            bytes[pos + 3],
        ]) as usize;
        pos += 4;
        if pos + len > bytes.len() {
            break;
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
    Some(VorbisComments {
        vendor,
        entries,
    })
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
        let block = build_block(
            "libvorbis 1.3.7",
            &[("TITLE", "Track"), ("ARTIST", "Hans Zimmer")],
        );
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
    fn stops_at_partial_entry_without_returning_none() {
        let mut bytes = build_block("v", &[("TITLE", "x")]);
        bytes.truncate(bytes.len() - 1); // chop the last byte of value
        // Should return Some with whatever could be parsed, or None on UTF-8
        // failure depending on truncation.  Either way: must not panic.
        let _ = parse(&bytes);
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
