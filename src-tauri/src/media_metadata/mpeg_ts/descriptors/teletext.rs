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

//! Teletext descriptor (tag 0x56) — DVB SI ETSI EN 300 468 §6.2.43.
//!
//! Body: one or more 5-byte entries
//! `(ISO-639 lang × 3 + (type/magazine) + page byte)`.  The page byte is
//! BCD-encoded `(magazine × 100 + tens × 10 + units)`; magazine 0 maps to
//! "8" (the 800-page block).
//!
//! mkvtoolnix `r_mpeg_ts.cpp:760-817` walks every entry and only treats
//! teletext types 2 (subtitle) and 5 (subtitle for the hearing impaired) as
//! subtitles — PARSER-092.

/// One decoded teletext entry.  Mirrors mkvtoolnix's per-entry state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeletextEntry {
    pub language_iso_639_2: String,
    /// 5-bit type field — Table 94 of ETSI EN 300 468.
    pub teletext_type: u8,
    pub page: u32,
}

impl TeletextEntry {
    /// `true` when this entry is one of the documented subtitle types
    /// (2 = subtitle page, 5 = hearing-impaired subtitle page).
    pub fn is_subtitle(&self) -> bool {
        matches!(self.teletext_type, 2 | 5)
    }

    /// `true` when this is the hearing-impaired subtitle variant.
    pub fn is_hearing_impaired(&self) -> bool {
        self.teletext_type == 5
    }
}

/// Decode every 5-byte entry in the descriptor body.  Truncated trailers are
/// dropped silently; the BCD page check rejects malformed page bytes.
pub fn decode_all(body: &[u8]) -> Vec<TeletextEntry> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 5 <= body.len() {
        let lang_bytes = &body[pos..pos + 3];
        let type_magazine = body[pos + 3];
        let page_byte = body[pos + 4];
        let teletext_type = (type_magazine >> 3) & 0x1F;
        let magazine = (type_magazine & 0x07) as u32;
        let mag_norm = if magazine == 0 { 8 } else { magazine };
        let tens = ((page_byte >> 4) & 0x0F) as u32;
        let units = (page_byte & 0x0F) as u32;
        pos += 5;
        if tens > 9 || units > 9 {
            continue;
        }
        let page = mag_norm * 100 + tens * 10 + units;
        let language_iso_639_2 = String::from_utf8_lossy(lang_bytes)
            .trim_end_matches('\0')
            .trim()
            .to_string();
        out.push(TeletextEntry {
            language_iso_639_2,
            teletext_type,
            page,
        });
    }
    out
}

/// Legacy single-entry decoder kept for back-compat with the
/// [`DescriptorSummary::teletext_page`] field.  Returns the first decoded
/// page regardless of teletext type.
pub fn decode(body: &[u8]) -> Option<u32> {
    decode_all(body).first().map(|e| e.page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_page_888() {
        // type 0, magazine 0 → 8, page 0x88 → 88
        let body = [b'e', b'n', b'g', 0x00, 0x88];
        let v = decode_all(&body);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].language_iso_639_2, "eng");
        assert_eq!(v[0].teletext_type, 0);
        assert_eq!(v[0].page, 888);
    }

    #[test]
    fn decodes_page_100() {
        let body = [b'e', b'n', b'g', 0x01, 0x00];
        assert_eq!(decode(&body), Some(100));
    }

    #[test]
    fn type_field_extracted_from_top_five_bits() {
        // teletext type 2 (subtitle page) in the top 5 bits, magazine = 1.
        let body = [b'e', b'n', b'g', (2 << 3) | 0x01, 0x88];
        let v = decode_all(&body);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].teletext_type, 2);
        assert!(v[0].is_subtitle());
        assert!(!v[0].is_hearing_impaired());
        assert_eq!(v[0].page, 188);
    }

    #[test]
    fn hearing_impaired_subtitle_flag_set_for_type_5() {
        let body = [b'd', b'e', b'u', (5 << 3) | 0x02, 0x12];
        let v = decode_all(&body);
        assert!(v[0].is_subtitle());
        assert!(v[0].is_hearing_impaired());
        assert_eq!(v[0].language_iso_639_2, "deu");
        assert_eq!(v[0].page, 212);
    }

    #[test]
    fn rejects_non_bcd_page() {
        let body = [b'e', b'n', b'g', 0x00, 0xAB];
        assert!(decode_all(&body).is_empty());
    }

    #[test]
    fn rejects_truncated_body() {
        assert!(decode_all(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn decodes_multiple_entries() {
        let mut body = Vec::new();
        body.extend_from_slice(&[b'e', b'n', b'g', (2 << 3) | 0x01, 0x50]); // subtitle page 150
        body.extend_from_slice(&[b'd', b'e', b'u', (5 << 3) | 0x02, 0x12]); // hearing-impaired 212
        body.extend_from_slice(&[b'f', b'r', b'a', 0x00, 0x88]);            // initial-page 888
        let v = decode_all(&body);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].page, 150);
        assert_eq!(v[1].page, 212);
        assert_eq!(v[2].page, 888);
        assert!(v[0].is_subtitle() && !v[0].is_hearing_impaired());
        assert!(v[1].is_subtitle() && v[1].is_hearing_impaired());
        assert!(!v[2].is_subtitle());
    }
}
