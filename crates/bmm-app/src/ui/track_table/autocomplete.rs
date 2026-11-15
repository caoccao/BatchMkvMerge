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

//! The free-text cells of the track table: the language cell and the
//! track-name cell.
//!
//! Every option is listed (typing filters nothing): the language cell shows
//! the profile's preferred languages first, then a divider and every other
//! language; the name cell shows the presets of the row's language. The first
//! option matching the text is highlighted and scrolled to the middle, the
//! list opens below the cell unless there is more room above, a click or a
//! focus selects the whole text, and every keystroke reports the text.
//! Keyboard as in a combo box: ↓/↑ open the list and move through it, Enter
//! takes the keyboard option or the typed text (an alias such as `ger` becomes
//! its short code `de` first), Escape closes the list. gpui-kit's `Combobox`
//! is not used: it does not take free text.

use std::ops::Range;
use std::rc::Rc;

use bmm_core::languages::{
  first_matching_language_option_index, first_matching_title_option_index, language_label, to_short_code,
};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const OPTION_ROW_HEIGHT: f32 = 34.0;
pub const OPTION_VISIBLE_ROWS: usize = 10;
/// The list's width: its widest option, at least this and at most the window
/// less [`LIST_WINDOW_INSET`].
const LIST_MIN_WIDTH: f32 = 280.0;
const LIST_WINDOW_INSET: f32 = 32.0;
const OPTION_PADDING_X: f32 = 12.0;
const VIEWPORT_MARGIN: f32 = 8.0;

/// Which column the cell edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellKind {
  Language,
  TrackName,
}

impl CellKind {
  fn name(self) -> &'static str {
    match self {
      Self::Language => "language",
      Self::TrackName => "name",
    }
  }
}

/// Where the option list opens relative to the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
  Below,
  Above,
}

/// `useDropdownBehaviour.updatePlacement`: below when the list fits there,
/// or when there is at least as much room below as above.
pub fn placement_for(cell_top: f32, cell_bottom: f32, viewport_height: f32, option_count: usize) -> Placement {
  let list_height = option_count.min(OPTION_VISIBLE_ROWS) as f32 * OPTION_ROW_HEIGHT;
  let below = viewport_height - cell_bottom - VIEWPORT_MARGIN;
  let above = cell_top - VIEWPORT_MARGIN;
  if below >= list_height || below >= above {
    Placement::Below
  } else {
    Placement::Above
  }
}

/// The first row to show so that `matching` sits in the middle.
pub fn first_visible_row(matching: usize, option_count: usize) -> usize {
  let visible = option_count.min(OPTION_VISIBLE_ROWS);
  matching.saturating_sub(visible / 2)
}

/// The keyboard option after ↓ (`down`) or ↑ from `current`, wrapping around;
/// from none, ↓ takes the first and ↑ the last.
pub fn step_option(current: Option<usize>, count: usize, down: bool) -> Option<usize> {
  if count == 0 {
    return None;
  }
  Some(match (current, down) {
    (None, true) => 0,
    (None, false) => count - 1,
    (Some(index), true) => (index + 1) % count,
    (Some(index), false) => (index + count - 1) % count,
  })
}

/// The text of the cell changed: on every keystroke the typed text, after
/// Enter or a click on an option the committed value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellEdited(pub String);

pub struct AutocompleteCell {
  /// Element id prefix, unique in the table: `{kind}-{row key}`.
  id: SharedString,
  kind: CellKind,
  input: Entity<InputState>,
  options: Rc<Vec<String>>,
  preferred_count: usize,
  /// The option matching the text: highlighted, and scrolled to.
  matching: Option<usize>,
  /// The keyboard option: the one equal to the text, then moved by ↓/↑.
  highlighted: Option<usize>,
  open: bool,
  placement: Placement,
  list_width: Pixels,
  cell_bounds: Bounds<Pixels>,
  scroll: UniformListScrollHandle,
  disabled: bool,
  _subscription: Subscription,
}

impl EventEmitter<CellEdited> for AutocompleteCell {}

