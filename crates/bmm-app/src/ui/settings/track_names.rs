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

//! The Track Names group (the Track Names tab of `Settings.tsx`): the
//! Video / Audio / Subtitles toggle, a filterable language list (the kind's
//! preferred languages first, a divider, then the rest; a language with
//! names in the success colour; ↑ / ↓ move through it once it has the
//! focus) and the names of the chosen language, one per line. Every
//! keystroke stores the names; an empty text removes the language.

use bmm_core::languages::MKV_LANGUAGES;
use gpui_kit::base::{TestSupportExt, TestTextExt};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{StreamKind, kind_toggle, parse_language_codes};
use crate::state::Stores;
use crate::state::config::active_profile;

/// The height of a list row.
pub const ROW_HEIGHT: f32 = 26.0;
/// The height of the list and of the names field.
pub const LIST_HEIGHT: f32 = 300.0;

/// The label of an MKV code; the code itself when unknown.
fn label(code: &str) -> String {
  MKV_LANGUAGES
    .iter()
    .find(|language| language.code == code)
    .map_or(code, |language| language.label)
    .to_string()
}

/// The list: the preferred codes that match `filter`, then every other MKV
/// code that matches (by label or code, case-insensitive). The second
/// number is how many are preferred.
pub fn visible_codes(preferred: &[String], filter: &str) -> (Vec<String>, usize) {
  let filter = filter.trim().to_lowercase();
  let matches = |code: &str| {
    filter.is_empty() || label(code).to_lowercase().contains(&filter) || code.to_lowercase().contains(&filter)
  };
  let mut codes: Vec<String> = preferred.iter().filter(|code| matches(code)).cloned().collect();
  let preferred_count = codes.len();
  codes.extend(
    MKV_LANGUAGES
      .iter()
      .map(|language| language.code)
      .filter(|code| !preferred.iter().any(|preferred| preferred == code) && matches(code))
      .map(str::to_string),
  );
  (codes, preferred_count)
}

pub struct TrackNamesEditor {
  stores: Stores,
  window: AnyWindowHandle,
  kind: StreamKind,
  filter: Entity<InputState>,
  /// The language whose names are shown.
  selected: Option<String>,
  names: Entity<TextareaState>,
  list_focus: FocusHandle,
  scroll: UniformListScrollHandle,
  /// What the names field shows: kind, profile, language and text.
  synced: (StreamKind, String, Option<String>, String),
  _subscriptions: Vec<Subscription>,
}

