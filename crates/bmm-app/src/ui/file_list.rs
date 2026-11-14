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

//! The File List tab (`FileList.tsx`): the Welcome page while it is empty,
//! otherwise one box per card (a unit, or a group of structurally identical
//! units) with a one-line header and its track table. The boxes become the
//! merge cards with the cards.
//!
//! The table views are kept by their first root, so they survive files
//! being added, grouped and edited (typing in a group's language cell
//! changes its group key). A click in a box makes it the active card, the
//! one the table keys act on: ↑/↓, Space, Ctrl/Cmd+A and Ctrl/Cmd+Shift+A
//! work wherever the focus is, as React's document listener did, except in
//! a text field and when another control binds the key.

use std::collections::{HashMap, HashSet};

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

use super::track_table::{TrackTable, scope_title};
use super::welcome::Welcome;
use crate::domain::status::{QueueItem, QueueItemStatus};
use crate::state::files::{FileEvent, TableScope};
use crate::state::queue::QueueEvent;
use crate::state::{Stores, TabType};

/// A key the active card's table handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKey {
  Up,
  Down,
  Space,
  SelectAll,
  ClearAll,
}

/// The table key of a keystroke: plain ↑, ↓ and Space, and A with Ctrl or
/// Cmd (React read `ctrlKey || metaKey`), with Shift to clear.
pub fn table_key(keystroke: &Keystroke) -> Option<TableKey> {
  let modifiers = &keystroke.modifiers;
  if modifiers.alt || modifiers.function {
    return None;
  }
  let plain = !modifiers.control && !modifiers.platform && !modifiers.shift;
  match keystroke.key.as_str() {
    "up" if plain => Some(TableKey::Up),
    "down" if plain => Some(TableKey::Down),
    "space" if plain => Some(TableKey::Space),
    "a" if modifiers.control || modifiers.platform => Some(if modifiers.shift {
      TableKey::ClearAll
    } else {
      TableKey::SelectAll
    }),
    _ => None,
  }
}

pub struct FileListView {
  stores: Stores,
  /// The table of each card, by its first root.
  tables: HashMap<String, Entity<TrackTable>>,
  scroll: ScrollHandle,
  window: Option<AnyWindowHandle>,
  _subscriptions: Vec<Subscription>,
}

impl FileListView {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let view = cx.weak_entity();
    let subscriptions = vec![
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      cx.subscribe(&stores.files, |_, _, event: &FileEvent, cx| {
        // The headers show the active card; the rows repaint themselves.
        if matches!(event, FileEvent::Highlight { .. }) {
          cx.notify();
        }
      }),
      cx.subscribe(&stores.queue, |_, _, event: &QueueEvent, cx| {
        if matches!(event, QueueEvent::ItemChanged { .. }) {
          cx.notify();
        }
      }),
      cx.intercept_keystrokes(move |event, window, cx| {
        let handled = view
          .update(cx, |view, cx| view.on_keystroke(event, window, cx))
          .unwrap_or(false);
        if handled {
          cx.stop_propagation();
        }
      }),
    ];
    Self {
      stores,
      tables: HashMap::new(),
      scroll: ScrollHandle::new(),
      window: None,
      _subscriptions: subscriptions,
    }
  }

  /// The table of the card whose first root is `id`.
  pub fn table(&self, id: &str) -> Option<Entity<TrackTable>> {
    self.tables.get(id).cloned()
  }

  /// A keystroke anywhere in the window, before the key bindings: the
  /// table keys act on the active card's table. `true` when handled.
  fn on_keystroke(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.window != Some(window.window_handle()) || self.stores.ui.read(cx).active_tab() != TabType::FileList {
      return false;
    }
    let Some(key) = table_key(&event.keystroke) else {
      return false;
    };
    // A text field keeps its keys, and so does any control that binds one
    // (a menu, a list).
    if event.context_stack.iter().any(|context| context.contains("Input")) {
      return false;
    }
    let (claimed, _) = cx
      .key_bindings()
      .borrow()
      .bindings_for_input(std::slice::from_ref(&event.keystroke), &event.context_stack);
    if !claimed.is_empty() {
      return false;
    }
    let Some(table) = self
      .stores
      .files
      .read(cx)
      .active_card()
      .and_then(|id| self.tables.get(id))
      .cloned()
    else {
      return false;
    };
    if table.read(cx).is_disabled(cx) {
      return false;
    }
    match key {
      TableKey::Up | TableKey::Down => table.update(cx, |table, cx| table.move_cursor(key == TableKey::Down, cx)),
      TableKey::SelectAll | TableKey::ClearAll => {
        table.update(cx, |table, cx| table.select_all_rows(key == TableKey::SelectAll, cx));
        true
      }
      TableKey::Space => {
        // A focused button or checkbox takes Space itself.
        let focus = window.focused(cx);
        if focus.is_some_and(|focus| &focus != table.read(cx).focus_handle()) {
          return false;
        }
        table.update(cx, |table, cx| table.flip_selected(cx))
      }
    }
  }

  /// Creates the tables of new cards, hands the others their current units
  /// and drops the tables of cards that are gone.
  fn sync_tables(&mut self, scopes: &[TableScope], cx: &mut Context<Self>) {
    let mut live = HashSet::new();
    for scope in scopes {
      live.insert(scope.id().to_string());
      match self.tables.get(scope.id()) {
        Some(table) => table.update(cx, |table, cx| table.set_scope(scope.clone(), cx)),
        None => {
          let (stores, scroll) = (self.stores.clone(), self.scroll.clone());
          let table = cx.new(|cx| {
            let mut table = TrackTable::new(stores, scope.clone(), cx);
            table.set_scroll_handle(scroll);
            table
          });
          self.tables.insert(scope.id().to_string(), table);
        }
      }
    }
    self.tables.retain(|id, _| live.contains(id));
  }
}

