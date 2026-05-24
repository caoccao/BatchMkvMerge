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

//! Shared text-encoding detection for the subtitle readers.
//!
//! We use [`encoding_rs::Encoding::for_bom`] for BOM-anchored detection
//! (UTF-8 / UTF-16 LE / UTF-16 BE).  For BOM-less files we default to
//! UTF-8 and call into the lossy decoder so non-ASCII bytes never panic.

use encoding_rs::Encoding;

/// Detected text encoding + byte offset where the decoded payload begins
/// (the BOM is stripped from the returned start).
#[derive(Debug, Clone, Copy)]
pub struct DetectedEncoding {
    pub label: &'static str,
    pub bom_length: usize,
}

/// Sniff the BOM at the start of `bytes`.  Falls back to UTF-8 when no BOM
/// is present.
pub fn detect(bytes: &[u8]) -> DetectedEncoding {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let label = match enc.name() {
            "UTF-8" => "UTF-8",
            "UTF-16LE" => "UTF-16 LE",
            "UTF-16BE" => "UTF-16 BE",
            _ => "UTF-8",
        };
        return DetectedEncoding {
            label,
            bom_length: bom_len,
        };
    }
    DetectedEncoding {
        label: "UTF-8",
        bom_length: 0,
    }
}

/// Decode a probe slice into a `Cow<str>` for line-prefix sniffing.  We
/// always run the decoder so callers don't have to special-case UTF-16.
pub fn decode_lossy(bytes: &[u8]) -> String {
    let detected = detect(bytes);
    let body = &bytes[detected.bom_length..];
    let encoding = match detected.label {
        "UTF-16 LE" => encoding_rs::UTF_16LE,
        "UTF-16 BE" => encoding_rs::UTF_16BE,
        _ => encoding_rs::UTF_8,
    };
    let (decoded, _, _) = encoding.decode(body);
    decoded.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf8_bom() {
        let bytes = [0xEFu8, 0xBB, 0xBF, b'a', b'b'];
        let d = detect(&bytes);
        assert_eq!(d.label, "UTF-8");
        assert_eq!(d.bom_length, 3);
    }

    #[test]
    fn detects_utf16_le_bom() {
        let bytes = [0xFFu8, 0xFE, b'a', 0];
        let d = detect(&bytes);
        assert_eq!(d.label, "UTF-16 LE");
        assert_eq!(d.bom_length, 2);
    }

    #[test]
    fn detects_utf16_be_bom() {
        let bytes = [0xFEu8, 0xFF, 0, b'a'];
        let d = detect(&bytes);
        assert_eq!(d.label, "UTF-16 BE");
        assert_eq!(d.bom_length, 2);
    }

    #[test]
    fn no_bom_defaults_to_utf8() {
        let bytes = b"plain ascii";
        let d = detect(bytes);
        assert_eq!(d.label, "UTF-8");
        assert_eq!(d.bom_length, 0);
    }

    #[test]
    fn decode_lossy_handles_utf16_le() {
        // "Hi" in UTF-16 LE with BOM: FF FE 48 00 69 00
        let bytes = [0xFFu8, 0xFE, b'H', 0, b'i', 0];
        let decoded = decode_lossy(&bytes);
        assert_eq!(decoded, "Hi");
    }

    #[test]
    fn decode_lossy_passes_through_utf8() {
        assert_eq!(decode_lossy(b"hello"), "hello");
    }

    #[test]
    fn decode_lossy_handles_utf8_bom() {
        let bytes = [0xEFu8, 0xBB, 0xBF, b'h', b'i'];
        assert_eq!(decode_lossy(&bytes), "hi");
    }

    #[test]
    fn decode_lossy_replaces_invalid_utf8_bytes() {
        let bytes = [b'a', 0xFF, b'b'];
        let decoded = decode_lossy(&bytes);
        assert!(decoded.starts_with('a'));
        assert!(decoded.ends_with('b'));
    }
}