impl AutocompleteCell {
  pub fn new(
    kind: CellKind,
    row_key: &str,
    value: &str,
    options: Rc<Vec<String>>,
    preferred_count: usize,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let input = cx.new(|cx| {
      let mut state = InputState::new(window, cx);
      state.set_value(value.to_string(), window, cx);
      state
    });
    let subscription = cx.subscribe_in(&input, window, Self::on_input_event);
    let mut cell = Self {
      id: Self::element_id(kind, row_key),
      kind,
      input,
      options,
      preferred_count,
      matching: None,
      highlighted: None,
      open: false,
      placement: Placement::Below,
      list_width: px(LIST_MIN_WIDTH),
      cell_bounds: Bounds::default(),
      scroll: UniformListScrollHandle::new(),
      disabled: false,
      _subscription: subscription,
    };
    cell.sync_matches(value);
    cell
  }

  /// The cell's element id; its input's is this with `-input` appended.
  pub fn element_id(kind: CellKind, row_key: &str) -> SharedString {
    format!("{}-{row_key}", kind.name()).into()
  }

  pub fn kind(&self) -> CellKind {
    self.kind
  }

  pub fn value(&self, cx: &App) -> SharedString {
    self.input.read(cx).value()
  }

  pub fn is_open(&self) -> bool {
    self.open
  }

  pub fn placement(&self) -> Placement {
    self.placement
  }

  pub fn matching(&self) -> Option<usize> {
    self.matching
  }

  pub fn highlighted(&self) -> Option<usize> {
    self.highlighted
  }

  pub fn options(&self) -> &[String] {
    &self.options
  }

  pub fn preferred_count(&self) -> usize {
    self.preferred_count
  }

  pub fn list_width(&self) -> Pixels {
    self.list_width
  }

  pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
    self.input.read(cx).focus_handle(cx).is_focused(window)
  }

  /// Shows a value changed elsewhere (another selected row, the
  /// automation), without reporting it back. A focused cell is left alone:
  /// it is the one being edited, and its edits reach the rows only after
  /// the keystroke's effects run, so a frame drawn in between would undo
  /// the typing. Nothing else edits a focused cell's row.
  pub fn sync_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
    if self.is_focused(window, cx) {
      return;
    }
    if self.input.read(cx).value().as_ref() != value {
      self
        .input
        .update(cx, |state, cx| state.set_value(value.to_string(), window, cx));
      self.sync_matches(value);
      cx.notify();
    }
  }

  /// Replaces the options (the profile's preferred languages or the
  /// language's presets changed).
  pub fn set_options(&mut self, options: Rc<Vec<String>>, preferred_count: usize, cx: &mut Context<Self>) {
    if !Rc::ptr_eq(&self.options, &options) && (*self.options != *options || self.preferred_count != preferred_count) {
      self.options = options;
      self.preferred_count = preferred_count;
      let value = self.input.read(cx).value();
      self.sync_matches(&value);
      if self.options.is_empty() {
        self.open = false;
      }
      cx.notify();
    }
  }

  /// A cell of a row being merged takes no input.
  pub fn set_disabled(&mut self, disabled: bool, window: &mut Window, cx: &mut Context<Self>) {
    if self.disabled != disabled {
      self.disabled = disabled;
      self.input.update(cx, |state, cx| state.set_disabled(disabled, cx));
      if disabled {
        self.open = false;
        if self.is_focused(window, cx) {
          window.blur(cx);
        }
      }
      cx.notify();
    }
  }

  fn on_input_event(
    &mut self,
    input: &Entity<InputState>,
    event: &InputEvent,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match event {
      InputEvent::Focus => {
        input.update(cx, |state, cx| state.select_all(window, cx));
      }
      InputEvent::Change => {
        let value = input.read(cx).value().to_string();
        self.sync_matches(&value);
        if !self.open {
          self.open(window, cx);
        }
        self.scroll_to_match();
        cx.notify();
        cx.emit(CellEdited(value));
      }
      InputEvent::PressEnter { .. } => {}
      InputEvent::Blur => self.close(cx),
    }
  }

  /// The matching option, and the keyboard option equal to `value` (kept in
  /// step with the value on every change).
  fn sync_matches(&mut self, value: &str) {
    self.matching = match self.kind {
      CellKind::Language => first_matching_language_option_index(&self.options, value),
      CellKind::TrackName => first_matching_title_option_index(&self.options, value),
    };
    self.highlighted = self.options.iter().position(|option| option == value);
  }

  fn label(&self, option: &str) -> SharedString {
    match self.kind {
      CellKind::Language => SharedString::new(language_label(option)),
      CellKind::TrackName => SharedString::from(option.to_string()),
    }
  }

  fn update_placement(&mut self, window: &Window) {
    self.placement = placement_for(
      self.cell_bounds.top().as_f32(),
      self.cell_bounds.bottom().as_f32(),
      window.viewport_size().height.as_f32(),
      self.options.len(),
    );
  }

  /// The widest option label, within the list's limits.
  fn measure_list_width(&mut self, window: &Window) {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let widest = self
      .options
      .iter()
      .map(|option| {
        let label = self.label(option);
        let run = style.to_run(label.len());
        window.text_system().shape_line(label, font_size, &[run], None).width
      })
      .max()
      .unwrap_or_default();
    let max = (window.viewport_size().width - px(LIST_WINDOW_INSET)).max(px(LIST_MIN_WIDTH));
    self.list_width = (widest + px(2.0 * OPTION_PADDING_X + 16.0)).clamp(px(LIST_MIN_WIDTH), max);
  }

  /// Called from prepaint with the cell's bounds. A cell opened in the frame
  /// it first appears in has none yet, so the placement follows them.
  fn set_cell_bounds(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
    if self.cell_bounds != bounds {
      self.cell_bounds = bounds;
      if self.open {
        self.update_placement(window);
        cx.notify();
      }
    }
  }

  fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.disabled || self.options.is_empty() {
      return;
    }
    self.update_placement(window);
    self.measure_list_width(window);
    self.open = true;
    self.scroll_to_match();
    cx.notify();
  }

  fn close(&mut self, cx: &mut Context<Self>) {
    if self.open {
      self.open = false;
      cx.notify();
    }
  }

  fn scroll_to_match(&self) {
    if let Some(matching) = self.matching {
      self
        .scroll
        .scroll_to_item_strict(first_visible_row(matching, self.options.len()), ScrollStrategy::Top);
    }
  }

  /// ↓ / ↑: open the list, or move the keyboard option.
  fn step(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
    if !self.open {
      self.open(window, cx);
      return;
    }
    self.highlighted = step_option(self.highlighted, self.options.len(), down);
    if let Some(index) = self.highlighted {
      self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
    }
    cx.notify();
  }

  /// Enter: an alias of a listed language becomes its short code; otherwise
  /// the keyboard option, else the typed text (free solo; the language cell
  /// normalises it, the name cell keeps it as typed).
  fn press_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let typed = self.input.read(cx).value().to_string();
    let committed = match self.kind {
      CellKind::Language => {
        let normalized = to_short_code(&typed);
        if normalized != typed && self.options.contains(&normalized) {
          normalized
        } else if let Some(option) = self.open_option() {
          option
        } else {
          normalized
        }
      }
      CellKind::TrackName => self.open_option().unwrap_or(typed),
    };
    self.commit(committed, window, cx);
  }

  /// The keyboard option while the list is open.
  fn open_option(&self) -> Option<String> {
    self
      .open
      .then(|| self.highlighted.and_then(|index| self.options.get(index).cloned()))
      .flatten()
  }

  fn commit(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
    if self.input.read(cx).value().as_ref() != value {
      self
        .input
        .update(cx, |state, cx| state.set_value(value.clone(), window, cx));
    }
    self.sync_matches(&value);
    self.close(cx);
    cx.emit(CellEdited(value));
  }

  fn commit_option(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(option) = self.options.get(index).cloned() {
      self.commit(option, window, cx);
    }
  }

  fn render_options(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let theme = cx.theme();
    let (border, selected, hover) = (theme.border, theme.list_active, theme.list_hover);
    range
      .map(|index| {
        let divider = self.preferred_count > 0 && index == self.preferred_count;
        let matched = self.matching == Some(index);
        let keyboard = self.highlighted == Some(index);
        div()
          .id(("option", index))
          .test_support()
          .w_full()
          .h(px(OPTION_ROW_HEIGHT))
          .px(px(OPTION_PADDING_X))
          .flex()
          .items_center()
          .whitespace_nowrap()
          .overflow_hidden()
          .text_ellipsis()
          .when(divider, |row| row.border_t_1().border_color(border))
          .when(keyboard && !matched, |row| row.bg(hover))
          .when(matched, |row| row.bg(selected))
          .hover(move |style| style.bg(if matched { selected } else { hover }))
          .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
              cx.stop_propagation();
              this.commit_option(index, window, cx);
            }),
          )
          .child(self.label(&self.options[index]))
          .into_any_element()
      })
      .collect()
  }
}

