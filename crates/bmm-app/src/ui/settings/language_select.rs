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

//! The options of the Automation tab's und-language select: the profile's
//! preferred languages (all three lists, `buildCombinedLanguageOptions`)
//! first, a divider, then every other language, each by its full label
//! (`German (de; ger)`). Typing in the select's search box filters both
//! parts by label or code; the React Autocomplete listed every option
//! unfiltered (Q&A decision).

use bmm_core::config::ConfigProfile;
use bmm_core::languages::{build_combined_language_options, language_label};
use gpui_kit::component::searchable_list::{SearchableListDelegate, SearchableListItem};
use gpui_kit::component::{ActiveTheme, IndexPath};
use gpui_kit::*;

/// One language: its short code and its full label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageChoice {
  code: SharedString,
  label: SharedString,
}

impl LanguageChoice {
  fn new(code: &str) -> Self {
    Self {
      code: code.to_string().into(),
      label: language_label(code).to_string().into(),
    }
  }
}

impl SearchableListItem for LanguageChoice {
  type Value = SharedString;

  fn title(&self) -> SharedString {
    self.label.clone()
  }

  fn value(&self) -> &SharedString {
    &self.code
  }

  fn matches(&self, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || self.label.to_lowercase().contains(&query) || self.code.to_lowercase().contains(&query)
  }
}

/// The two parts, each with the indices of the options matching the search.
pub struct LanguageChoices {
  parts: [Vec<LanguageChoice>; 2],
  matched: [Vec<usize>; 2],
}

impl LanguageChoices {
  pub fn new(profile: Option<&ConfigProfile>) -> Self {
    let options = build_combined_language_options(profile);
    let (preferred, rest) = options.options.split_at(options.preferred_count);
    let parts = [
      preferred
        .iter()
        .map(|code| LanguageChoice::new(code))
        .collect::<Vec<_>>(),
      rest.iter().map(|code| LanguageChoice::new(code)).collect::<Vec<_>>(),
    ];
    let matched = [(0..parts[0].len()).collect(), (0..parts[1].len()).collect()];
    Self { parts, matched }
  }

  /// How many options are preferred.
  pub fn preferred_count(&self) -> usize {
    self.parts[0].len()
  }
}

impl SearchableListDelegate for LanguageChoices {
  type Item = LanguageChoice;

  fn sections_count(&self, _: &App) -> usize {
    2
  }

  fn items_count(&self, section: usize) -> usize {
    self.matched.get(section).map_or(0, Vec::len)
  }

  fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
    let index = *self.matched.get(ix.section)?.get(ix.row)?;
    self.parts[ix.section].get(index)
  }

  fn position<V>(&self, value: &V) -> Option<IndexPath>
  where
    Self::Item: SearchableListItem<Value = V>,
    V: PartialEq,
  {
    (0..2).find_map(|section| {
      self.matched[section]
        .iter()
        .position(|&index| self.parts[section][index].value() == value)
        .map(|row| IndexPath::default().section(section).row(row))
    })
  }

  fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
    for section in 0..2 {
      self.matched[section] = (0..self.parts[section].len())
        .filter(|&index| self.parts[section][index].matches(query))
        .collect();
    }
    Task::ready(())
  }

  /// The divider before the other languages, when both parts show options.
  fn render_section_header(&self, section: usize, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
    (section == 1 && !self.matched[0].is_empty() && !self.matched[1].is_empty()).then(|| {
      div()
        .w_full()
        .my(px(4.0))
        .h(px(1.0))
        .bg(cx.theme().border)
        .into_any_element()
    })
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  #[test]
  fn preferred_languages_come_first() {
    let choices = LanguageChoices::new(Some(&ConfigProfile::default()));
    // eng, chi, spa, ger, fre, jpn as short codes.
    assert_eq!(choices.preferred_count(), 6);
    let first = choices.item(IndexPath::default().section(0).row(0)).unwrap();
    assert_eq!(first.code.as_ref(), "en");
    assert_eq!(first.label.as_ref(), language_label("en"));
    assert_eq!(
      choices.items_count(1),
      bmm_core::languages::all_short_codes().len() - choices.preferred_count()
    );
    assert_eq!(
      choices.position(&SharedString::from("de")),
      Some(IndexPath::default().section(0).row(3))
    );
  }

  #[test]
  fn a_choice_matches_its_label_or_code() {
    let german = LanguageChoice::new("de");
    assert!(german.matches("GERM"));
    assert!(german.matches("de"));
    assert!(german.matches(""));
    assert!(!german.matches("french"));
  }
}
