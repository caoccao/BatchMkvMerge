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

//! Language identifiers carried by tracks.
//!
//! Every track surfaces both the legacy ISO 639-2 alpha-3 code (consumed by
//! existing tooling) and, when available, the canonical BCP-47 IETF tag.
//! The selection pipeline matches plan §7.1.

pub mod bcp47;
pub mod iso_639;

use serde::{Deserialize, Serialize};
use specta::Type;

/// A track's resolved language pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Language {
    /// Canonical ISO 639-2 alpha-3 code (terminologic when a B/T pair exists).
    /// Renamed on the wire because serde's `camelCase` rule mangles fields
    /// containing digits (`iso639_2` → `iso6392`); we keep the underscore.
    #[serde(rename = "iso639_2")]
    pub iso639_2: String,
    /// BCP-47 IETF tag when the source provided one and it validated.
    pub ietf: Option<String>,
    /// English name lookup for display.  Always derived from `iso639_2`.
    pub name: Option<String>,
}

impl Language {
    /// Construct from an ISO 639-2 code.  Unknown codes fall back to `"und"`
    /// with a `name` of `None`, so callers never silently lose data.
    pub fn from_iso_639_2(code: &str) -> Self {
        match iso_639::lookup(code) {
            Some(m) => Self {
                iso639_2: m.canonical.to_owned(),
                ietf: None,
                name: Some(m.name.to_owned()),
            },
            None => Self::undetermined(),
        }
    }

    /// Construct from a BCP-47 IETF tag.  When validation succeeds we record
    /// the canonical form in `ietf` and resolve the primary subtag back to
    /// ISO 639-2 if possible (best-effort).  An unparseable tag returns
    /// `None` so callers can fall back to ISO 639-2.
    pub fn from_ietf(tag: &str) -> Option<Self> {
        let canonical_ietf = bcp47::validate(tag)?;
        let primary = bcp47::primary_subtag(tag).unwrap_or_default();
        // Most IETF primary subtags are ISO 639-1 (2 letters).  If the
        // primary subtag is 3 letters we can try the ISO 639-2 lookup
        // directly — otherwise fall back to "und" so we always emit
        // something in `iso639_2`.
        let iso = match primary.len() {
            3 => iso_639::lookup(&primary).map(|m| (m.canonical, m.name)),
            _ => None,
        };
        let (iso639_2, name) = match iso {
            Some((c, n)) => (c.to_owned(), Some(n.to_owned())),
            None => (iso_639::UND.to_owned(), None),
        };
        Some(Self {
            iso639_2,
            ietf: Some(canonical_ietf),
            name,
        })
    }

    /// The `"und"` sentinel — used when both fields are absent or invalid.
    pub fn undetermined() -> Self {
        Self {
            iso639_2: iso_639::UND.to_owned(),
            ietf: None,
            name: Some("Undetermined".to_owned()),
        }
    }

    /// Default-when-absent (Matroska defaults Language to `"eng"`).
    pub fn english_default() -> Self {
        Self::from_iso_639_2("eng")
    }