impl Render for AutocompleteCell {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let entity = cx.entity();
    let focused = self.is_focused(window, cx);
    let theme = cx.theme();
    let underline = if self.disabled {
      theme.border
    } else if focused {
      theme.primary
    } else {
      theme.muted_foreground.opacity(0.6)
    };
    let dropdown = self.open.then(|| {
      let rows = self.options.len().min(OPTION_VISIBLE_ROWS);
      let (anchor, position) = match self.placement {
        Placement::Below => (Anchor::TopLeft, self.cell_bounds.bottom_left()),
        Placement::Above => (Anchor::BottomLeft, self.cell_bounds.origin),
      };
      deferred(
        anchored()
          .anchor(anchor)
          .position(position)
          .snap_to_window_with_margin(px(VIEWPORT_MARGIN))
          .child(
            div()
              .id(SharedString::from(format!("{}-list", self.id)))
              .test_support()
              .occlude()
              .w(self.list_width)
              .py(px(4.0))
              .bg(theme.popover)
              .text_color(theme.popover_foreground)
              .border_1()
              .border_color(theme.border)
              .rounded_md()
              .shadow_md()
              .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
              .child(
                uniform_list(
                  SharedString::from(format!("{}-options", self.id)),
                  self.options.len(),
                  cx.processor(|this, range, _window, cx| this.render_options(range, cx)),
                )
                .track_scroll(&self.scroll)
                .h(px(rows as f32 * OPTION_ROW_HEIGHT)),
              ),
          ),
      )
      .with_priority(1)
    });

    div()
      .id(self.id.clone())
      .relative()
      .w_full()
      .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
        cx.stop_propagation();
        this.step(true, window, cx);
      }))
      .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
        cx.stop_propagation();
        this.step(false, window, cx);
      }))
      .capture_action(cx.listener(|this, _: &Enter, window, cx| {
        cx.stop_propagation();
        this.press_enter(window, cx);
      }))
      .capture_action(cx.listener(|this, _: &Escape, _, cx| {
        if this.open {
          cx.stop_propagation();
          this.close(cx);
        }
      }))
      .on_click(cx.listener(|this, _, window, cx| {
        if this.disabled {
          return;
        }
        this.input.update(cx, |state, cx| state.select_all(window, cx));
        if !this.open {
          this.open(window, cx);
        }
      }))
      .child(
        Input::new(&self.input)
          .id(SharedString::from(format!("{}-input", self.id)))
          .appearance(false)
          .small()
          .disabled(self.disabled),
      )
      .child(
        div()
          .absolute()
          .left_0()
          .right_0()
          .bottom_0()
          .h(px(if focused { 2.0 } else { 1.0 }))
          .bg(underline),
      )
      .child(
        canvas(
          move |bounds, window, cx| entity.update(cx, |cell, cx| cell.set_cell_bounds(bounds, window, cx)),
          |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
      )
      .children(dropdown)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  #[test]
  fn opens_below_when_the_list_fits_or_below_is_larger() {
    // 10 rows × 34 px = 340 px list.
    assert_eq!(placement_for(100.0, 130.0, 800.0, 186), Placement::Below);
    assert_eq!(placement_for(700.0, 730.0, 800.0, 186), Placement::Above);
    // Neither side fits: the larger side wins, ties go below.
    assert_eq!(placement_for(200.0, 230.0, 430.0, 186), Placement::Below);
    assert_eq!(placement_for(260.0, 290.0, 430.0, 186), Placement::Above);
    // A short list fits below even near the bottom.
    assert_eq!(placement_for(650.0, 680.0, 800.0, 2), Placement::Below);
  }

  #[test]
  fn centres_the_matching_row() {
    assert_eq!(first_visible_row(0, 186), 0);
    assert_eq!(first_visible_row(3, 186), 0);
    assert_eq!(first_visible_row(50, 186), 45);
    assert_eq!(first_visible_row(5, 3), 4);
  }

  #[test]
  fn the_keyboard_option_wraps_around() {
    assert_eq!(step_option(None, 0, true), None);
    assert_eq!(step_option(None, 5, true), Some(0));
    assert_eq!(step_option(None, 5, false), Some(4));
    assert_eq!(step_option(Some(4), 5, true), Some(0));
    assert_eq!(step_option(Some(0), 5, false), Some(4));
    assert_eq!(step_option(Some(2), 5, true), Some(3));
  }
}
