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

//! Spike-only language table, parsed from the React app's `mkvLanguages.ts`
//! at startup. Phase 3 replaces it with a generated `bmm-core::languages`.

use std::collections::HashMap;

/// The 186 real entries are padded with synthetic ones to this many options,
/// the size the plan sets for the autocomplete stress test.
pub const STRESS_OPTION_COUNT: usize = 700;

const SOURCE: &str = include_str!("../../../../src/mkvLanguages.ts");

pub struct LanguageTable {
  /// Canonical option values (short codes), in table order.
  pub codes: Vec<String>,
  label_by_code: HashMap<String, String>,
  short_by_any: HashMap<String, String>,
}

impl LanguageTable {
  pub fn load(pad_to: usize) -> Self {
    let mut table = Self {
      codes: Vec::new(),
      label_by_code: HashMap::new(),
      short_by_any: HashMap::new(),
    };
    for (code, label) in parse_entries(SOURCE) {
      table.add(&code, &label);
    }
    let mut synthetic = 1;
    while table.codes.len() < pad_to {
      let code = format!("x{synthetic:03}");
      table.add(&code, &format!("Synthetic language {synthetic:03} ({code})"));
      synthetic += 1;
    }
    table
  }

  fn add(&mut self, code: &str, label: &str) {
    let short = short_code_for(code, label);
    let display_name = label.rsplit_once(" (").map_or(label, |(name, _)| name).trim();
    let two_letter = two_letter_code(label).unwrap_or_default();
    for alias in [code, short.as_str(), two_letter.as_str(), display_name, label] {
      if !alias.is_empty() {
        self
          .short_by_any
          .entry(alias.to_lowercase())
          .or_insert_with(|| short.clone());
      }
    }
    self
      .label_by_code
      .entry(short.clone())
      .or_insert_with(|| label.to_string());
    self.label_by_code.insert(code.to_string(), label.to_string());
    if !self.codes.contains(&short) {
      self.codes.push(short);
    }
  }

  pub fn label(&self, code: &str) -> String {
    self
      .label_by_code
      .get(code)
      .cloned()
      .unwrap_or_else(|| code.to_string())
  }

  /// Any typed form (3-letter code, alpha-2, English name, full label) to the
  /// canonical short code; unknown text is returned trimmed.
  pub fn to_short_code(&self, value: &str) -> String {
    let trimmed = value.trim();
    self
      .short_by_any
      .get(&trimmed.to_lowercase())
      .cloned()
      .unwrap_or_else(|| trimmed.to_string())
  }

  /// Options for a track: the preferred languages first, then the rest.
  /// Returns the options and how many of them are preferred.
  pub fn options(&self, preferred: &str) -> (Vec<String>, usize) {
    let mut options: Vec<String> = Vec::new();
    for part in preferred.split(',').map(str::trim).filter(|part| !part.is_empty()) {
      let short = self.to_short_code(part);
      if !options.contains(&short) {
        options.push(short);
      }
    }
    let preferred_count = options.len();
    let rest: Vec<String> = self
      .codes
      .iter()
      .filter(|code| !options.contains(code))
      .cloned()
      .collect();
    options.extend(rest);
    (options, preferred_count)
  }

  /// Index of the option the dropdown highlights and scrolls to for `input`:
  /// an exact alias match first, else the first code prefix or label substring.
  pub fn first_matching_index(&self, options: &[String], input: &str) -> Option<usize> {
    let query = input.trim().to_lowercase();
    if query.is_empty() {
      return None;
    }
    if let Some(exact) = self.short_by_any.get(&query)
      && let Some(index) = options.iter().position(|option| option == exact)
    {
      return Some(index);
    }
    options.iter().position(|option| {
      option.to_lowercase().starts_with(&query) || self.label(option).to_lowercase().contains(&query)
    })
  }
}

fn parse_entries(source: &str) -> Vec<(String, String)> {
  let mut entries = Vec::new();
  let mut code: Option<String> = None;
  for line in source.lines() {
    let line = line.trim();
    if let Some(value) = json_string_value(line, "\"code\":") {
      code = Some(value);
    } else if let Some(value) = json_string_value(line, "\"label\":")
      && let Some(code) = code.take()
    {
      entries.push((code, value));
    }
  }
  entries
}

fn json_string_value(line: &str, key: &str) -> Option<String> {
  let rest = line.strip_prefix(key)?.trim().trim_end_matches(',');
  Some(rest.strip_prefix('"')?.strip_suffix('"')?.to_string())
}

fn two_letter_code(label: &str) -> Option<String> {
  let inner = label.strip_suffix(')')?.rsplit_once('(')?.1;
  let (two, three) = inner.split_once(';')?;
  let (two, three) = (two.trim(), three.trim());
  (two.len() == 2 && three.len() == 3 && two.chars().all(|c| c.is_ascii_alphabetic())).then(|| two.to_lowercase())
}

fn short_code_for(code: &str, label: &str) -> String {
  two_letter_code(label).unwrap_or_else(|| code.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_the_real_table_and_pads_it() {
    let real = LanguageTable::load(0);
    assert_eq!(real.codes.len(), 186);
    assert_eq!(real.codes[0], "und");
    assert_eq!(
      LanguageTable::load(STRESS_OPTION_COUNT).codes.len(),
      STRESS_OPTION_COUNT
    );
  }

  #[test]
  fn normalises_aliases_to_short_codes() {
    let table = LanguageTable::load(0);
    assert_eq!(table.to_short_code("ger"), "de");
    assert_eq!(table.to_short_code("German"), "de");
    assert_eq!(table.to_short_code(" eng "), "en");
    assert_eq!(table.to_short_code("und"), "und");
    assert_eq!(table.to_short_code("Klingon-ish"), "Klingon-ish");
  }

  #[test]
  fn preferred_languages_come_first_without_duplicates() {
    let table = LanguageTable::load(0);
    let (options, preferred) = table.options("ja, eng, jpn");
    assert_eq!(preferred, 2);
    assert_eq!(&options[..2], ["ja", "en"]);
    assert_eq!(options.len(), table.codes.len());
    assert_eq!(options.iter().filter(|code| *code == "en").count(), 1);
  }

  #[test]
  fn matches_exact_alias_then_prefix_then_label() {
    let table = LanguageTable::load(0);
    let (options, _) = table.options("");
    assert_eq!(table.first_matching_index(&options, ""), None);
    let german = table.first_matching_index(&options, "ger").unwrap();
    assert_eq!(options[german], "de");
    let by_label = table.first_matching_index(&options, "japan").unwrap();
    assert_eq!(options[by_label], "ja");
  }
}