impl TrackNamesEditor {
  pub fn new(stores: Stores, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("settings.filter")));
    let names = cx.new(|cx| TextareaState::new(window, cx).placeholder(tr!("settings.trackNamesHint")));
    let subscriptions = vec![
      cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          cx.notify();
        }
      }),
      cx.subscribe(&names, |this, names, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          let text = names.read(cx).value().to_string();
          this.store_names(text, cx);
        }
      }),
      cx.observe_in(&stores.config, window, |this, _, window, cx| this.sync(window, cx)),
    ];
    let profile = active_profile(stores.config.read(cx).config()).map(|profile| profile.name.clone());
    Self {
      stores,
      window: window.window_handle(),
      kind: StreamKind::Audio,
      filter,
      selected: None,
      names,
      list_focus: cx.focus_handle(),
      scroll: UniformListScrollHandle::new(),
      synced: (StreamKind::Audio, profile.unwrap_or_default(), None, String::new()),
      _subscriptions: subscriptions,
    }
  }

  pub fn kind(&self) -> StreamKind {
    self.kind
  }

  pub fn selected(&self) -> Option<&str> {
    self.selected.as_deref()
  }

  pub fn filter(&self) -> &Entity<InputState> {
    &self.filter
  }

  pub fn names(&self) -> &Entity<TextareaState> {
    &self.names
  }

  pub fn list_focus(&self) -> &FocusHandle {
    &self.list_focus
  }

  /// The shown kind's names of the active profile for `code`.
  fn stored(&self, code: &str, cx: &App) -> String {
    active_profile(self.stores.config.read(cx).config())
      .and_then(|profile| self.kind.track_names(profile).get(code).cloned())
      .unwrap_or_default()
  }

  /// The list as shown, with the number of preferred rows.
  pub fn codes(&self, cx: &App) -> (Vec<String>, usize) {
    let preferred = active_profile(self.stores.config.read(cx).config())
      .map(|profile| parse_language_codes(self.kind.preferred_languages(profile)))
      .unwrap_or_default();
    visible_codes(&preferred, &self.filter.read(cx).value())
  }

  /// Stores the field's text as the selected language's names; an empty
  /// text removes the language.
  fn store_names(&mut self, text: String, cx: &mut Context<Self>) {
    let Some(code) = self.selected.clone() else {
      return;
    };
    self.synced.3 = text.clone();
    let kind = self.kind;
    self.stores.config.update(cx, |store, cx| {
      store.update_active_profile(cx, |profile| {
        let names = kind.track_names_mut(profile);
        if text.is_empty() {
          names.remove(&code);
        } else {
          names.insert(code, text);
        }
      })
    });
  }

  /// Puts the stored names of the selection into the field unless it shows
  /// them already or is being edited.
  fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let profile = active_profile(self.stores.config.read(cx).config())
      .map(|profile| profile.name.clone())
      .unwrap_or_default();
    let text = self
      .selected
      .as_deref()
      .map(|code| self.stored(code, cx))
      .unwrap_or_default();
    let next = (self.kind, profile, self.selected.clone(), text.clone());
    if next == self.synced {
      return;
    }
    let shown_elsewhere = next.0 != self.synced.0 || next.1 != self.synced.1 || next.2 != self.synced.2;
    self.synced = next;
    let focused = self.names.read(cx).focus_handle(cx).is_focused(window);
    if (shown_elsewhere || !focused) && self.names.read(cx).value() != text.as_str() {
      self.names.update(cx, |names, cx| names.set_value(text, window, cx));
    }
    cx.notify();
  }

  fn sync_now(&mut self, cx: &mut Context<Self>) {
    let window = self.window;
    let editor = cx.entity();
    cx.defer(move |cx| {
      let _ = cx.update_window(window, |_, window, cx| {
        editor.update(cx, |this, cx| this.sync(window, cx))
      });
    });
  }

  /// Chooses the language whose names are edited.
  pub fn select(&mut self, code: &str, cx: &mut Context<Self>) {
    if self.selected.as_deref() != Some(code) {
      self.selected = Some(code.to_string());
      self.sync_now(cx);
      cx.notify();
    }
  }

  /// Shows another kind (the toggle).
  pub fn set_kind(&mut self, kind: StreamKind, cx: &mut Context<Self>) {
    if kind != self.kind {
      self.kind = kind;
      self.sync_now(cx);
      cx.notify();
    }
  }

  /// ↑ / ↓ in the list: the previous or next language; from no selection
  /// the last or the first.
  pub fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
    let (codes, _) = self.codes(cx);
    if codes.is_empty() {
      return;
    }
    let current = self
      .selected
      .as_ref()
      .and_then(|code| codes.iter().position(|other| other == code));
    let next = match (current, down) {
      (None, true) => 0,
      (None, false) => codes.len() - 1,
      (Some(index), true) => (index + 1).min(codes.len() - 1),
      (Some(index), false) => index.saturating_sub(1),
    };
    self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
    let code = codes[next].clone();
    self.select(&code, cx);
  }
}

