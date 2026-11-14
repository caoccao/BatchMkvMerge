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

//! Prototype 3: the free-text language autocomplete cell.
//!
//! Ports the behaviour of `TrackCellAutocomplete.tsx`: every option is listed
//! (no filtering), preferred languages first with a divider, the first
//! matching option highlighted and scrolled to the middle, the list opening
//! below the cell unless there is more room above, select-all on focus, and
//! Enter committing free text after normalising an alias to its short code.
//! gpui-kit's `Combobox` is not used because it does not accept free text.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::languages::LanguageTable;

pub const OPTION_ROW_HEIGHT: f32 = 34.0;
pub const OPTION_VISIBLE_ROWS: usize = 10;
const DROPDOWN_WIDTH: f32 = 320.0;
const VIEWPORT_MARGIN: f32 = 8.0;

/// Where the option list opens relative to the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
  Below,
  Above,
}

/// Same rule as `useDropdownBehaviour.updatePlacement`: below when the list
/// fits there, or when there is at least as much room below as above.
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

/// First option row to show so the matching option sits in the middle.
pub fn first_visible_row(matching: usize, option_count: usize) -> usize {
  let visible = option_count.min(OPTION_VISIBLE_ROWS);
  matching.saturating_sub(visible / 2)
}

pub struct AutocompleteCell {
  id: SharedString,
  input: Entity<InputState>,
  table: Rc<LanguageTable>,
  options: Rc<Vec<String>>,
  preferred_count: usize,
  matching: Option<usize>,
  open: bool,
  placement: Placement,
  cell_bounds: Bounds<Pixels>,
  scroll: UniformListScrollHandle,
  /// The value the cell reports to its owner (what React's `onChange` gets).
  pub value: SharedString,
  _subscription: Subscription,
}

impl AutocompleteCell {
  pub fn new(
    id: impl Into<SharedString>,
    table: Rc<LanguageTable>,
    preferred: &str,
    initial: &str,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let input = cx.new(|cx| {
      let mut state = InputState::new(window, cx);
      state.set_value(initial.to_string(), window, cx);
      state
    });
    let subscription = cx.subscribe_in(&input, window, Self::on_input_event);
    let (options, preferred_count) = table.options(preferred);
    let mut cell = Self {
      id: id.into(),
      input,
      table,
      options: Rc::new(options),
      preferred_count,
      matching: None,
      open: false,
      placement: Placement::Below,
      cell_bounds: Bounds::default(),
      scroll: UniformListScrollHandle::new(),
      value: initial.to_string().into(),
      _subscription: subscription,
    };
    cell.matching = cell.table.first_matching_index(&cell.options, initial);
    cell
  }

  pub fn is_open(&self) -> bool {
    self.open
  }

  pub fn placement(&self) -> Placement {
    self.placement
  }

  pub fn debug_bounds(&self) -> Bounds<Pixels> {
    self.cell_bounds
  }

  pub fn matching(&self) -> Option<usize> {
    self.matching
  }

  pub fn options(&self) -> &[String] {
    &self.options
  }

  pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
    self.input.update(cx, |state, cx| state.focus(window, cx));
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
        self.open(window, cx);
      }
      InputEvent::Change => {
        self.value = input.read(cx).value();
        self.matching = self.table.first_matching_index(&self.options, &self.value);
        if !self.open {
          self.open(window, cx);
        }
        self.scroll_to_match();
        cx.notify();
      }
      InputEvent::PressEnter { .. } => {
        self.commit_typed(window, cx);
      }
      InputEvent::Blur => {
        self.close(cx);
      }
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

  /// Called from prepaint with the cell's measured bounds. A cell focused in
  /// the frame it first appears in has no bounds yet when it opens, so the
  /// placement is recomputed whenever the bounds change while open.
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
    self.update_placement(window);
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

  /// Enter: an alias that normalises to a listed option becomes its short
  /// code; any other text is kept as typed (free solo).
  fn commit_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let typed = self.input.read(cx).value().to_string();
    let normalized = self.table.to_short_code(&typed);
    if normalized != typed && self.options.contains(&normalized) {
      self.set_value(normalized, window, cx);
    } else {
      self.value = typed.trim().to_string().into();
    }
    self.close(cx);
  }

  fn commit_option(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(code) = self.options.get(index).cloned() {
      self.set_value(code, window, cx);
    }
    self.close(cx);
  }

  fn set_value(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
    self.value = value.clone().into();
    self.matching = self.table.first_matching_index(&self.options, &value);
    self.input.update(cx, |state, cx| state.set_value(value, window, cx));
    cx.notify();
  }

  fn render_options(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
    let theme = cx.theme();
    let (border, active, hover) = (theme.border, theme.list_active, theme.list_hover);
    range
      .map(|index| {
        let divider = self.preferred_count > 0 && index == self.preferred_count && index < self.options.len();
        let matched = self.matching == Some(index);
        let label = self.table.label(&self.options[index]);
        div()
          .id(("option", index))
          .w_full()
          .h(px(OPTION_ROW_HEIGHT))
          .px_3()
          .flex()
          .items_center()
          .when(divider, |row| row.border_t_1().border_color(border))
          .when(matched, |row| row.bg(active))
          .hover(move |style| style.bg(hover))
          .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
              cx.stop_propagation();
              this.commit_option(index, window, cx);
            }),
          )
          .child(label)
          .test_support()
          .into_any_element()
      })
      .collect()
  }
}

