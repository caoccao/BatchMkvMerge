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

//! The Languages group: the Video / Audio / Subtitles toggle, a filter with
//! Add All and Remove All, and two panes: Available (every MKV language,
//! filtered, the preferred ones in the success colour) and Preferred (the
//! profile's ordered list). A row, or every checked row of its pane when it is
//! checked, is dragged with the mouse (GPUI's drag, with a chip showing the
//! first language and `+N`): from Available into Preferred inserts before the
//! row under the pointer or at the end, from Preferred onto Available removes,
//! within Preferred reorders. A 2 px line shows where the languages go. The
//! checkboxes do not start a drag. There is no keyboard drag.

use std::collections::HashSet;

use bmm_core::languages::{MKV_LANGUAGES, MkvLanguage};
use gpui_kit::base::{TestSupportExt, TestTextExt};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{StreamKind, kind_toggle, parse_language_codes};
use crate::state::Stores;
use crate::state::config::active_profile;
use crate::ui::checkbox::{select_all_state, tri_state_checkbox};
use crate::ui::text_button;

/// The height of a pane row.
pub const ROW_HEIGHT: f32 = 30.0;
/// The height of a pane's list.
pub const LIST_HEIGHT: f32 = 300.0;

/// The two panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
  Available,
  Preferred,
}

impl Pane {
  fn name(self) -> &'static str {
    match self {
      Self::Available => "available",
      Self::Preferred => "preferred",
    }
  }
}

/// The languages being dragged and the pane they come from; also the chip
/// that follows the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanguageDrag {
  pub from: Pane,
  pub codes: Vec<String>,
}

impl Render for LanguageDrag {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let first = self.codes.first().map(|code| label(code)).unwrap_or_default();
    h_flex()
      .gap(px(8.0))
      .px(px(8.0))
      .py(px(4.0))
      .rounded(theme.radius)
      .border_1()
      .border_color(theme.border)
      .bg(theme.background)
      .shadow_md()
      .text_sm()
      .child(first.to_string())
      .when(self.codes.len() > 1, |chip| {
        chip.child(
          div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(format!("+{}", self.codes.len() - 1)),
        )
      })
  }
}

/// The label of an MKV code; the code itself when unknown.
fn label(code: &str) -> &str {
  MKV_LANGUAGES
    .iter()
    .find(|language| language.code == code)
    .map_or(code, |language| language.label)
}

/// The languages matching `filter` by label or code (case-insensitive).
pub fn filter_languages(filter: &str) -> Vec<&'static MkvLanguage> {
  let filter = filter.trim().to_lowercase();
  MKV_LANGUAGES
    .iter()
    .filter(|language| {
      filter.is_empty() || language.label.to_lowercase().contains(&filter) || language.code.contains(&filter)
    })
    .collect()
}

/// The preferred list after dropping `drag` before `target` (an index in
/// `preferred`, the end when `None`); `None` when nothing changes.
pub fn drop_on_preferred(preferred: &[String], drag: &LanguageDrag, target: Option<usize>) -> Option<Vec<String>> {
  if drag.codes.is_empty() {
    return None;
  }
  let target = target.filter(|index| *index < preferred.len());
  match drag.from {
    Pane::Available => {
      let index = target.unwrap_or(preferred.len());
      let added: Vec<String> = drag
        .codes
        .iter()
        .filter(|code| !preferred.contains(code))
        .cloned()
        .collect();
      if added.is_empty() {
        return None;
      }
      let mut next = preferred[..index].to_vec();
      next.extend(added);
      next.extend_from_slice(&preferred[index..]);
      Some(next)
    }
    Pane::Preferred => {
      let index = match target {
        Some(index) if drag.codes.contains(&preferred[index]) => return None,
        Some(index) => index,
        None => preferred.len(),
      };
      let moved: HashSet<&String> = drag.codes.iter().collect();
      let remaining: Vec<String> = preferred.iter().filter(|code| !moved.contains(code)).cloned().collect();
      let before = preferred[..index].iter().filter(|code| moved.contains(code)).count();
      let index = index - before;
      let mut next = remaining[..index].to_vec();
      next.extend(drag.codes.iter().cloned());
      next.extend_from_slice(&remaining[index..]);
      (next != preferred).then_some(next)
    }
  }
}

/// The preferred list after dropping `drag` on the Available pane: a drag
/// from Preferred removes its languages.
pub fn drop_on_available(preferred: &[String], drag: &LanguageDrag) -> Option<Vec<String>> {
  if drag.from != Pane::Preferred {
    return None;
  }
  let next: Vec<String> = preferred
    .iter()
    .filter(|code| !drag.codes.contains(code))
    .cloned()
    .collect();
  (next.len() != preferred.len()).then_some(next)
}

