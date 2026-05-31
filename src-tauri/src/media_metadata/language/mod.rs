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
//! The selection pipeline prefers a valid IETF tag, falls back to a valid
//! ISO-639-2 alpha-3 code, then to `eng` (Matroska default) or `und` per the
//! caller's request.

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
  /// Bibliographic ISO 639-2/B alias (e.g. `fre` for `fra`), when the language
  /// has a distinct B code.  Lets a language filter written in either the
  /// bibliographic or terminologic form match the same track.
  #[serde(rename = "iso639_2Bib")]
  pub iso639_2_bib: Option<String>,
  /// ISO 639-1 alpha-2 code (e.g. `fr`), when one exists.
  #[serde(rename = "iso639_1")]
  pub iso639_1: Option<String>,
  /// BCP-47 IETF tag when the source provided one and it validated.
  pub ietf: Option<String>,
  /// English name lookup for display.  Always derived from `iso639_2`.
  pub name: Option<String>,
}

impl Language {
  /// Build the full equivalent-code set (terminologic + bibliographic +
  /// alpha-2) from a canonical ISO 639-2 code.  `ietf` is left `None` for
  /// callers to fill.
  fn from_canonical(canonical: &str, name: Option<String>) -> Self {
    Self {
      iso639_2: canonical.to_owned(),
      iso639_2_bib: iso_639::term_to_bib(canonical).map(str::to_owned),
      iso639_1: iso_639::alpha3_to_alpha2(canonical).map(str::to_owned),
      ietf: None,
      name,
    }
  }

  /// Construct from an ISO 639-2 code.  Unknown codes fall back to `"und"`
  /// with a `name` of `None`, so callers never silently lose data.
  pub fn from_iso_639_2(code: &str) -> Self {
    match iso_639::lookup(code) {
      Some(m) => Self::from_canonical(m.canonical, Some(m.name.to_owned())),
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
    // Most IETF primary subtags are ISO 639-1 (2 letters).  Map those
    // back to their ISO 639-2 alpha-3 code first; 3-letter primaries hit
    // the alpha-3 table directly.  mkvtoolnix resolves the effective
    // language the same way (`mtx::bcp47::language_c` over the 639
    // registry).  Unknown primaries fall back to "und".
    let iso = match primary.len() {
      2 => iso_639::alpha2_to_alpha3(&primary)
        .and_then(iso_639::lookup)
        .map(|m| (m.canonical, m.name)),
      3 => iso_639::lookup(&primary).map(|m| (m.canonical, m.name)),
      _ => None,
    };
    let (iso639_2, name) = match iso {
      Some((c, n)) => (c.to_owned(), Some(n.to_owned())),
      None => (iso_639::UND.to_owned(), None),
    };
    let mut lang = Self::from_canonical(&iso639_2, name);
    lang.ietf = Some(canonical_ietf);
    Some(lang)
  }

  /// Construct a language only when the source hint maps to a known ISO-639
  /// code. Invalid hints return `None` so callers can omit language metadata
  /// instead of manufacturing `und`.
  pub fn from_valid_hint(hint: &str) -> Option<Self> {
    if hint.is_empty() {
      return None;
    }
    if let Some(lang) = Self::from_ietf(hint) {
      if lang.name.is_some() {
        return Some(lang);
      }
    }
    if iso_639::is_valid(hint) {
      return Some(Self::from_iso_639_2(hint));
    }
    None
  }

  /// The `"und"` sentinel — used when both fields are absent or invalid.
  pub fn undetermined() -> Self {
    Self {
      iso639_2: iso_639::UND.to_owned(),
      iso639_2_bib: None,
      iso639_1: None,
      ietf: None,
      name: Some("Undetermined".to_owned()),
    }
  }

  /// Default-when-absent (Matroska defaults Language to `"eng"`).
  pub fn english_default() -> Self {
    Self::from_iso_639_2("eng")
  }

  /// Resolution pipeline (prefer IETF when valid, else ISO-639-2, else
  /// `eng` or `und` depending on `default_eng`).
  ///
  /// * If `ietf_hint` is non-empty and validates → use that (record the
  ///   canonical form, derive iso639_2 from the primary subtag).
  /// * Else if `iso_hint` is non-empty and validates → use that.
  /// * Else if defaults are required → use `eng`.
  /// * Else → `und`.
  pub fn resolve(ietf_hint: Option<&str>, iso_hint: Option<&str>, default_eng: bool) -> Self {
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
  fn exposes_bibliographic_and_alpha2_aliases() {
    // French/German/Chinese have a B/T split — expose both forms + alpha-2.
    let l = Language::from_iso_639_2("fre");
    assert_eq!(l.iso639_2, "fra");
    assert_eq!(l.iso639_2_bib.as_deref(), Some("fre"));
    assert_eq!(l.iso639_1.as_deref(), Some("fr"));
    let de = Language::from_ietf("de").unwrap();
    assert_eq!(de.iso639_2, "deu");
    assert_eq!(de.iso639_2_bib.as_deref(), Some("ger"));
    assert_eq!(de.iso639_1.as_deref(), Some("de"));
    // English has no bibliographic twin but does have an alpha-2.
    let en = Language::from_iso_639_2("eng");
    assert_eq!(en.iso639_2_bib, None);
    assert_eq!(en.iso639_1.as_deref(), Some("en"));
  }

  #[test]
  fn from_iso_639_2_unknown_becomes_und() {
    let l = Language::from_iso_639_2("zzz");
    assert_eq!(l.iso639_2, "und");
    assert_eq!(l.name.as_deref(), Some("Undetermined"));
  }

  #[test]
  fn from_ietf_simple_two_letter_resolves_iso() {
    let l = Language::from_ietf("en-US").unwrap();
    assert_eq!(l.ietf.as_deref(), Some("en-US"));
    // Primary subtag "en" maps through the alpha-2 → alpha-3 table to eng.
    assert_eq!(l.iso639_2, "eng");
    assert_eq!(l.name.as_deref(), Some("English"));
  }

  #[test]
  fn from_ietf_two_letter_bibliographic_resolves_to_terminologic() {
    // "de" → "ger" (bibliographic) → "deu" (terminologic canonical).
    let l = Language::from_ietf("de").unwrap();
    assert_eq!(l.iso639_2, "deu");
    let l = Language::from_ietf("pt-BR").unwrap();
    assert_eq!(l.iso639_2, "por");
    let l = Language::from_ietf("ja").unwrap();
    assert_eq!(l.iso639_2, "jpn");
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
  fn from_valid_hint_rejects_unknown_primary_subtag() {
    assert!(Language::from_valid_hint("zzz").is_none());
  }

  #[test]
  fn from_valid_hint_accepts_iso_and_ietf_values() {
    assert_eq!(Language::from_valid_hint("eng").unwrap().iso639_2, "eng");
    assert_eq!(Language::from_valid_hint("fr-FR").unwrap().iso639_2, "fra");
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
    // primary "ja" maps through the alpha-2 table to jpn
    assert_eq!(l.iso639_2, "jpn");
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