impl Render for TrackNamesEditor {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (codes, preferred_count) = self.codes(cx);
    let profile = active_profile(self.stores.config.read(cx).config()).cloned();
    let configured: Vec<bool> = codes
      .iter()
      .map(|code| {
        profile
          .as_ref()
          .and_then(|profile| self.kind.track_names(profile).get(code))
          .is_some_and(|names| !names.trim().is_empty())
      })
      .collect();
    let theme = cx.theme();
    let (border, success, selected_bg, hover) = (theme.border, theme.success, theme.list_active, theme.list_hover);
    let selected = self.selected.clone();
    let divider = preferred_count > 0 && preferred_count < codes.len();
    let editor = cx.entity().downgrade();
    let rows = codes.clone();
    let list = uniform_list(
      "track-names-list",
      codes.len(),
      cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
        range
          .filter_map(|index| rows.get(index).map(|code| (index, code)))
          .map(|(index, code)| {
            let text = label(code);
            let owned = code.clone();
            div()
              .id(SharedString::from(format!("track-names-{code}")))
              .test_support()
              .test_text(text.clone())
              .w_full()
              .h(px(ROW_HEIGHT))
              .px(px(8.0))
              .flex()
              .items_center()
              .text_sm()
              .truncate()
              .cursor_pointer()
              .when(divider && index == preferred_count, |row| {
                row.border_t_1().border_color(border)
              })
              .when(configured[index], |row| row.text_color(success))
              .when(selected.as_deref() == Some(code.as_str()), |row| row.bg(selected_bg))
              .hover(move |style| style.bg(hover))
              .on_click(cx.listener(move |this, _, window, cx| {
                this.select(&owned, cx);
                window.focus(&this.list_focus, cx);
              }))
              .child(text)
          })
          .collect::<Vec<_>>()
      }),
    )
    .track_scroll(&self.scroll)
    .size_full();

    v_flex()
      .id("track-names")
      .test_support()
      .w_full()
      .gap(px(8.0))
      .child(kind_toggle("track-names-kind", self.kind, move |kind, _, cx| {
        let _ = editor.update(cx, |editor, cx| editor.set_kind(kind, cx));
      }))
      // The filter above the list only, so the list and the names start on
      // the same line (React mirrored the filter with a hidden field).
      .child(
        h_flex()
          .w_full()
          .gap(px(8.0))
          .child(
            div()
              .flex_1()
              .min_w_0()
              .child(Input::new(&self.filter).small().cleanable(true).w_full()),
          )
          .child(div().flex_1().min_w_0()),
      )
      .child(
        h_flex()
          .w_full()
          .gap(px(8.0))
          .items_start()
          .child(
            v_flex().flex_1().min_w_0().child(
              div()
                .id("track-names-languages")
                .test_support()
                .track_focus(&self.list_focus)
                .h(px(LIST_HEIGHT))
                .border_1()
                .border_color(border)
                .rounded(px(4.0))
                .overflow_hidden()
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                  let down = match event.keystroke.key.as_str() {
                    "down" => true,
                    "up" => false,
                    _ => return,
                  };
                  cx.stop_propagation();
                  this.move_selection(down, cx);
                }))
                .child(list),
            ),
          )
          .child(
            div().flex_1().min_w_0().child(
              Textarea::new(&self.names)
                .small()
                .h(px(LIST_HEIGHT))
                .disabled(self.selected.is_none()),
            ),
          ),
      )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  #[test]
  fn preferred_codes_come_first_and_the_filter_applies_to_both_parts() {
    let preferred = vec!["jpn".to_string(), "eng".to_string()];
    let (codes, count) = visible_codes(&preferred, "");
    assert_eq!(count, 2);
    assert_eq!(&codes[..2], ["jpn", "eng"]);
    assert_eq!(codes.len(), MKV_LANGUAGES.len());
    assert!(!codes[2..].contains(&"eng".to_string()));
    let (codes, count) = visible_codes(&preferred, "JAPAN");
    assert_eq!((codes, count), (vec!["jpn".to_string()], 1));
    let (codes, count) = visible_codes(&preferred, "ger");
    assert_eq!(count, 0);
    assert!(codes.contains(&"ger".to_string()));
  }
}
