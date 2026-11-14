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

//! Prototype 4: drag-reorder of rows in a scrolling table.
//!
//! GPUI's own `on_drag` starts after 2 px (a hard-coded constant) and then
//! suppresses the click, so a row click with a 3 px wobble would be lost. The
//! React table (dnd-kit) activates at 5 px. This prototype therefore runs its
//! own pointer state machine: press arms, more than 5 px of movement starts
//! the drag, release either reorders (dragging) or toggles the row's UI
//! selection (a click). Mouse-down on a control inside a row stops
//! propagation, so checkboxes and buttons never arm a drag.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Pointer travel that turns a press into a drag (dnd-kit `distance: 5`).
pub const ACTIVATION_DISTANCE: f32 = 5.0;
const ROW_HEIGHT: f32 = 32.0;
const AUTOSCROLL_EDGE: f32 = 28.0;
const AUTOSCROLL_STEP: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragState {
  Idle,
  Pressed {
    row: usize,
    origin: Point<Pixels>,
  },
  Dragging {
    row: usize,
    cursor: Point<Pixels>,
    insert_at: usize,
  },
}

#[derive(Clone, Debug)]
pub struct Row {
  pub id: usize,
  pub name: SharedString,
  pub checked: bool,
}

pub struct ReorderTable {
  pub rows: Vec<Row>,
  pub selected: BTreeSet<usize>,
  pub drag: DragState,
  pub button_clicks: usize,
  row_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
  viewport: Rc<RefCell<Bounds<Pixels>>>,
  scroll: ScrollHandle,
}

/// Index a dragged row is inserted before (0..=len), from the cursor's y and
/// the rows' bounds in display order.
pub fn insert_index(row_bounds: &[Bounds<Pixels>], cursor_y: Pixels) -> usize {
  row_bounds
    .iter()
    .position(|bounds| cursor_y < bounds.center().y)
    .unwrap_or(row_bounds.len())
}

/// Moves `from` so that it lands before the row that was at `insert_at`.
pub fn move_row<T>(items: &mut Vec<T>, from: usize, insert_at: usize) {
  if from >= items.len() {
    return;
  }
  let item = items.remove(from);
  let target = if insert_at > from { insert_at - 1 } else { insert_at };
  items.insert(target.min(items.len()), item);
}

impl ReorderTable {
  pub fn new(row_count: usize) -> Self {
    Self {
      rows: (0..row_count)
        .map(|id| Row {
          id,
          name: format!("Track {id:02} · audio · eng").into(),
          checked: id % 3 == 0,
        })
        .collect(),
      selected: BTreeSet::new(),
      drag: DragState::Idle,
      button_clicks: 0,
      row_bounds: Rc::default(),
      viewport: Rc::default(),
      scroll: ScrollHandle::new(),
    }
  }

  pub fn order(&self) -> Vec<usize> {
    self.rows.iter().map(|row| row.id).collect()
  }

  fn on_row_mouse_down(&mut self, row: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
    self.drag = DragState::Pressed {
      row,
      origin: event.position,
    };
    cx.notify();
  }

  fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
    match self.drag {
      DragState::Idle => {}
      DragState::Pressed { row, origin } => {
        if event.pressed_button != Some(MouseButton::Left) {
          self.drag = DragState::Idle;
        } else if (event.position - origin).magnitude() > f64::from(ACTIVATION_DISTANCE) {
          self.drag = DragState::Dragging {
            row,
            cursor: event.position,
            insert_at: insert_index(&self.row_bounds.borrow(), event.position.y),
          };
          cx.notify();
        }
      }
      DragState::Dragging { row, .. } => {
        self.autoscroll(event.position.y);
        self.drag = DragState::Dragging {
          row,
          cursor: event.position,
          insert_at: insert_index(&self.row_bounds.borrow(), event.position.y),
        };
        cx.notify();
      }
    }
  }

  fn on_mouse_up(&mut self, cx: &mut Context<Self>) {
    match std::mem::replace(&mut self.drag, DragState::Idle) {
      DragState::Idle => return,
      DragState::Pressed { row, .. } => {
        let id = self.rows[row].id;
        if !self.selected.remove(&id) {
          self.selected.insert(id);
        }
      }
      DragState::Dragging { row, insert_at, .. } => move_row(&mut self.rows, row, insert_at),
    }
    cx.notify();
  }

  /// Scrolls one step when the cursor is within the edge band of the
  /// scrolling viewport while dragging.
  fn autoscroll(&self, cursor_y: Pixels) {
    let viewport = *self.viewport.borrow();
    let offset = self.scroll.offset();
    let delta = if cursor_y < viewport.top() + px(AUTOSCROLL_EDGE) {
      AUTOSCROLL_STEP
    } else if cursor_y > viewport.bottom() - px(AUTOSCROLL_EDGE) {
      -AUTOSCROLL_STEP
    } else {
      return;
    };
    let max = self.scroll.max_offset().y;
    let y = (offset.y + px(delta)).min(px(0.0)).max(-max);
    self.scroll.set_offset(point(offset.x, y));
  }

  fn render_row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let selected = self.selected.contains(&row.id);
    let dragged = matches!(self.drag, DragState::Dragging { row: dragged, .. } if dragged == index);
    let indicator_before = matches!(self.drag, DragState::Dragging { insert_at, .. } if insert_at == index);
    let indicator_after = matches!(self.drag, DragState::Dragging { insert_at, .. } if insert_at == self.rows.len() && index + 1 == self.rows.len());
    let id = row.id;
    h_flex()
      .id(("row", row.id))
      .h(px(ROW_HEIGHT))
      .px_2()
      .gap_2()
      .border_b_1()
      .border_color(theme.table_row_border)
      .when(selected, |el| el.bg(theme.table_active))
      .when(dragged, |el| el.opacity(0.45))
      .when(indicator_before, |el| el.border_t_2().border_color(theme.primary))
      .when(indicator_after, |el| el.border_b_2().border_color(theme.primary))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, _, cx| this.on_row_mouse_down(index, event, cx)),
      )
      .child(div().text_color(theme.muted_foreground).child(IconName::GripVertical))
      .child(
        div()
          .id(("checkbox-cell", id))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            Checkbox::new(("checkbox", id))
              .checked(row.checked)
              .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                if let Some(row) = this.rows.iter_mut().find(|row| row.id == id) {
                  row.checked = *checked;
                }
                cx.notify();
              })),
          ),
      )
      .child(div().flex_1().child(row.name.clone()))
      .child(
        div()
          .id(("button-cell", id))
          .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            Button::new(("action", id))
              .ghost()
              .xsmall()
              .label("Edit")
              .on_click(cx.listener(|this, _, _, cx| {
                this.button_clicks += 1;
                cx.notify();
              })),
          ),
      )
      .test_support()
  }
}

impl Render for ReorderTable {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let rows: Vec<AnyElement> = self
      .rows
      .iter()
      .enumerate()
      .map(|(index, row)| self.render_row(index, row, cx).into_any_element())
      .collect();
    let row_bounds = self.row_bounds.clone();
    let viewport = self.viewport.clone();
    let ghost = match self.drag {
      DragState::Dragging { row, cursor, .. } => Some(
        deferred(
          anchored().position(cursor + point(px(12.0), px(4.0))).child(
            div()
              .px_2()
              .py_1()
              .rounded_sm()
              .bg(cx.theme().popover)
              .border_1()
              .border_color(cx.theme().border)
              .shadow_md()
              .child(self.rows[row].name.clone()),
          ),
        )
        .with_priority(1),
      ),
      _ => None,
    };
    v_flex()
      .size_full()
      .p_4()
      .gap_2()
      .child(format!(
        "Drag rows by any non-control part ({ACTIVATION_DISTANCE} px activation). Clicks toggle selection; checkboxes and buttons never start a drag. Selected: {:?}, button clicks: {}",
        self.selected, self.button_clicks
      ))
      .child(
        div()
          .id("reorder-viewport")
          .flex_1()
          .min_h_0()
          .border_1()
          .border_color(cx.theme().border)
          .rounded_md()
          .overflow_y_scroll()
          .track_scroll(&self.scroll)
          .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.on_mouse_move(event, cx)))
          .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.on_mouse_up(cx)))
          .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.on_mouse_up(cx)))
          .child(
            canvas(
              move |bounds, _, _| *viewport.borrow_mut() = bounds,
              |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
          )
          .child(
            v_flex()
              .on_children_prepainted(move |bounds, _, _| *row_bounds.borrow_mut() = bounds)
              .children(rows),
          ),
      )
      .children(ghost)
  }
}