impl Render for AutocompleteCell {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let entity = cx.entity();
    let rows = self.options.len().min(OPTION_VISIBLE_ROWS);
    let list_height = px(rows as f32 * OPTION_ROW_HEIGHT);
    let dropdown = self.open.then(|| {
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
              .occlude()
              .w(px(DROPDOWN_WIDTH))
              .bg(cx.theme().popover)
              .border_1()
              .border_color(cx.theme().border)
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
                .h(list_height),
              ),
          ),
      )
      .with_priority(1)
    });

    div()
      .id(self.id.clone())
      .relative()
      .w(px(180.0))
      .child(Input::new(&self.input).id(SharedString::from(format!("{}-input", self.id))))
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

/// Demo panel: three cells at the top (the list opens below) and three at
/// the bottom of the window (it flips above).
pub struct AutocompleteDemo {
  pub top: Vec<Entity<AutocompleteCell>>,
  pub bottom: Vec<Entity<AutocompleteCell>>,
}

impl AutocompleteDemo {
  pub fn new(table: Rc<LanguageTable>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let make = |id: &str, preferred: &str, initial: &str, window: &mut Window, cx: &mut Context<Self>| {
      let table = table.clone();
      cx.new(|cx| AutocompleteCell::new(id.to_string(), table, preferred, initial, window, cx))
    };
    let top = vec![
      make("video-top", "ja, en", "ja", window, cx),
      make("audio-top", "ja, en", "eng", window, cx),
      make("subtitle-top", "en, de", "", window, cx),
    ];
    let bottom = vec![
      make("video-bottom", "ja, en", "und", window, cx),
      make("audio-bottom", "ja, en", "German", window, cx),
      make("subtitle-bottom", "en, de", "zh", window, cx),
    ];
    Self { top, bottom }
  }
}

impl Render for AutocompleteDemo {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let values = |cells: &[Entity<AutocompleteCell>], cx: &App| {
      cells
        .iter()
        .map(|cell| format!("[{}]", cell.read(cx).value))
        .collect::<Vec<_>>()
        .join(" ")
    };
    let top_values = values(&self.top, cx);
    let bottom_values = values(&self.bottom, cx);
    v_flex()
      .size_full()
      .p_4()
      .justify_between()
      .child(
        v_flex()
          .gap_2()
          .child("Language cells (preferred first, divider, match highlight, Enter normalises).")
          .child(h_flex().gap_2().children(self.top.iter().cloned()))
          .child(
            div()
              .text_color(cx.theme().muted_foreground)
              .child(format!("Values: {top_values}")),
          ),
      )
      .child(
        v_flex()
          .gap_2()
          .child(
            div()
              .text_color(cx.theme().muted_foreground)
              .child(format!("Values: {bottom_values}")),
          )
          .child(h_flex().gap_2().children(self.bottom.iter().cloned())),
      )
  }
}

#[cfg(test)]
mod tests {
  // `gpui_kit::*` also exports GPUI's `test` macro; name the built-in one.
  use super::*;
  use core::prelude::v1::test;

  use gpui_kit::test::TestWindowExt;

  fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<AutocompleteDemo>) {
    let opened = cx.update(|cx| {
      gpui_kit::init(cx);
      let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
          Point::default(),
          size(px(900.0), px(700.0)),
        ))),
        ..Default::default()
      };
      let table = Rc::new(LanguageTable::load(crate::languages::STRESS_OPTION_COUNT));
      gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| AutocompleteDemo::new(table, window, cx))
      })
      .expect("open window")
    });
    // Focus listeners are activated and delivered through deferred effects.
    let (handle, demo) = opened;
    cx.update_window(handle, |_, window, _| window.activate_window())
      .unwrap();
    cx.run_until_parked();
    (handle, demo)
  }

  /// Clicks `id` and lets the focus events run.
  fn click(cx: &mut TestAppContext, handle: AnyWindowHandle, id: impl Into<ElementId>) {
    let id = id.into();
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
  }

  #[gpui_kit::test]
  fn focus_selects_all_and_enter_normalises_an_alias(cx: &mut TestAppContext) {
    let (handle, demo) = open(cx);
    let cell = cx.read(|cx| demo.read(cx).top[0].clone());
    click(cx, handle, "video-top-input");
    cx.update_window(handle, |_, window, cx| {
      window.input("ger", cx);
      window.press("enter", cx);
    })
    .unwrap();
    cx.read(|cx| {
      assert_eq!(cell.read(cx).value.as_ref(), "de");
      assert!(!cell.read(cx).is_open());
    });
  }

  #[gpui_kit::test]
  fn enter_keeps_free_text(cx: &mut TestAppContext) {
    let (handle, demo) = open(cx);
    let cell = cx.read(|cx| demo.read(cx).top[2].clone());
    click(cx, handle, "subtitle-top-input");
    cx.update_window(handle, |_, window, cx| {
      window.input("Commentary", cx);
      window.press("enter", cx);
    })
    .unwrap();
    cx.read(|cx| assert_eq!(cell.read(cx).value.as_ref(), "Commentary"));
  }

  #[gpui_kit::test]
  fn typing_highlights_the_first_match_and_opens_the_list(cx: &mut TestAppContext) {
    let (handle, demo) = open(cx);
    let cell = cx.read(|cx| demo.read(cx).top[1].clone());
    click(cx, handle, "audio-top-input");
    cx.update_window(handle, |_, window, cx| window.input("japan", cx))
      .unwrap();
    cx.read(|cx| {
      let cell = cell.read(cx);
      assert!(cell.is_open());
      let matching = cell.matching().expect("a match");
      assert_eq!(cell.options()[matching], "ja");
      // Preferred "ja" is the first option.
      assert_eq!(matching, 0);
    });
  }

  #[gpui_kit::test]
  fn the_list_flips_above_cells_at_the_bottom(cx: &mut TestAppContext) {
    let (handle, demo) = open(cx);
    let (top, bottom) = cx.read(|cx| (demo.read(cx).top[0].clone(), demo.read(cx).bottom[0].clone()));
    click(cx, handle, "video-top-input");
    cx.read(|cx| {
      assert!(top.read(cx).is_open());
      assert_eq!(top.read(cx).placement(), Placement::Below);
    });
    click(cx, handle, "video-bottom-input");
    cx.read(|cx| {
      assert!(!top.read(cx).is_open());
      assert!(bottom.read(cx).is_open());
      assert_eq!(bottom.read(cx).placement(), Placement::Above);
    });
  }

  #[gpui_kit::test]
  fn clicking_an_option_commits_it(cx: &mut TestAppContext) {
    let (handle, demo) = open(cx);
    let cell = cx.read(|cx| demo.read(cx).top[0].clone());
    click(cx, handle, "video-top-input");
    click(cx, handle, ("option", 1usize));
    cx.read(|cx| {
      let cell = cell.read(cx);
      assert_eq!(cell.value.as_ref(), cell.options()[1]);
      assert!(!cell.is_open());
    });
  }

  #[test]
  fn opens_below_when_the_list_fits_or_below_is_larger() {
    // 10 rows × 34 px = 340 px list.
    assert_eq!(placement_for(100.0, 130.0, 800.0, 700), Placement::Below);
    assert_eq!(placement_for(700.0, 730.0, 800.0, 700), Placement::Above);
    // Neither side fits: the larger side wins, ties go below.
    assert_eq!(placement_for(200.0, 230.0, 430.0, 700), Placement::Below);
    assert_eq!(placement_for(260.0, 290.0, 430.0, 700), Placement::Above);
    // A short list fits below even near the bottom.
    assert_eq!(placement_for(650.0, 680.0, 800.0, 2), Placement::Below);
  }

  #[test]
  fn centres_the_matching_row() {
    assert_eq!(first_visible_row(0, 700), 0);
    assert_eq!(first_visible_row(3, 700), 0);
    assert_eq!(first_visible_row(50, 700), 45);
    assert_eq!(first_visible_row(5, 3), 4);
  }
}