/// The row a pointer at `y` is over in a list whose top is `top`, scrolled
/// by `scroll` (negative when scrolled down): the insertion index, `len` at
/// or below the last row.
fn index_at(y: Pixels, top: Pixels, scroll: Pixels, len: usize) -> usize {
  let offset = (y - top - scroll) / px(ROW_HEIGHT);
  if offset < 0.0 { 0 } else { (offset as usize).min(len) }
}

pub struct LanguagePicker {
  stores: Stores,
  kind: StreamKind,
  filter: Entity<InputState>,
  /// The checked rows of each pane.
  preferred_checked: HashSet<String>,
  available_checked: HashSet<String>,
  available_scroll: UniformListScrollHandle,
  preferred_scroll: ScrollHandle,
  /// Where a drag over the Preferred list would insert; `Some(len)` is the
  /// end.
  drop_index: Option<usize>,
  _subscriptions: Vec<Subscription>,
}

impl LanguagePicker {
  pub fn new(stores: Stores, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("settings.filter")));
    let subscriptions = vec![
      cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          cx.notify();
        }
      }),
      cx.observe(&stores.config, |_, _, cx| cx.notify()),
    ];
    Self {
      stores,
      kind: StreamKind::Audio,
      filter,
      preferred_checked: HashSet::new(),
      available_checked: HashSet::new(),
      available_scroll: UniformListScrollHandle::new(),
      preferred_scroll: ScrollHandle::new(),
      drop_index: None,
      _subscriptions: subscriptions,
    }
  }

  pub fn kind(&self) -> StreamKind {
    self.kind
  }

  pub fn filter(&self) -> &Entity<InputState> {
    &self.filter
  }

  /// The active profile's preferred list of the shown kind.
  pub fn preferred(&self, cx: &App) -> Vec<String> {
    active_profile(self.stores.config.read(cx).config())
      .map(|profile| parse_language_codes(self.kind.preferred_languages(profile)))
      .unwrap_or_default()
  }

  fn available(&self, cx: &App) -> Vec<&'static MkvLanguage> {
    filter_languages(&self.filter.read(cx).value())
  }

  fn set_preferred(&mut self, codes: Vec<String>, cx: &mut Context<Self>) {
    let kind = self.kind;
    self.stores.config.update(cx, |store, cx| {
      store.update_active_profile(cx, |profile| kind.set_preferred_languages(profile, codes.join(", ")))
    });
  }

  /// Shows another kind: a fresh filter and no checked rows.
  pub fn set_kind(&mut self, kind: StreamKind, window: &mut Window, cx: &mut Context<Self>) {
    if kind != self.kind {
      self.kind = kind;
      self.preferred_checked.clear();
      self.available_checked.clear();
      self.drop_index = None;
      self.filter.update(cx, |input, cx| input.set_value("", window, cx));
      cx.notify();
    }
  }

  /// Add All: every filtered language not yet preferred, at the end.
  pub fn add_all(&mut self, cx: &mut Context<Self>) {
    let mut preferred = self.preferred(cx);
    let before = preferred.len();
    for language in self.available(cx) {
      if !preferred.iter().any(|code| code == language.code) {
        preferred.push(language.code.to_string());
      }
    }
    if preferred.len() != before {
      self.set_preferred(preferred, cx);
    }
  }

  /// Remove All: an empty preferred list.
  pub fn remove_all(&mut self, cx: &mut Context<Self>) {
    if !self.preferred(cx).is_empty() {
      self.set_preferred(Vec::new(), cx);
    }
  }

  pub fn toggle(&mut self, pane: Pane, code: &str, checked: bool, cx: &mut Context<Self>) {
    let set = match pane {
      Pane::Available => &mut self.available_checked,
      Pane::Preferred => &mut self.preferred_checked,
    };
    if checked {
      set.insert(code.to_string());
    } else {
      set.remove(code);
    }
    cx.notify();
  }

  /// A pane header's box: checks or unchecks every row the pane shows.
  pub fn toggle_all(&mut self, pane: Pane, checked: bool, cx: &mut Context<Self>) {
    let codes: Vec<String> = match pane {
      Pane::Available => self
        .available(cx)
        .iter()
        .map(|language| language.code.to_string())
        .collect(),
      Pane::Preferred => self.preferred(cx),
    };
    let set = match pane {
      Pane::Available => &mut self.available_checked,
      Pane::Preferred => &mut self.preferred_checked,
    };
    for code in codes {
      if checked {
        set.insert(code);
      } else {
        set.remove(&code);
      }
    }
    cx.notify();
  }

  pub fn checked(&self, pane: Pane) -> &HashSet<String> {
    match pane {
      Pane::Available => &self.available_checked,
      Pane::Preferred => &self.preferred_checked,
    }
  }

  /// What dragging `code` takes along: every checked row of its pane when
  /// it is checked (in the pane's order), else the row alone.
  pub fn drag_set(&self, pane: Pane, code: &str, cx: &App) -> Vec<String> {
    let checked = self.checked(pane);
    if !checked.contains(code) {
      return vec![code.to_string()];
    }
    match pane {
      Pane::Preferred => self
        .preferred(cx)
        .into_iter()
        .filter(|code| checked.contains(code))
        .collect(),
      Pane::Available => MKV_LANGUAGES
        .iter()
        .filter(|language| checked.contains(language.code))
        .map(|language| language.code.to_string())
        .collect(),
    }
  }

  /// Drops `drag` on the Preferred list before row `target` (the end when
  /// `None`).
  pub fn drop_preferred(&mut self, drag: &LanguageDrag, target: Option<usize>, cx: &mut Context<Self>) {
    self.drop_index = None;
    if let Some(next) = drop_on_preferred(&self.preferred(cx), drag, target) {
      if drag.from == Pane::Available {
        self.available_checked.clear();
      }
      self.set_preferred(next, cx);
    }
    cx.notify();
  }

  /// Drops `drag` on the Available pane.
  pub fn drop_available(&mut self, drag: &LanguageDrag, cx: &mut Context<Self>) {
    self.drop_index = None;
    if let Some(next) = drop_on_available(&self.preferred(cx), drag) {
      self.preferred_checked.clear();
      self.set_preferred(next, cx);
    }
    cx.notify();
  }

  /// Follows a drag over the Preferred list for the drop line.
  fn track_drag(&mut self, event: &DragMoveEvent<LanguageDrag>, cx: &mut Context<Self>) {
    let position = event.event.position;
    let index = event.bounds.contains(&position).then(|| {
      index_at(
        position.y,
        event.bounds.top(),
        self.preferred_scroll.offset().y,
        self.preferred(cx).len(),
      )
    });
    if index != self.drop_index {
      self.drop_index = index;
      cx.notify();
    }
  }

  /// A pane's header: the box for every shown row and the title.
  fn header(&self, pane: Pane, codes: &[String], cx: &mut Context<Self>) -> impl IntoElement + use<> {
    let checked = self.checked(pane);
    let count = codes.iter().filter(|code| checked.contains(*code)).count();
    let title = match pane {
      Pane::Available => tr!("settings.available"),
      Pane::Preferred => tr!("settings.preferred"),
    };
    h_flex()
      .flex_shrink_0()
      .h(px(ROW_HEIGHT + 4.0))
      .border_b_1()
      .border_color(cx.theme().border)
      .child(div().w(px(40.0)).flex().justify_center().child(tri_state_checkbox(
        SharedString::from(format!("language-{}-all", pane.name())),
        select_all_state(count, codes.len()),
        codes.is_empty(),
        title.clone(),
        cx,
        {
          let picker = cx.entity().downgrade();
          move |checked, _, cx| {
            let _ = picker.update(cx, |this, cx| this.toggle_all(pane, checked, cx));
          }
        },
      )))
      .child(
        div()
          .px(px(8.0))
          .font_weight(FontWeight::SEMIBOLD)
          .text_sm()
          .child(title),
      )
  }

  /// One row: its box (which does not start a drag) and its label; with
  /// `line`, the drop line above it.
  fn row(&self, pane: Pane, code: &str, highlight: bool, line: bool, cx: &mut Context<Self>) -> AnyElement {
    let theme = cx.theme();
    let (border, hover, success, primary) = (theme.border, theme.list_hover, theme.success, theme.primary);
    let drag = LanguageDrag {
      from: pane,
      codes: self.drag_set(pane, code, cx),
    };
    let checked = self.checked(pane).contains(code);
    let owned = code.to_string();
    let text = label(code).to_string();
    div()
      .id(SharedString::from(format!("language-{}-{code}", pane.name())))
      .test_support()
      .test_text(text.clone())
      .relative()
      .w_full()
      .flex()
      .items_center()
      .h(px(ROW_HEIGHT))
      .flex_shrink_0()
      .border_b_1()
      .border_color(border)
      .cursor_grab()
      .hover(move |style| style.bg(hover))
      .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
      // A narrow pane truncates long names (600 × 450).
      .tooltip({
        let text = SharedString::from(text.clone());
        move |window, cx| Tooltip::new(text.clone()).build(window, cx)
      })
      .child(
        div()
          .w(px(40.0))
          .flex()
          .justify_center()
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            Checkbox::new(SharedString::from(format!("language-{}-check-{code}", pane.name())))
              .accessibility_label(text.clone())
              .checked(checked)
              .on_click(cx.listener(move |this, checked: &bool, _, cx| this.toggle(pane, &owned, *checked, cx))),
          ),
      )
      .child(
        div()
          .flex_1()
          .min_w_0()
          .px(px(8.0))
          .text_sm()
          .truncate()
          .when(highlight, |label| label.text_color(success))
          .child(text),
      )
      .when(line, |row| {
        row.child(div().absolute().top(px(-1.0)).left_0().right_0().h(px(2.0)).bg(primary))
      })
      .into_any_element()
  }
}

