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

//! The MKV language list and the helpers built on it: alias lookup for
//! language filters, short-code normalisation for the editable Language cell,
//! and the option lists of the language and track-name autocompletes.
//!
//! The canonical cell value is the *short code*: the ISO 639-1 alpha-2 code
//! from the label's `(xx; yyy)` suffix when there is one, else the 3-letter
//! MKV code. Track-name presets in the settings are keyed by the MKV code.

mod table;

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::config::ConfigProfile;
use crate::media_track::MediaTrackType;

/// One entry of the MKV language list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MkvLanguage {
  /// The 3-letter MKV code (bibliographic where ISO 639-2 has two forms).
  pub code: &'static str,
  /// English name followed by the ISO 639 codes, e.g. `German (de; ger)`.
  pub label: &'static str,
}

/// Every language the pickers offer, in display order (`und` and `mul` first).
pub const MKV_LANGUAGES: &[MkvLanguage] = &table::ENTRIES;

/// A language option list: the profile's preferred languages first, then
/// every other language. The dropdown draws a divider after
/// `preferred_count` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageOptions {
  pub options: Vec<String>,
  pub preferred_count: usize,
}

struct LanguageIndex {
  /// Short and MKV code to label.
  label_by_code: HashMap<&'static str, &'static str>,
  /// Any lowercased form (MKV code, short code, alpha-2, English name, full
  /// label) to the short code; the first language claiming a form keeps it.
  short_code_by_any: HashMap<String, &'static str>,
  /// The same forms to the MKV code.
  mkv_code_by_any: HashMap<String, &'static str>,
  /// Distinct short codes in table order.
  short_codes: Vec<&'static str>,
  /// Each code of a label's suffix (and the MKV code) to all of them.
  codes_by_alias: HashMap<String, Vec<String>>,
}

static INDEX: LazyLock<LanguageIndex> = LazyLock::new(|| {
  let mut index = LanguageIndex {
    label_by_code: HashMap::new(),
    short_code_by_any: HashMap::new(),
    mkv_code_by_any: HashMap::new(),
    short_codes: Vec::new(),
    codes_by_alias: HashMap::new(),
  };
  for language in MKV_LANGUAGES {
    let short_code = short_language_code_for(language);
    let display_name = display_name(language.label);
    let two_letter = two_letter_code(language.label, false).unwrap_or("");
    for alias in [language.code, short_code, two_letter, display_name, language.label] {
      if alias.is_empty() {
        continue;
      }
      let key = alias.to_lowercase();
      index.short_code_by_any.entry(key.clone()).or_insert(short_code);
      index.mkv_code_by_any.entry(key).or_insert(language.code);
    }
    index.label_by_code.entry(short_code).or_insert(language.label);
    index.label_by_code.insert(language.code, language.label);
    if !index.short_codes.contains(&short_code) {
      index.short_codes.push(short_code);
    }

    let suffix = language
      .label
      .strip_suffix(')')
      .and_then(|rest| rest.rsplit_once('('))
      .map(|(_, codes)| codes)
      .filter(|codes| !codes.is_empty() && !codes.contains(')'));
    let mut codes: Vec<String> = Vec::new();
    for code in std::iter::once(language.code).chain(suffix.into_iter().flat_map(|codes| codes.split(';'))) {
      let code = code.trim().to_lowercase();
      if !code.is_empty() && !codes.contains(&code) {
        codes.push(code);
      }
    }
    for code in &codes {
      index.codes_by_alias.insert(code.clone(), codes.clone());
    }
  }
  index
});

/// The alpha-2 code of a `(xx; yyy)` group in `label`. With `at_end`, the
/// group must close the label; otherwise the first such group counts.
fn two_letter_code(label: &str, at_end: bool) -> Option<&str> {
  let bytes = label.as_bytes();
  let mut start = 0;
  while let Some(offset) = label[start..].find('(') {
    let open = start + offset;
    let rest = &bytes[open + 1..];
    if rest.len() >= 2 && rest[0].is_ascii_alphabetic() && rest[1].is_ascii_alphabetic() && rest.get(2) == Some(&b';') {
      let after = label[open + 4..].trim_start();
      let after_bytes = after.as_bytes();
      let closes =
        after_bytes.len() >= 4 && after_bytes[..3].iter().all(u8::is_ascii_alphabetic) && after_bytes[3] == b')';
      if closes && (!at_end || after.len() == 4) {
        return Some(&label[open + 1..open + 3]);
      }
    }
    start = open + 1;
  }
  None
}

