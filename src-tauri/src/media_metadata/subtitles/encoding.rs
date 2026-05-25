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
//! (UTF-8 / UTF-16 LE / UTF-16 BE).  For BOM-less files we consult the
//! per-parse subtitle-charset hint pushed by `parse()` (PARSER-089) before
//! falling back to UTF-8.

use std::cell::RefCell;

use encoding_rs::Encoding;

thread_local! {
    /// Per-thread subtitle charset hint, set by `parse()` for the duration
    /// of a single call.  Empty string means "auto".
    static SUBTITLE_CHARSET_HINT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Set the subtitle charset hint for the current thread.  Returns the previous
/// value so callers can restore it on exit (`parse()` does this around its
/// dispatch call).
pub fn set_subtitle_charset_hint(label: String) -> String {
    SUBTITLE_CHARSET_HINT.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), label))
}

fn lookup_hint() -> Option<&'static Encoding> {
    SUBTITLE_CHARSET_HINT.with(|cell| {
        let s = cell.borrow();
        if s.is_empty() {
            None
        } else {
            Encoding::for_label(s.as_bytes())
        }
    })
}

/// Detected text encoding + byte offset where the decoded payload begins
/// (the BOM is stripped from the returned start).
#[derive(Debug, Clone, Copy)]
pub struct DetectedEncoding {
    pub label: &'static str,
    pub bom_length: usize,
}

/// Sniff the BOM at the start of `bytes`.  Falls back to the per-thread
/// charset hint if one was set, otherwise to UTF-8 (PARSER-089).
pub fn detect(bytes: &[u8]) -> DetectedEncoding {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let label: &'static str = match enc.name() {
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
    if let Some(enc) = lookup_hint() {
        return DetectedEncoding {
            label: enc.name(),
            bom_length: 0,
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
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let (decoded, _, _) = enc.decode(&bytes[bom_len..]);
        return decoded.into_owned();
    }
    let encoding = lookup_hint().unwrap_or(encoding_rs::UTF_8);
    let (decoded, _, _) = encoding.decode(bytes);
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

    // ---- PARSER-089: configurable subtitle charset --------------------

    #[test]
    fn hint_overrides_default_for_bom_less_text() {
        // Pre-test cleanup in case a parallel test set the hint.
        let prev = set_subtitle_charset_hint("windows-1252".to_string());
        // "café" in Windows-1252 — 0xE9 is `é`.
        let bytes = [b'c', b'a', b'f', 0xE9];
        let d = detect(&bytes);
        assert_eq!(d.label, "windows-1252");
        let decoded = decode_lossy(&bytes);
        assert_eq!(decoded, "café");
        set_subtitle_charset_hint(prev);
    }

    #[test]
    fn hint_ignored_when_bom_is_present() {
        let prev = set_subtitle_charset_hint("windows-1252".to_string());
        let bytes = [0xEFu8, 0xBB, 0xBF, b'a'];
        let d = detect(&bytes);
        assert_eq!(d.label, "UTF-8");
        set_subtitle_charset_hint(prev);
    }

    #[test]
    fn empty_hint_keeps_utf8_default() {
        let prev = set_subtitle_charset_hint(String::new());
        let d = detect(b"plain");
        assert_eq!(d.label, "UTF-8");
        set_subtitle_charset_hint(prev);
    }
}
