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

//! Shared ID3v2 / ID3v1 detection.
//!
//! Frame-synced audio formats (MP3, AAC, AC-3, DTS) routinely have their
//! payload bookended by ID3 tags.  Layout per ID3v2 §3.1:
//!
//! ```text
//! "ID3"               (3 bytes)
//! u8  major_version
//! u8  revision
//! u8  flags           (bit 4 = footer present, bit 6 = extended header)
//! u32 size            (synchsafe — top bit of every byte is 0)
//! ```
//!
//! Total skip = 10 + decoded_size [+ 10 if footer-flag is set].
//!
//! ID3v1 sits at the *end* of the file (last 128 bytes start with "TAG").
//! We expose a helper for that as well.

const ID3V2_HEADER_SIZE: usize = 10;
const ID3V2_FOOTER_SIZE: usize = 10;
const ID3V1_SIZE: usize = 128;

/// `Some(offset)` is the byte position where audio data starts.  Returns
/// `None` when the buffer doesn't start with `ID3`.
pub fn skip_id3v2(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < ID3V2_HEADER_SIZE || &bytes[..3] != b"ID3" {
        return None;
    }
    let flags = bytes[5];
    let size = synchsafe_to_u32(bytes[6], bytes[7], bytes[8], bytes[9]) as usize;
    let footer = if flags & 0x10 != 0 { ID3V2_FOOTER_SIZE } else { 0 };
    Some(ID3V2_HEADER_SIZE + size + footer)
}

/// `true` when the trailing 128 bytes of a file form an ID3v1 tag.  The
/// caller is expected to pass the *last* 128 bytes (or longer suffix).
pub fn has_id3v1_trailer(tail: &[u8]) -> bool {
    if tail.len() < ID3V1_SIZE {
        return false;
    }
    &tail[tail.len() - ID3V1_SIZE..tail.len() - ID3V1_SIZE + 3] == b"TAG"
}

/// Decode a 4-byte synchsafe integer (top bit of every byte is 0).
pub fn synchsafe_to_u32(b0: u8, b1: u8, b2: u8, b3: u8) -> u32 {
    ((b0 as u32 & 0x7F) << 21)
        | ((b1 as u32 & 0x7F) << 14)
        | ((b2 as u32 & 0x7F) << 7)
        | (b3 as u32 & 0x7F)
}

/// File size left for the audio payload after stripping both kinds of tags.
pub fn payload_bounds(bytes: &[u8]) -> (usize, usize) {
    let start = skip_id3v2(bytes).unwrap_or(0);
    let end = if bytes.len() >= ID3V1_SIZE && has_id3v1_trailer(bytes) {
        bytes.len() - ID3V1_SIZE
    } else {
        bytes.len()
    };
    let end = end.max(start);
    (start, end)
}

#[cfg(test)]
pub(crate) fn build_id3v2_tag(footer: bool, body_size: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(ID3V2_HEADER_SIZE + body_size + if footer { ID3V2_FOOTER_SIZE } else { 0 });
    v.extend_from_slice(b"ID3");
    v.push(4); // major_version
    v.push(0); // revision
    v.push(if footer { 0x10 } else { 0 }); // flags
    let s = body_size as u32;
    v.push(((s >> 21) & 0x7F) as u8);
    v.push(((s >> 14) & 0x7F) as u8);
    v.push(((s >> 7) & 0x7F) as u8);
    v.push((s & 0x7F) as u8);
    v.extend(vec![0u8; body_size]);
    if footer {
        v.extend(vec![0u8; ID3V2_FOOTER_SIZE]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchsafe_decoding_handles_max_value() {
        assert_eq!(synchsafe_to_u32(0x7F, 0x7F, 0x7F, 0x7F), 0x0FFF_FFFF);
        assert_eq!(synchsafe_to_u32(0, 0, 0, 0), 0);
        assert_eq!(synchsafe_to_u32(0, 0, 0, 1), 1);
    }

    #[test]
    fn skip_id3v2_returns_none_without_magic() {
        assert!(skip_id3v2(&[0u8; 64]).is_none());
    }

    #[test]
    fn skip_id3v2_returns_none_when_too_short() {
        assert!(skip_id3v2(b"ID3").is_none());
    }

    #[test]
    fn skip_id3v2_returns_full_header_plus_body() {
        let tag = build_id3v2_tag(false, 256);
        assert_eq!(skip_id3v2(&tag), Some(10 + 256));
    }

    #[test]
    fn skip_id3v2_includes_footer_when_flag_set() {
        let tag = build_id3v2_tag(true, 128);
        assert_eq!(skip_id3v2(&tag), Some(10 + 128 + 10));
    }

    #[test]
    fn has_id3v1_trailer_detects_tag_marker() {
        let mut bytes = vec![0u8; 1024];
        bytes[1024 - 128] = b'T';
        bytes[1024 - 127] = b'A';
        bytes[1024 - 126] = b'G';
        assert!(has_id3v1_trailer(&bytes));
    }

    #[test]
    fn has_id3v1_trailer_returns_false_when_short() {
        assert!(!has_id3v1_trailer(&[0u8; 64]));
    }

    #[test]
    fn payload_bounds_strips_id3v2_header_only() {
        let mut bytes = build_id3v2_tag(false, 32);
        bytes.extend(vec![0xFFu8; 100]);
        let (start, end) = payload_bounds(&bytes);
        assert_eq!(start, 10 + 32);
        assert_eq!(end, bytes.len());
    }

    #[test]
    fn payload_bounds_strips_id3v1_trailer() {
        let mut bytes = vec![0xFFu8; 256];
        let len = bytes.len();
        bytes[len - 128] = b'T';
        bytes[len - 127] = b'A';
        bytes[len - 126] = b'G';
        let (start, end) = payload_bounds(&bytes);
        assert_eq!(start, 0);
        assert_eq!(end, len - 128);
    }

    #[test]
    fn payload_bounds_does_not_let_end_drop_below_start() {
        let tag = build_id3v2_tag(false, 16);
        let (start, end) = payload_bounds(&tag);
        // Whole file is the tag — end should clamp to start, not go negative.
        assert!(end >= start);
    }
}