/// The label without its trailing `( … )` group, e.g. `German`.
fn display_name(label: &str) -> &str {
  let trimmed = label.trim_end();
  if let Some(rest) = trimmed.strip_suffix(')')
    && let Some((name, codes)) = rest.rsplit_once('(')
    && !codes.contains(')')
  {
    return name.trim();
  }
  label.trim()
}

/// The canonical cell value of a language: its alpha-2 code when the label
/// ends in `(xx; yyy)`, else its MKV code.
pub fn short_language_code_for(language: &MkvLanguage) -> &'static str {
  two_letter_code(language.label, true).unwrap_or(language.code)
}

/// Every ISO code known for a language value (MKV code and the label's
/// codes), lowercased; an unknown value is returned alone, an empty one gives
/// nothing. Language filters match a track when any of its codes is listed.
pub fn language_code_aliases(value: &str) -> Vec<String> {
  let normalized = value.trim().to_lowercase();
  if let Some(codes) = INDEX.codes_by_alias.get(&normalized) {
    return codes.clone();
  }
  if normalized.is_empty() {
    Vec::new()
  } else {
    vec![normalized]
  }
}

/// Normalise any language form (3-letter code, alpha-2, English name, full
/// label; any case) to the short code. Unknown text is returned trimmed, so
/// free text survives.
pub fn to_short_code(value: &str) -> String {
  let trimmed = value.trim();
  match INDEX.short_code_by_any.get(&trimmed.to_lowercase()) {
    Some(code) => (*code).to_string(),
    None => trimmed.to_string(),
  }
}

/// Map any language form to the 3-letter MKV code that keys the track-name
/// presets; a BCP 47 tag maps by its primary language (`pt-BR` → `por`).
/// Unknown text is returned trimmed.
pub fn to_mkv_code(value: &str) -> String {
  let trimmed = value.trim();
  let lowered = trimmed.to_lowercase();
  let primary = lowered.split('-').next().unwrap_or_default();
  match INDEX
    .mkv_code_by_any
    .get(&lowered)
    .or_else(|| INDEX.mkv_code_by_any.get(primary))
  {
    Some(code) => (*code).to_string(),
    None => trimmed.to_string(),
  }
}

/// The Language cell's value for what the user typed, or `None` when
/// `mkvmerge` would reject it. A known language in any form (code, alias,
/// English name, label; any case) becomes its short code (`German` → `de`).
/// A well-formed BCP 47 tag whose primary language is known keeps its
/// subtags, with the primary as its short code and the subtags in the usual
/// case (`pt-br` → `pt-BR`, `ZH-HANT` → `zh-Hant`, `es-419`). Anything else,
/// a typo or an empty cell, is `None`.
pub fn normalize_language(value: &str) -> Option<String> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return None;
  }
  if let Some(code) = INDEX.short_code_by_any.get(&trimmed.to_lowercase()) {
    return Some((*code).to_string());
  }
  let tag = language_tags::LanguageTag::parse(trimmed).ok()?;
  let primary = INDEX.short_code_by_any.get(&tag.primary_language().to_lowercase())?;
  let mut subtags = vec![(*primary).to_string()];
  let mut in_extension = false;
  for subtag in trimmed.split('-').skip(1) {
    // After a singleton (`u`, `x`, …) every subtag is lowercase.
    in_extension |= subtag.len() == 1;
    let is_alpha = subtag.chars().all(|c| c.is_ascii_alphabetic());
    let is_digit = subtag.chars().all(|c| c.is_ascii_digit());
    subtags.push(if in_extension {
      subtag.to_ascii_lowercase()
    } else if subtag.len() == 4 && is_alpha {
      let (first, rest) = subtag.split_at(1);
      format!("{}{}", first.to_ascii_uppercase(), rest.to_ascii_lowercase())
    } else if (subtag.len() == 2 && is_alpha) || (subtag.len() == 3 && is_digit) {
      subtag.to_ascii_uppercase()
    } else {
      subtag.to_ascii_lowercase()
    });
  }
  Some(subtags.join("-"))
}

/// The label of a short or MKV code; the code itself when unknown.
pub fn language_label(code: &str) -> &str {
  INDEX.label_by_code.get(code).copied().unwrap_or(code)
}

/// Every distinct short code, in table order.
pub fn all_short_codes() -> &'static [&'static str] {
  &INDEX.short_codes
}

fn preferred_languages(profile: &ConfigProfile, track_type: MediaTrackType) -> Option<&str> {
  match track_type {
    MediaTrackType::Video => Some(&profile.preferred_video_languages),
    MediaTrackType::Audio => Some(&profile.preferred_audio_languages),
    MediaTrackType::Subtitles => Some(&profile.preferred_subtitle_languages),
    _ => None,
  }
}

