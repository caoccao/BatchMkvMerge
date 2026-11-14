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
//! otherwise one card per entry: a [`FileCard`] per merge unit, a
//! [`GroupCard`] per group of structurally identical units.
//!
//! The cards and their track tables are kept by the card's first root, so
//! they survive files being added, grouped and edited (typing in a group's
//! language cell changes its group key). Each card is drawn as a cached
//! view at the height it measured, so a card that changes repaints alone.
//!
//! A card dragged by its handle onto another file card merges into it
//! (`cardDrag.ts`): the list hit-tests the cards while the drag moves, marks
//! the target and sets the cursor (copy over a file card, not-allowed over a
//! group card or the dragged card itself). Files dragged in from the system
//! outline the list.
//!
//! A click in a card makes it the active card, the one the table keys act
//! on: ↑/↓, Space, Ctrl/Cmd+A and Ctrl/Cmd+Shift+A work wherever the focus
//! is, as React's document listener did, except in a text field and when
//! another control binds the key.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

use super::cards::{CardDrag, FileCard, GroupCard};
use super::track_table::TrackTable;
use super::welcome::Welcome;
use crate::state::files::TableScope;
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

/// Where a card drag points: a file card it may merge into, a card it may
/// not merge into (a group card, or itself), or no card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
  Card(String),
  Refused,
  None,
}

/// The drop target of a drag of `source` at `position` over the cards
/// `scopes`, laid out at `bounds` (same order).
pub fn drop_target(
  source: &str,
  position: Point<Pixels>,
  scopes: &[TableScope],
  bounds: &[Bounds<Pixels>],
) -> DropTarget {
  match scopes.iter().zip(bounds).find(|(_, bounds)| bounds.contains(&position)) {
    Some((scope, _)) if !scope.is_group() && scope.id() != source => DropTarget::Card(scope.id().to_string()),
    Some(_) => DropTarget::Refused,
    None => DropTarget::None,
  }
}

/// A card view of either kind.
#[derive(Clone)]
pub enum CardView {
  File(Entity<FileCard>),
  Group(Entity<GroupCard>),
}

impl CardView {
  pub fn file(&self) -> Option<&Entity<FileCard>> {
    match self {
      Self::File(card) => Some(card),
      Self::Group(_) => None,
    }
  }

  pub fn group(&self) -> Option<&Entity<GroupCard>> {
    match self {
      Self::File(_) => None,
      Self::Group(card) => Some(card),
    }
  }

  pub fn render_count(&self, cx: &App) -> usize {
    match self {
      Self::File(card) => card.read(cx).render_count(),
      Self::Group(card) => card.read(cx).render_count(),
    }
  }

  /// The card as a cached view at its measured height; uncached until it
  /// has measured itself.
  fn element(&self, cx: &App) -> AnyElement {
    fn cached<V: Render>(card: &Entity<V>, height: Option<Pixels>) -> AnyElement {
      match height {
        Some(height) => card
          .clone()
          .cached(StyleRefinement::default().w_full().h(height))
          .into_any_element(),
        None => card.clone().into_any_element(),
      }
    }
    match self {
      Self::File(card) => cached(card, card.read(cx).height().get()),
      Self::Group(card) => cached(card, card.read(cx).height().get()),
    }
  }
}

pub struct FileListView {
  stores: Stores,
  /// The table of each card, by its first root.
  tables: HashMap<String, Entity<TrackTable>>,
  /// The cards, by their first root.
  cards: HashMap<String, CardView>,
  /// The cards in list order and their bounds in the last frame, for the
  /// card drag.
  scopes: Vec<TableScope>,
  card_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
  /// The file card a card drag would merge into.
  drag_target: Option<String>,
  scroll: ScrollHandle,
  window: Option<AnyWindowHandle>,
  _subscriptions: Vec<Subscription>,
}