/// A queue item's status as the Queue tab shows it: the label (with the
/// percentage while merging) in the status colour.
fn status_label(item: &QueueItem, cx: &App) -> (String, Hsla) {
  let theme = cx.theme();
  let color = match item.status {
    QueueItemStatus::Merging | QueueItemStatus::Completed => theme.success,
    QueueItemStatus::Cancelled | QueueItemStatus::Failed => theme.danger,
    QueueItemStatus::Waiting => theme.foreground,
  };
  let mut label = tr!(item.status.label_key()).to_string();
  if item.status == QueueItemStatus::Merging {
    label.push_str(&format!(" {}%", item.progress));
  }
  (label, color)
}

impl Render for FileListView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    self.window = Some(window.window_handle());
    let files = self.stores.files.read(cx);
    if !files.has_files() {
      self.tables.clear();
      return Welcome::new(self.stores.clone()).into_any_element();
    }
    let scopes: Vec<TableScope> = files.entries().iter().map(TableScope::from_entry).collect();
    let active = files.active_card().map(str::to_string);
    self.sync_tables(&scopes, cx);

    let theme = cx.theme();
    let (border, primary, muted) = (theme.border, theme.primary, theme.muted_foreground);
    let boxes: Vec<AnyElement> = scopes
      .iter()
      .enumerate()
      .map(|(index, scope)| {
        let id = scope.id().to_string();
        let queue = self.stores.queue.read(cx);
        let statuses: Vec<AnyElement> = scope
          .roots()
          .filter_map(|root| queue.item(root).map(|item| (root, item)))
          .map(|(root, item)| {
            let (label, color) = status_label(item, cx);
            let label = if scope.is_group() {
              format!("{}: {label}", bmm_core::merge_args::file_name(root))
            } else {
              label
            };
            div().text_color(color).child(label).into_any_element()
          })
          .collect();
        let card = id.clone();
        v_flex()
          .id(("file-entry", index))
          .test_support()
          .w_full()
          .p(super::space(1.0))
          .gap(super::space(0.5))
          .border_1()
          .rounded(px(4.0))
          .border_color(if active.as_deref() == Some(id.as_str()) {
            primary
          } else {
            border
          })
          .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
            let card = card.clone();
            this
              .stores
              .files
              .update(cx, |files, cx| files.set_active_card(Some(card), cx));
          }))
          .child(
            h_flex()
              .flex_wrap()
              .gap(super::space(1.0))
              .child(
                div()
                  .min_w_0()
                  .max_w_full()
                  .truncate()
                  .text_color(muted)
                  .child(scope_title(scope)),
              )
              .children(statuses),
          )
          .child(self.tables[&id].clone())
          .into_any_element()
      })
      .collect();
    div()
      .id("file-list")
      .test_support()
      .size_full()
      .overflow_y_scroll()
      .restrict_scroll_to_axis()
      .track_scroll(&self.scroll)
      .text_size(super::text::body2())
      .child(v_flex().gap(super::space(1.0)).children(boxes))
      .vertical_scrollbar(&self.scroll)
      .into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  fn key(text: &'static str) -> Option<TableKey> {
    table_key(&Keystroke::parse(text).unwrap())
  }

  #[test]
  fn recognises_the_table_keys() {
    assert_eq!(key("up"), Some(TableKey::Up));
    assert_eq!(key("down"), Some(TableKey::Down));
    assert_eq!(key("space"), Some(TableKey::Space));
    assert_eq!(key("ctrl-a"), Some(TableKey::SelectAll));
    assert_eq!(key("cmd-a"), Some(TableKey::SelectAll));
    assert_eq!(key("ctrl-shift-a"), Some(TableKey::ClearAll));
    assert_eq!(key("cmd-shift-a"), Some(TableKey::ClearAll));
    for other in [
      "shift-down",
      "ctrl-up",
      "alt-a",
      "a",
      "shift-space",
      "enter",
      "ctrl-alt-a",
    ] {
      assert_eq!(key(other), None, "{other}");
    }
  }
}