fn options_from_preferred<'a>(preferred: impl IntoIterator<Item = &'a str>) -> LanguageOptions {
  let mut options: Vec<String> = Vec::new();
  for part in preferred.into_iter().flat_map(|list| list.split(',')).map(str::trim) {
    if part.is_empty() {
      continue;
    }
    let short = to_short_code(part);
    if !options.contains(&short) {
      options.push(short);
    }
  }
  let preferred_count = options.len();
  let rest: Vec<String> = all_short_codes()
    .iter()
    .filter(|code| !options[..preferred_count].iter().any(|option| option == *code))
    .map(|code| (*code).to_string())
    .collect();
  options.extend(rest);
  LanguageOptions {
    options,
    preferred_count,
  }
}

/// Language options of a track type's Language cell: the profile's preferred
/// languages for that type first (deduplicated, normalised to short codes),
/// then every other language. Types without a preference list get the plain
/// list.
pub fn build_language_options(profile: Option<&ConfigProfile>, track_type: MediaTrackType) -> LanguageOptions {
  options_from_preferred(profile.and_then(|profile| preferred_languages(profile, track_type)))
}

/// Options of a profile-wide picker: the union of the three preferred lists
/// (video, audio, subtitles) first, then every other language.
pub fn build_combined_language_options(profile: Option<&ConfigProfile>) -> LanguageOptions {
  options_from_preferred(profile.into_iter().flat_map(|profile| {
    [
      profile.preferred_video_languages.as_str(),
      profile.preferred_audio_languages.as_str(),
      profile.preferred_subtitle_languages.as_str(),
    ]
  }))
}

/// The track-name presets for a track type and language: one per line in the
/// profile, trimmed, blank lines dropped, case-insensitive duplicates dropped
/// (the first spelling wins). Presets are keyed by the MKV code, while the
/// cell holds the short code, so `language` is mapped back first.
pub fn build_track_name_options(
  profile: Option<&ConfigProfile>,
  track_type: MediaTrackType,
  language: &str,
) -> Vec<String> {
  let Some(profile) = profile else {
    return Vec::new();
  };
  let presets = match track_type {
    MediaTrackType::Video => &profile.track_names_video,
    MediaTrackType::Audio => &profile.track_names_audio,
    MediaTrackType::Subtitles => &profile.track_names_subtitle,
    _ => return Vec::new(),
  };
  let Some(raw) = presets.get(&to_mkv_code(language)) else {
    return Vec::new();
  };
  let mut seen: Vec<String> = Vec::new();
  let mut names = Vec::new();
  for name in raw.split('\n').map(str::trim) {
    let lowered = name.to_lowercase();
    if name.is_empty() || seen.contains(&lowered) {
      continue;
    }
    seen.push(lowered);
    names.push(name.to_string());
  }
  names
}

/// The option the language dropdown highlights for the typed text: an exact
/// alias match first, else the first option whose code starts with, or whose
/// label contains, the text (case-insensitive). `None` for blank text.
pub fn first_matching_language_option_index(options: &[String], input: &str) -> Option<usize> {
  let query = input.trim().to_lowercase();
  if query.is_empty() {
    return None;
  }
  if let Some(exact) = INDEX.short_code_by_any.get(&query)
    && let Some(index) = options.iter().position(|option| option == exact)
  {
    return Some(index);
  }
  options.iter().position(|option| {
    option.to_lowercase().starts_with(&query)
      || INDEX
        .label_by_code
        .get(option.as_str())
        .is_some_and(|label| label.to_lowercase().contains(&query))
  })
}

