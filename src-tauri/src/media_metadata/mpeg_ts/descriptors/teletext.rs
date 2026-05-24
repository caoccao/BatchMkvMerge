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
//! `(ISO-639 lang × 3 + type/magazine byte + page byte)`.  The page number is
//! BCD-encoded `(magazine_number × 100 + page_tens × 10 + page_units)`.
//! Magazine 0 is conventionally "800".

pub fn decode(body: &[u8]) -> Option<u32> {
    if body.len() < 5 {
        return None;
    }
    let type_magazine = body[3];
    let page_byte = body[4];
    let magazine = (type_magazine & 0x07) as u32;
    let mag_norm = if magazine == 0 { 8 } else { magazine };
    let tens = ((page_byte >> 4) & 0x0F) as u32;
    let units = (page_byte & 0x0F) as u32;
    if tens > 9 || units > 9 {
        return None;
    }
    Some(mag_norm * 100 + tens * 10 + units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_page_888() {
        // Magazine 0 → 8, page 0x88 → 8 8
        let body = [b'e', b'n', b'g', 0x00, 0x88];
        assert_eq!(decode(&body), Some(888));
    }

    #[test]
    fn decodes_page_100() {
        // Magazine 1, page 0x00 → "100"
        let body = [b'e', b'n', b'g', 0x01, 0x00];
        assert_eq!(decode(&body), Some(100));
    }

    #[test]
    fn rejects_non_bcd_page() {
        // 0xAB → tens=10 → not BCD
        let body = [b'e', b'n', b'g', 0x00, 0xAB];
        assert!(decode(&body).is_none());
    }

    #[test]
    fn rejects_truncated_body() {
        assert!(decode(&[1, 2, 3]).is_none());
    }

    #[test]
    fn decodes_magazine_7_page_99() {
        let body = [b'd', b'e', b'u', 0x07, 0x99];
        assert_eq!(decode(&body), Some(799));
    }
}