#[cfg(test)]
mod tests {
  // `gpui_kit::*` also exports GPUI's `test` macro; name the built-in one.
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::test::TestWindowExt;

  #[test]
  fn moves_rows_like_a_sortable_list() {
    let mut items = vec![0, 1, 2, 3, 4];
    move_row(&mut items, 1, 4);
    assert_eq!(items, [0, 2, 3, 1, 4]);
    move_row(&mut items, 3, 0);
    assert_eq!(items, [1, 0, 2, 3, 4]);
    move_row(&mut items, 0, 5);
    assert_eq!(items, [0, 2, 3, 4, 1]);
    move_row(&mut items, 2, 2);
    assert_eq!(items, [0, 2, 3, 4, 1]);
  }

  fn open(cx: &mut TestAppContext, rows: usize) -> (AnyWindowHandle, Entity<ReorderTable>) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
          Point::default(),
          size(px(800.0), px(600.0)),
        ))),
        ..Default::default()
      };
      gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| ReorderTable::new(rows))).expect("open window")
    })
  }

  fn centre(window: &Window, row: usize) -> Point<Pixels> {
    window.find(("row", row)).bounds().center()
  }

  #[gpui_kit::test]
  fn dragging_past_the_activation_distance_reorders(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, 8);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let from = centre(window, 1);
      // Below the centre of row 4: insert before row 5.
      let to = centre(window, 4) + point(px(0.0), px(6.0));
      window.drag(from, to, cx);
    })
    .unwrap();
    cx.read(|cx| {
      assert_eq!(table.read(cx).order(), [0, 2, 3, 4, 1, 5, 6, 7]);
      assert!(table.read(cx).selected.is_empty());
      assert_eq!(table.read(cx).drag, DragState::Idle);
    });
  }

  #[gpui_kit::test]
  fn a_wobble_within_the_activation_distance_is_a_click(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, 8);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let from = centre(window, 2) + point(px(60.0), px(0.0));
      window.drag(from, from + point(px(3.0), px(3.0)), cx);
    })
    .unwrap();
    cx.read(|cx| {
      assert_eq!(table.read(cx).order(), [0, 1, 2, 3, 4, 5, 6, 7]);
      assert_eq!(table.read(cx).selected.iter().copied().collect::<Vec<_>>(), [2]);
    });
  }

  #[gpui_kit::test]
  fn pressing_a_control_never_starts_a_drag(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, 8);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let checkbox = window.find(("checkbox", 1usize)).bounds().center();
      window.drag(checkbox, centre(window, 6), cx);
      let button = window.find(("action", 2usize)).bounds().center();
      window.drag(button, centre(window, 0), cx);
    })
    .unwrap();
    cx.read(|cx| {
      assert_eq!(table.read(cx).order(), [0, 1, 2, 3, 4, 5, 6, 7]);
      assert!(table.read(cx).selected.is_empty());
    });
  }

  #[gpui_kit::test]
  fn clicking_controls_works_without_selecting_the_row(cx: &mut TestAppContext) {
    let (handle, table) = open(cx, 8);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.click(("checkbox", 1usize), cx);
      window.click(("action", 1usize), cx);
    })
    .unwrap();
    cx.read(|cx| {
      let table = table.read(cx);
      assert!(table.rows[1].checked);
      assert_eq!(table.button_clicks, 1);
      assert!(table.selected.is_empty());
      assert_eq!(table.order(), [0, 1, 2, 3, 4, 5, 6, 7]);
    });
  }

  #[test]
  fn insert_index_uses_row_centres() {
    let rows: Vec<Bounds<Pixels>> = (0..3)
      .map(|i| Bounds::new(point(px(0.0), px(i as f32 * 32.0)), size(px(100.0), px(32.0))))
      .collect();
    assert_eq!(insert_index(&rows, px(0.0)), 0);
    assert_eq!(insert_index(&rows, px(17.0)), 1);
    assert_eq!(insert_index(&rows, px(47.0)), 1);
    assert_eq!(insert_index(&rows, px(49.0)), 2);
    assert_eq!(insert_index(&rows, px(200.0)), 3);
  }
}
