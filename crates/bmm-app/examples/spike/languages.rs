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

//! The spike's language options: `bmm_core::languages` padded with synthetic
//! entries to the stress-test size.

use std::collections::HashMap;

use bmm_core::config::ConfigProfile;
use bmm_core::languages;
use bmm_core::media_track::MediaTrackType;

/// The 186 real entries are padded with synthetic ones to this many options,
/// the size the plan sets for the autocomplete stress test.
pub const STRESS_OPTION_COUNT: usize = 700;

pub struct LanguageTable {
  /// Canonical option values (short codes), in table order.
  pub codes: Vec<String>,
  synthetic_labels: HashMap<String, String>,
}

impl LanguageTable {
  pub fn load(pad_to: usize) -> Self {
    let mut codes: Vec<String> = languages::all_short_codes()
      .iter()
      .map(|code| (*code).to_string())
      .collect();
    let mut synthetic_labels = HashMap::new();
    let mut synthetic = 1;
    while codes.len() < pad_to {
      let code = format!("x{synthetic:03}");
      synthetic_labels.insert(code.clone(), format!("Synthetic language {synthetic:03} ({code})"));
      codes.push(code);
      synthetic += 1;
    }
    Self {
      codes,
      synthetic_labels,
    }
  }

  pub fn label(&self, code: &str) -> String {
    match self.synthetic_labels.get(code) {
      Some(label) => label.clone(),
      None => languages::language_label(code).to_string(),
    }
  }

  /// Any typed form (3-letter code, alpha-2, English name, full label) to the
  /// canonical short code; unknown text is returned trimmed.
  pub fn to_short_code(&self, value: &str) -> String {
    languages::to_short_code(value)
  }

  /// Options for a track: the preferred languages first, then the rest.
  /// Returns the options and how many of them are preferred.
  pub fn options(&self, preferred: &str) -> (Vec<String>, usize) {
    let profile = ConfigProfile {
      preferred_audio_languages: preferred.to_string(),
      ..ConfigProfile::default()
    };
    let built = languages::build_language_options(Some(&profile), MediaTrackType::Audio);
    let mut options = built.options;
    options.extend(
      self
        .codes
        .iter()
        .filter(|code| self.synthetic_labels.contains_key(*code))
        .cloned(),
    );
    (options, built.preferred_count)
  }

  /// Index of the option the dropdown highlights and scrolls to for `input`.
  pub fn first_matching_index(&self, options: &[String], input: &str) -> Option<usize> {
    languages::first_matching_language_option_index(options, input)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn pads_the_real_table() {
    let real = LanguageTable::load(0);
    assert_eq!(real.codes.len(), 186);
    assert_eq!(real.codes[0], "und");
    let padded = LanguageTable::load(STRESS_OPTION_COUNT);
    assert_eq!(padded.codes.len(), STRESS_OPTION_COUNT);
    assert_eq!(padded.label("x001"), "Synthetic language 001 (x001)");
    assert_eq!(padded.label("de"), "German (de; ger)");
  }

  #[test]
  fn preferred_languages_come_first_without_duplicates() {
    let table = LanguageTable::load(STRESS_OPTION_COUNT);
    let (options, preferred) = table.options("ja, eng, jpn");
    assert_eq!(preferred, 2);
    assert_eq!(&options[..2], ["ja", "en"]);
    assert_eq!(options.len(), STRESS_OPTION_COUNT);
    assert_eq!(options.iter().filter(|code| *code == "en").count(), 1);
    let german = table.first_matching_index(&options, "ger").unwrap();
    assert_eq!(options[german], "de");
  }
}