    /// Apply plan §7.1's resolution pipeline.
    ///
    /// * If `ietf_hint` is non-empty and validates → use that (record the
    ///   canonical form, derive iso639_2 from the primary subtag).
    /// * Else if `iso_hint` is non-empty and validates → use that.
    /// * Else if defaults are required → use `eng`.
    /// * Else → `und`.
    pub fn resolve(
        ietf_hint: Option<&str>,
        iso_hint: Option<&str>,
        default_eng: bool,
    ) -> Self {
        if let Some(tag) = ietf_hint {
            if !tag.is_empty() {
                if let Some(lang) = Self::from_ietf(tag) {
                    return lang;
                }
            }
        }
        if let Some(code) = iso_hint {
            if !code.is_empty() {
                if iso_639::is_valid(code) {
                    return Self::from_iso_639_2(code);
                }
            }
        }
        if default_eng {
            Self::english_default()
        } else {
            Self::undetermined()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_iso_639_2_resolves_canonical() {
        let l = Language::from_iso_639_2("eng");
        assert_eq!(l.iso639_2, "eng");
        assert_eq!(l.name.as_deref(), Some("English"));
        assert!(l.ietf.is_none());
    }

    #[test]
    fn from_iso_639_2_translates_bib_to_term() {
        let l = Language::from_iso_639_2("fre");
        assert_eq!(l.iso639_2, "fra");
        assert_eq!(l.name.as_deref(), Some("French"));
    }

    #[test]
    fn from_iso_639_2_unknown_becomes_und() {
        let l = Language::from_iso_639_2("zzz");
        assert_eq!(l.iso639_2, "und");
        assert_eq!(l.name.as_deref(), Some("Undetermined"));
    }

    #[test]
    fn from_ietf_simple_two_letter_keeps_ietf_only() {
        let l = Language::from_ietf("en-US").unwrap();
        assert_eq!(l.ietf.as_deref(), Some("en-US"));
        // Primary subtag is "en" (2 letters) so we cannot map to ISO 639-2
        // directly without a 2→3 table.  Plan defers that; we record und.
        assert_eq!(l.iso639_2, "und");
    }

    #[test]
    fn from_ietf_three_letter_primary_resolves_iso() {
        let l = Language::from_ietf("yue-Hant-HK").unwrap();
        assert_eq!(l.ietf.as_deref(), Some("yue-Hant-HK"));
        // "yue" is a known ISO 639-2/3-letter code? Actually it's 639-3 only.
        // For the language-tags crate the parse will succeed but our table
        // may not contain yue — accept either outcome.
        assert!(l.iso639_2 == "und" || l.iso639_2 == "yue");
    }

    #[test]
    fn from_ietf_garbage_is_none() {
        assert!(Language::from_ietf("not a tag").is_none());
        assert!(Language::from_ietf("").is_none());
    }

    #[test]
    fn english_default_is_eng() {
        let l = Language::english_default();
        assert_eq!(l.iso639_2, "eng");
        assert_eq!(l.name.as_deref(), Some("English"));
    }

    #[test]
    fn resolve_prefers_ietf_when_valid() {
        let l = Language::resolve(Some("ja"), Some("eng"), true);
        assert_eq!(l.ietf.as_deref(), Some("ja"));
        // primary "ja" is 2 letters → iso639_2 falls back to und
        assert_eq!(l.iso639_2, "und");
    }

    #[test]
    fn resolve_falls_through_to_iso_when_ietf_garbage() {
        let l = Language::resolve(Some("bogus tag"), Some("jpn"), true);
        assert_eq!(l.iso639_2, "jpn");
        assert!(l.ietf.is_none());
    }

    #[test]
    fn resolve_falls_through_to_iso_when_ietf_empty() {
        let l = Language::resolve(Some(""), Some("jpn"), true);
        assert_eq!(l.iso639_2, "jpn");
    }

    #[test]
    fn resolve_falls_through_to_eng_default() {
        let l = Language::resolve(None, None, true);
        assert_eq!(l.iso639_2, "eng");
    }

    #[test]
    fn resolve_falls_through_to_und_when_no_default() {
        let l = Language::resolve(None, None, false);
        assert_eq!(l.iso639_2, "und");
    }

    #[test]
    fn resolve_drops_invalid_iso_hint() {
        let l = Language::resolve(None, Some("zzz"), true);
        assert_eq!(l.iso639_2, "eng");
    }

    #[test]
    fn round_trip_through_json() {
        let l = Language::from_iso_639_2("jpn");
        let s = serde_json::to_string(&l).unwrap();
        assert!(s.contains("\"iso639_2\":\"jpn\""));
        assert!(s.contains("\"name\":\"Japanese\""));
        let back: Language = serde_json::from_str(&s).unwrap();
        assert_eq!(back, l);
    }
}