impl FileListView {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let view = cx.weak_entity();
    let subscriptions = vec![
      // Cards come and go only with structural changes; each card repaints
      // its own data.
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
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
      cards: HashMap::new(),
      scopes: Vec::new(),
      card_bounds: Rc::default(),
      drag_target: None,
      scroll: ScrollHandle::new(),
      window: None,
      _subscriptions: subscriptions,
    }
  }

  /// The table of the card whose first root is `id`.
  pub fn table(&self, id: &str) -> Option<Entity<TrackTable>> {
    self.tables.get(id).cloned()
  }

  /// The card whose first root is `id`.
  pub fn card(&self, id: &str) -> Option<&CardView> {
    self.cards.get(id)
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

  /// Creates the tables and cards of new entries, hands the others their
  /// current units and drops those of entries that are gone. A card that
  /// changes kind (a unit joining a group) is made again around the same
  /// table.
  fn sync_cards(&mut self, scopes: &[TableScope], cx: &mut Context<Self>) {
    let mut live = HashSet::new();
    for scope in scopes {
      let id = scope.id().to_string();
      live.insert(id.clone());
      let table = match self.tables.get(&id) {
        Some(table) => {
          table.update(cx, |table, cx| table.set_scope(scope.clone(), cx));
          table.clone()
        }
        None => {
          let (stores, scroll) = (self.stores.clone(), self.scroll.clone());
          let table = cx.new(|cx| {
            let mut table = TrackTable::new(stores, scope.clone(), cx);
            table.set_scroll_handle(scroll);
            table
          });
          self.tables.insert(id.clone(), table.clone());
          table
        }
      };
      match (self.cards.get(&id), scope.is_group()) {
        (Some(CardView::File(card)), false) => {
          card.update(cx, |card, cx| card.set_members(scope.members().to_vec(), cx));
        }
        (Some(CardView::Group(card)), true) => {
          card.update(cx, |card, cx| card.set_units(scope.units().to_vec(), cx));
        }
        _ => {
          let stores = self.stores.clone();
          let card = if scope.is_group() {
            let units = scope.units().to_vec();
            CardView::Group(cx.new(|cx| GroupCard::new(stores, units, table, cx)))
          } else {
            let members = scope.members().to_vec();
            CardView::File(cx.new(|cx| FileCard::new(stores, members, table, cx)))
          };
          self.cards.insert(id, card);
        }
      }
    }
    self.tables.retain(|id, _| live.contains(id));
    self.cards.retain(|id, _| live.contains(id));
  }

  /// A card drag moved: marks the file card it would merge into and shows
  /// whether it may.
  fn drag_moved(&mut self, event: &DragMoveEvent<CardDrag>, window: &mut Window, cx: &mut Context<Self>) {
    let source = event.drag(cx).root.clone();
    let target = drop_target(&source, event.event.position, &self.scopes, &self.card_bounds.borrow());
    let cursor = match target {
      DropTarget::Card(_) => CursorStyle::DragCopy,
      DropTarget::Refused => CursorStyle::OperationNotAllowed,
      DropTarget::None => CursorStyle::ClosedHand,
    };
    cx.set_active_drag_cursor_style(cursor, window);
    let target = match target {
      DropTarget::Card(root) => Some(root),
      _ => None,
    };
    self.set_drag_target(target, cx);
  }

  fn set_drag_target(&mut self, target: Option<String>, cx: &mut Context<Self>) {
    if self.drag_target != target {
      self.drag_target = target.clone();
      self
        .stores
        .files
        .update(cx, |files, cx| files.set_drop_target(target, cx));
    }
  }

  /// A card dropped in the list: it merges into the marked file card.
  fn dropped(&mut self, drag: &CardDrag, cx: &mut Context<Self>) {
    let target = self.drag_target.take();
    self.stores.files.update(cx, |files, cx| {
      files.set_drop_target(None, cx);
      if let Some(target) = target
        && target != drag.root
      {
        files.merge_card_into(&drag.root, &target, cx);
      }
    });
  }
}

impl Render for FileListView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    self.window = Some(window.window_handle());
    if self.drag_target.is_some() && !cx.has_active_drag() {
      // The drag ended outside the list.
      cx.defer_in(window, |this, _, cx| this.set_drag_target(None, cx));
    }
    let primary = cx.theme().primary;
    // System files dragged over the window outline the list (Q&A).
    let drop_zone = div()
      .id("file-drop-zone")
      .test_support()
      .size_full()
      .border_2()
      .border_color(transparent_black())
      .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_dashed().border_color(primary));

    let files = self.stores.files.read(cx);
    if !files.has_files() {
      self.tables.clear();
      self.cards.clear();
      self.scopes.clear();
      return drop_zone.child(Welcome::new(self.stores.clone())).into_any_element();
    }
    let scopes: Vec<TableScope> = files.entries().iter().map(TableScope::from_entry).collect();
    self.sync_cards(&scopes, cx);

    let entries: Vec<AnyElement> = scopes
      .iter()
      .enumerate()
      .map(|(index, scope)| {
        let card = scope.id().to_string();
        div()
          .id(("file-entry", index))
          .test_support()
          .w_full()
          .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
            let card = card.clone();
            this
              .stores
              .files
              .update(cx, |files, cx| files.set_active_card(Some(card), cx));
          }))
          .child(self.cards[scope.id()].element(cx))
          .into_any_element()
      })
      .collect();
    self.scopes = scopes;
    let card_bounds = self.card_bounds.clone();
    drop_zone
      .child(
        div()
          .id("file-list")
          .test_support()
          .size_full()
          .overflow_y_scroll()
          .restrict_scroll_to_axis()
          .track_scroll(&self.scroll)
          .text_size(super::text::body2())
          .on_drag_move(cx.listener(Self::drag_moved))
          .on_drop(cx.listener(|this, drag: &CardDrag, _, cx| this.dropped(drag, cx)))
          .child(
            v_flex()
              .gap(super::space(1.0))
              .on_children_prepainted(move |bounds, _, _| *card_bounds.borrow_mut() = bounds)
              .children(entries),
          )
          .vertical_scrollbar(&self.scroll),
      )
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