/// The option the track-name dropdown highlights: the first that starts with
/// the typed text, else the first that contains it (case-insensitive). `None`
/// for blank text.
pub fn first_matching_title_option_index(options: &[String], input: &str) -> Option<usize> {
  let query = input.trim().to_lowercase();
  if query.is_empty() {
    return None;
  }
  let lowered: Vec<String> = options.iter().map(|option| option.to_lowercase()).collect();
  lowered
    .iter()
    .position(|option| option.starts_with(&query))
    .or_else(|| lowered.iter().position(|option| option.contains(&query)))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn profile() -> ConfigProfile {
    ConfigProfile {
      preferred_video_languages: "eng".to_string(),
      preferred_audio_languages: "jpn, eng, ja,,".to_string(),
      preferred_subtitle_languages: "chi,Klingon".to_string(),
      ..ConfigProfile::default()
    }
  }

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
  }

  #[test]
  fn carries_the_mkv_language_table() {
    assert_eq!(MKV_LANGUAGES.len(), 186);
    assert_eq!(
      MKV_LANGUAGES[0],
      MkvLanguage {
        code: "und",
        label: "Undetermined (und)"
      }
    );
    assert_eq!(MKV_LANGUAGES[1].code, "mul");
    for language in MKV_LANGUAGES {
      assert_eq!(language.code.len(), 3, "{language:?}");
      assert!(language.label.ends_with(')'), "{language:?}");
    }
    let mut codes: Vec<_> = MKV_LANGUAGES.iter().map(|language| language.code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), MKV_LANGUAGES.len());
    assert_eq!(all_short_codes().len(), 186);
    assert_eq!(&all_short_codes()[..3], ["und", "mul", "ab"]);
  }

  #[test]
  fn derives_short_codes_and_display_names() {
    let german = MKV_LANGUAGES.iter().find(|language| language.code == "ger").unwrap();
    assert_eq!(short_language_code_for(german), "de");
    let und = MKV_LANGUAGES[0];
    assert_eq!(short_language_code_for(&und), "und");
    assert_eq!(display_name("Greek (modern, 1453-) (el; gre)"), "Greek (modern, 1453-)");
    assert_eq!(display_name("Undetermined (und)"), "Undetermined");
    assert_eq!(display_name("No codes"), "No codes");
    assert_eq!(display_name("Odd (a) b)"), "Odd (a) b)");
    assert_eq!(two_letter_code("X (ab;  cde)", true), Some("ab"));
    assert_eq!(two_letter_code("X (ab; cde) tail", true), None);
    assert_eq!(two_letter_code("X (ab; cde) tail", false), Some("ab"));
    assert_eq!(two_letter_code("X (abc; def) (gh; ijk)", false), Some("gh"));
    assert_eq!(two_letter_code("X (a1; cde)", false), None);
    assert_eq!(two_letter_code("X (ab;", false), None);
  }

  #[test]
  fn normalises_any_form_to_the_short_code() {
    assert_eq!(to_short_code("ger"), "de");
    assert_eq!(to_short_code("German"), "de");
    assert_eq!(to_short_code("GERMAN (DE; GER)"), "de");
    assert_eq!(to_short_code(" eng "), "en");
    assert_eq!(to_short_code("de"), "de");
    assert_eq!(to_short_code("Greek (modern, 1453-)"), "el");
    assert_eq!(to_short_code("und"), "und");
    assert_eq!(to_short_code(" Klingon "), "Klingon");
    assert_eq!(to_short_code(""), "");
  }

  #[test]
  fn maps_any_form_to_the_mkv_code() {
    assert_eq!(to_mkv_code("de"), "ger");
    assert_eq!(to_mkv_code("German"), "ger");
    assert_eq!(to_mkv_code("zh"), "chi");
    assert_eq!(to_mkv_code("und"), "und");
    // Only the codes the table lists are known: `deu` is not one of them.
    assert_eq!(to_mkv_code(" deu "), "deu");
  }

  #[test]
  fn labels_short_and_mkv_codes() {
    assert_eq!(language_label("de"), "German (de; ger)");
    assert_eq!(language_label("ger"), "German (de; ger)");
    assert_eq!(language_label("und"), "Undetermined (und)");
    assert_eq!(language_label("xx"), "xx");
  }

  #[test]
  fn lists_every_alias_of_a_language_value() {
    assert_eq!(language_code_aliases("fre"), ["fre", "fr"]);
    assert_eq!(language_code_aliases(" FR "), ["fre", "fr"]);
    assert_eq!(language_code_aliases("und"), ["und"]);
    assert_eq!(language_code_aliases("gre"), ["gre", "el"]);
    assert_eq!(language_code_aliases("XYZ"), ["xyz"]);
    assert!(language_code_aliases("  ").is_empty());
  }

  #[test]
  fn puts_preferred_languages_first() {
    let profile = profile();
    let audio = build_language_options(Some(&profile), MediaTrackType::Audio);
    assert_eq!(audio.preferred_count, 2);
    assert_eq!(&audio.options[..2], ["ja", "en"]);
    assert_eq!(audio.options.len(), 186);
    assert_eq!(audio.options.iter().filter(|code| *code == "en").count(), 1);
    assert_eq!(audio.options[2], "und");

    let subtitles = build_language_options(Some(&profile), MediaTrackType::Subtitles);
    assert_eq!(&subtitles.options[..2], ["zh", "Klingon"]);
    assert_eq!(subtitles.options.len(), 187);

    for (profile, track_type) in [
      (Some(&profile), MediaTrackType::Chapters),
      (None, MediaTrackType::Video),
    ] {
      let plain = build_language_options(profile, track_type);
      assert_eq!(plain.preferred_count, 0);
      assert_eq!(plain.options, all_short_codes());
    }
  }

  #[test]
  fn combines_the_three_preferred_lists() {
    let combined = build_combined_language_options(Some(&profile()));
    assert_eq!(combined.preferred_count, 4);
    assert_eq!(&combined.options[..4], ["en", "ja", "zh", "Klingon"]);
    assert_eq!(combined.options.len(), 187);
    assert_eq!(build_combined_language_options(None).preferred_count, 0);
  }

  #[test]
  fn lists_track_name_presets_by_mkv_code() {
    let mut profile = profile();
    profile
      .track_names_audio
      .insert("ger".to_string(), "Deutsch\n deutsch \n\nKommentar\n".to_string());
    assert_eq!(
      build_track_name_options(Some(&profile), MediaTrackType::Audio, "de"),
      ["Deutsch", "Kommentar"]
    );
    assert_eq!(
      build_track_name_options(Some(&profile), MediaTrackType::Subtitles, "zh"),
      ["Simplified Chinese", "Traditional Chinese"]
    );
    assert_eq!(
      build_track_name_options(Some(&profile), MediaTrackType::Video, "chi"),
      ["Mandarin", "Cantonese"]
    );
    assert!(build_track_name_options(Some(&profile), MediaTrackType::Audio, "Klingon").is_empty());
    assert!(build_track_name_options(Some(&profile), MediaTrackType::Attachment, "en").is_empty());
    assert!(build_track_name_options(None, MediaTrackType::Audio, "en").is_empty());
  }

  #[test]
  fn finds_the_language_option_to_highlight() {
    let options = build_language_options(Some(&profile()), MediaTrackType::Audio).options;
    let find = |input: &str| first_matching_language_option_index(&options, input).map(|index| options[index].as_str());
    assert_eq!(find(""), None);
    assert_eq!(find("   "), None);
    assert_eq!(find("ger"), Some("de"));
    assert_eq!(find("German"), Some("de"));
    assert_eq!(find("japan"), Some("ja"));
    // A label substring counts: "Japanese" contains "e".
    assert_eq!(find("e"), Some("ja"));
    assert_eq!(find("EN"), Some("en"));
    assert_eq!(find("zz-none"), None);
    // An exact alias missing from the options falls back to prefix / label.
    let subset = strings(&["en", "fr"]);
    assert_eq!(first_matching_language_option_index(&subset, "ger"), None);
    assert_eq!(first_matching_language_option_index(&subset, "fren"), Some(1));
  }

  #[test]
  fn finds_the_title_option_to_highlight() {
    let options = strings(&["Main", "Commentary", "Director's Commentary"]);
    let find = |input: &str| first_matching_title_option_index(&options, input);
    assert_eq!(find(""), None);
    assert_eq!(find("comm"), Some(1));
    assert_eq!(find("DIR"), Some(2));
    assert_eq!(find("ary"), Some(1));
    assert_eq!(find("x"), None);
  }

  #[test]
  fn a_bcp_47_tag_maps_to_the_presets_of_its_primary_language() {
    assert_eq!(to_mkv_code("pt-BR"), "por");
    assert_eq!(to_mkv_code("zh-Hant"), "chi");
    assert_eq!(to_mkv_code("xx-BR"), "xx-BR");
  }

  /// What `mkvmerge --language` accepts (checked against v101.0): a known
  /// language, or a well-formed tag whose primary language is known.
  #[test]
  fn normalises_typed_languages_that_mkvmerge_accepts() {
    assert_eq!(normalize_language("ger").as_deref(), Some("de"));
    assert_eq!(normalize_language(" English ").as_deref(), Some("en"));
    assert_eq!(normalize_language("und").as_deref(), Some("und"));
    assert_eq!(normalize_language("pt-br").as_deref(), Some("pt-BR"));
    assert_eq!(normalize_language("ZH-HANT").as_deref(), Some("zh-Hant"));
    assert_eq!(normalize_language("es-419").as_deref(), Some("es-419"));
    assert_eq!(normalize_language("sr-latn-RS").as_deref(), Some("sr-Latn-RS"));
    assert_eq!(normalize_language("ger-DE").as_deref(), Some("de-DE"));
    assert_eq!(
      normalize_language("de-DE-u-co-PHONEBK").as_deref(),
      Some("de-DE-u-co-phonebk")
    );
    for rejected in ["", "  ", "Englsh", "german-ish", "xx-BR", "pt--BR", "Klingon"] {
      assert_eq!(normalize_language(rejected), None, "{rejected:?}");
    }
  }
}