impl Render for LanguagePicker {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let preferred = self.preferred(cx);
    let preferred_set: HashSet<&String> = preferred.iter().collect();
    self.preferred_checked.retain(|code| preferred_set.contains(code));
    if !cx.has_active_drag() {
      self.drop_index = None;
    }
    let available: Vec<String> = self
      .available(cx)
      .iter()
      .map(|language| language.code.to_string())
      .collect();
    let preferred_codes: HashSet<String> = preferred.iter().cloned().collect();
    let theme = cx.theme();
    let (border, primary, selected, muted) = (theme.border, theme.primary, theme.list_active, theme.muted_foreground);
    let drop_index = self.drop_index;
    let picker = cx.entity().downgrade();

    let available_rows = available.clone();
    let available_list = uniform_list(
      "language-available-list",
      available.len(),
      cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
        range
          .filter_map(|index| available_rows.get(index))
          .map(|code| {
            let highlight = preferred_codes.contains(code);
            this.row(Pane::Available, code, highlight, false, cx)
          })
          .collect::<Vec<_>>()
      }),
    )
    .track_scroll(&self.available_scroll)
    .h(px(LIST_HEIGHT));

    let preferred_rows: Vec<AnyElement> = preferred
      .iter()
      .enumerate()
      .map(|(index, code)| self.row(Pane::Preferred, code, false, drop_index == Some(index), cx))
      .collect();
    let end_line = drop_index == Some(preferred.len());

    v_flex()
      .id("language-picker")
      .test_support()
      .w_full()
      .gap(px(8.0))
      .child(kind_toggle("language-kind", self.kind, {
        let picker = picker.clone();
        move |kind, window, cx| {
          let _ = picker.update(cx, |picker, cx| picker.set_kind(kind, window, cx));
        }
      }))
      .child(
        h_flex()
          .gap(px(8.0))
          .child(
            div()
              .flex_1()
              .child(Input::new(&self.filter).small().cleanable(true).w_full()),
          )
          .child(
            text_button("language-add-all")
              .outline()
              .small()
              .label(tr!("settings.addAll"))
              .disabled(available.is_empty())
              .on_click(cx.listener(|this, _, _, cx| this.add_all(cx))),
          )
          .child(
            text_button("language-remove-all")
              .outline()
              .small()
              .label(tr!("settings.removeAll"))
              .disabled(preferred.is_empty())
              .on_click(cx.listener(|this, _, _, cx| this.remove_all(cx))),
          ),
      )
      .child(
        h_flex()
          .w_full()
          .gap(px(8.0))
          .items_start()
          .child(
            v_flex()
              .id("language-available")
              .test_support()
              .flex_1()
              .min_w_0()
              .border_1()
              .border_color(border)
              .rounded(px(4.0))
              .overflow_hidden()
              .drag_over::<LanguageDrag>(move |style, drag, _, _| {
                if drag.from == Pane::Preferred {
                  style.bg(selected)
                } else {
                  style
                }
              })
              .on_drop(cx.listener(|this, drag: &LanguageDrag, _, cx| this.drop_available(drag, cx)))
              .child(self.header(Pane::Available, &available, cx))
              .child(available_list),
          )
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .border_1()
              .border_color(border)
              .rounded(px(4.0))
              .overflow_hidden()
              .child(self.header(Pane::Preferred, &preferred, cx))
              .child(
                v_flex()
                  .id("language-preferred")
                  .test_support()
                  .h(px(LIST_HEIGHT))
                  .overflow_y_scroll()
                  .track_scroll(&self.preferred_scroll)
                  .on_drag_move(
                    cx.listener(|this, event: &DragMoveEvent<LanguageDrag>, _, cx| this.track_drag(event, cx)),
                  )
                  .on_drop(cx.listener(|this, drag: &LanguageDrag, _, cx| {
                    let target = this.drop_index;
                    this.drop_preferred(drag, target, cx);
                  }))
                  .children(preferred_rows)
                  .when(end_line, |list| {
                    list.child(div().flex_shrink_0().h(px(2.0)).bg(primary))
                  }),
              ),
          ),
      )
      .child(
        div()
          .w_full()
          .text_center()
          .text_xs()
          .text_color(muted)
          .child(tr!("settings.languagesInstruction")),
      )
  }
}
