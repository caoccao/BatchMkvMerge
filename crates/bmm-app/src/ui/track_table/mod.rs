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

//! The track selection table of a card, with its row selection.
//!
//! One table per card: the merge checkboxes (and select-all, indeterminate
//! when some are ticked), ID, type emoji, codec, description, size and bit
//! rate as configured, the language and track-name cells, and the tri-state
//! default/forced buttons whose headers run the reset automation. Edits go to
//! the clicked row, or to every selected row when the clicked row is
//! selected, and on a group card to the same position of every unit
//! ([`TableScope`]).
//!
//! A click on a row toggles its selection (a highlight, independent of the
//! merge checkbox) and moves the keyboard cursor there (a dashed outline).
//! Pressing a row and moving more than 5 px drags it (GPUI's own drag starts
//! at 2 px and drops the click, so the table runs its own), with an insertion
//! line, a dimmed source row and auto-scroll at the list's edges. Controls in
//! a row never start a drag or a selection. The keyboard (↑/↓, Space,
//! Ctrl/Cmd+A, Ctrl/Cmd+Shift+A) reaches the table through the file list,
//! which knows the active card.

pub mod autocomplete;
#[cfg(test)]
pub(super) mod tests;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use bmm_core::config::{ConfigFormatting, ConfigProfile};
use bmm_core::languages::{build_language_options, build_track_name_options};
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::{MediaTrackKind, MediaTrackType};
use gpui_kit::assets::IconName;
use gpui_kit::base::{self as kit_base, CheckboxState, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Sizable, Size as ComponentSize, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use self::autocomplete::{AutocompleteCell, CellEdited, CellKind};
use crate::domain::file_tree::{CombinedTrack, FlagKind};
use crate::domain::format::{format_track_bit_rate, format_track_size, track_type_emoji};
use crate::domain::metadata_error::metadata_error_message;
use crate::state::Stores;
use crate::state::files::table::flag_of;
use crate::state::files::{FileEvent, LoadState, TableScope};
use crate::state::queue::QueueEvent;

/// Pointer travel that turns a press into a drag.
pub const ACTIVATION_DISTANCE: f32 = 5.0;
const ROW_HEIGHT: f32 = 34.0;
const CELL_PADDING: Pixels = px(6.0);
const AUTOSCROLL_EDGE: f32 = 28.0;
const AUTOSCROLL_STEP: f32 = 12.0;

/// The row pointer state machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragState {
  Idle,
  /// Pressed on a row, not moved past the activation distance yet.
  Pressed {
    row: usize,
    origin: Point<Pixels>,
  },
  /// Dragging `row`; it would land before the row at `insert_at`.
  Dragging {
    row: usize,
    cursor: Point<Pixels>,
    insert_at: usize,
  },
}

/// The position (0..=len) a dragged row is inserted at, from the pointer's
/// y and the rows' bounds in display order.
pub fn insert_index(row_bounds: &[Bounds<Pixels>], y: Pixels) -> usize {
  row_bounds
    .iter()
    .position(|bounds| y < bounds.center().y)
    .unwrap_or(row_bounds.len())
}

/// The row a dragged row moves to, or `None` when inserting at `insert_at`
/// leaves it where it is.
pub fn move_target(from: usize, insert_at: usize) -> Option<usize> {
  let target = if insert_at > from { insert_at - 1 } else { insert_at };
  (target != from).then_some(target)
}

/// The cursor row after ↓ (`down`) or ↑: from none, ↓ takes the first row
/// and ↑ the last; it stops at the ends.
pub fn step_cursor(len: usize, current: Option<usize>, down: bool) -> Option<usize> {
  if len == 0 {
    return None;
  }
  Some(match (current, down) {
    (None, true) => 0,
    (None, false) => len - 1,
    (Some(index), true) => (index + 1).min(len - 1),
    (Some(index), false) => index.saturating_sub(1),
  })
}

/// The table's columns, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
  Check,
  Id,
  Type,
  Codec,
  Description,
  Size,
  BitRate,
  Language,
  Name,
  Default,
  Forced,
}

impl Column {
  const TEXT_HEADERS: [(Self, &'static str); 8] = [
    (Self::Id, "merge.header.id"),
    (Self::Type, "merge.header.type"),
    (Self::Codec, "merge.header.codec"),
    (Self::Description, "merge.header.description"),
    (Self::Size, "merge.header.size"),
    (Self::BitRate, "merge.header.bitRate"),
    (Self::Language, "merge.header.language"),
    (Self::Name, "merge.header.trackName"),
  ];

  fn centered(self) -> bool {
    matches!(self, Self::Check | Self::Default | Self::Forced)
  }

  /// One-based, for screen readers.
  fn index(self) -> usize {
    self as usize + 1
  }

  fn style<E: Styled>(self, element: E, width: Pixels) -> E {
    let element = element
      .flex()
      .items_center()
      .px(CELL_PADDING)
      .whitespace_nowrap()
      .overflow_hidden();
    let element = if self == Self::Name {
      element.flex_1().min_w(width)
    } else {
      element.flex_none().w(width)
    };
    if self.centered() {
      element.justify_center()
    } else {
      element
    }
  }
}

/// The widest displayed value or heading sets each column's width. Track
/// Name uses a minimum that leaves room to type and takes the remaining width.
struct TrackColumns {
  headers: [(Column, SharedString); Column::TEXT_HEADERS.len()],
  widths: [Pixels; Column::Forced as usize + 1],
}

impl TrackColumns {
  fn measure(rows: &[RowTexts], window: &Window) -> Self {
    let mut body = window.text_style();
    body.font_size = super::text::body2().into();
    let heading = TextStyle {
      font_weight: FontWeight::MEDIUM,
      ..body.clone()
    };
    let input = TextStyle {
      font_size: rems(0.875).into(),
      ..body.clone()
    };
    let rem_size = window.rem_size();
    let measure = |text: &str, style: &TextStyle| {
      text
        .lines()
        .map(|line| {
          let run = style.to_run(line.len());
          window
            .text_system()
            .layout_line(line, style.font_size.to_pixels(rem_size), &[run], None)
            .width
        })
        .max()
        .unwrap_or_default()
    };

    let headers = Column::TEXT_HEADERS.map(|(column, key)| (column, tr!(key)));
    let mut widths = [px(0.0); Column::Forced as usize + 1];
    for (column, label) in &headers {
      widths[*column as usize] = measure(label, &heading);
    }
    widths[Column::Check as usize] = rems(0.875).to_pixels(rem_size);
    widths[Column::Default as usize] = rems(1.25).to_pixels(rem_size);
    widths[Column::Forced as usize] = widths[Column::Default as usize];
    for row in rows {
      for (column, text) in [
        (Column::Id, row.id.as_str()),
        (Column::Type, row.emoji),
        (Column::Codec, row.codec.as_str()),
        (Column::Description, row.description.as_str()),
        (Column::Size, row.size.as_str()),
        (Column::BitRate, row.bit_rate.as_str()),
      ] {
        widths[column as usize] = widths[column as usize].max(measure(text, &body));
      }
      let language = measure(&row.language, &input) + (ComponentSize::Small.input_px() + px(1.0)) * 2.0;
      widths[Column::Language as usize] = widths[Column::Language as usize].max(language);
    }
    let mut widths = widths.map(|width| width.ceil() + CELL_PADDING * 2.0);
    widths[Column::Name as usize] = widths[Column::Name as usize].max(px(140.0));
    Self { headers, widths }
  }

  fn style<E: Styled>(&self, column: Column, element: E) -> E {
    column.style(element, self.widths[column as usize])
  }

  fn min_width(&self) -> Pixels {
    self.widths.iter().copied().sum()
  }
}

/// The text of a row's cells as the table shows them (the language and
/// track-name cells are inputs on track rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowTexts {
  /// `id`, or `member:id` for a multi-member unit.
  pub id: String,
  pub emoji: &'static str,
  /// The type's tooltip.
  pub type_name: &'static str,
  /// "Chapters", localised, on the chapters row.
  pub codec: String,
  /// The localised entry count on the chapters row.
  pub description: String,
  pub size: String,
  pub bit_rate: String,
  pub language: String,
  pub track_name: String,
}

/// A row's cell texts: the ID column, the type emoji, the localised chapters
/// codec and entry count, size and bit rate with the configured precision
/// and unit.
pub fn row_texts(row: &CombinedTrack, qualified_ids: bool, formatting: &ConfigFormatting) -> RowTexts {
  let track = &row.track;
  let chapters = track.kind == MediaTrackKind::Chapters;
  RowTexts {
    id: if qualified_ids {
      format!("{}:{}", row.member_index, track.id)
    } else {
      track.id.to_string()
    },
    emoji: track_type_emoji(track.track_type),
    type_name: track.track_type.as_str(),
    codec: if chapters {
      tr!("settings.chapters").to_string()
    } else {
      track.codec.clone()
    },
    description: match (chapters, track.chapter_entry_count) {
      (true, Some(1)) => tr!("merge.chapterEntryCount.single", count = 1).to_string(),
      (true, Some(count)) => tr!("merge.chapterEntryCount.multiple", count = count).to_string(),
      _ => track.description.clone(),
    },
    size: track
      .size
      .map(|size| format_track_size(size, track.track_type, Some(formatting)))
      .unwrap_or_default(),
    bit_rate: track
      .bit_rate
      .map(|bit_rate| format_track_bit_rate(bit_rate, track.track_type, Some(formatting)))
      .unwrap_or_default(),
    language: track.language.clone(),
    track_name: track.track_name.clone(),
  }
}

/// What a row press needs at mouse-up, kept from the last render.
#[derive(Clone)]
struct RowInfo {
  key: String,
  ghost: SharedString,
}

/// The language options of each track type, built for the preferred
/// languages (video, audio, subtitle) of one profile.
#[derive(Default)]
struct LanguageOptionsCache {
  preferred: Option<(String, String, String)>,
  options: HashMap<MediaTrackType, (Rc<Vec<String>>, usize)>,
}

pub struct TrackTable {
  stores: Stores,
  scope: TableScope,
  focus: FocusHandle,
  /// UI row selection, by row key.
  selected: HashSet<String>,
  /// The keyboard cursor row.
  cursor: Option<String>,
  drag: DragState,
  rows: Vec<RowInfo>,
  row_bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
  cells: HashMap<(CellKind, String), (Entity<AutocompleteCell>, Subscription)>,
  language_options: LanguageOptionsCache,
  /// The scrolling list around the table, for auto-scroll while dragging.
  scroll: Option<ScrollHandle>,
  /// The table's own sideways scrolling, below its minimum width.
  scroll_x: ScrollHandle,
  _subscriptions: Vec<Subscription>,
}

impl TrackTable {
  pub fn new(stores: Stores, scope: TableScope, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.subscribe(&stores.files, |this, _, event: &FileEvent, cx| match event {
        FileEvent::Changed { files } => {
          if files.iter().any(|file| this.scope.files().any(|own| own == file)) {
            cx.notify();
          }
        }
        FileEvent::Highlight { .. } => {}
      }),
      cx.subscribe(&stores.queue, |this, _, event: &QueueEvent, cx| {
        if let QueueEvent::ItemChanged { file } = event
          && this.scope.roots().any(|root| root == file)
        {
          cx.notify();
        }
      }),
      cx.observe(&stores.config, |_, _, cx| cx.notify()),
    ];
    Self {
      stores,
      scope,
      focus: cx.focus_handle(),
      selected: HashSet::new(),
      cursor: None,
      drag: DragState::Idle,
      rows: Vec::new(),
      row_bounds: Rc::default(),
      cells: HashMap::new(),
      language_options: LanguageOptionsCache::default(),
      scroll: None,
      scroll_x: ScrollHandle::new(),
      _subscriptions: subscriptions,
    }
  }

  pub fn scope(&self) -> &TableScope {
    &self.scope
  }

  /// The card's units changed (a file joined, a group formed or split).
  pub fn set_scope(&mut self, scope: TableScope, cx: &mut Context<Self>) {
    if self.scope != scope {
      self.scope = scope;
      cx.notify();
    }
  }

  pub fn set_scroll_handle(&mut self, scroll: ScrollHandle) {
    self.scroll = Some(scroll);
  }

  pub fn focus_handle(&self) -> &FocusHandle {
    &self.focus
  }

  pub fn selected_rows(&self) -> &HashSet<String> {
    &self.selected
  }

  pub fn cursor(&self) -> Option<&str> {
    self.cursor.as_deref()
  }

  pub fn drag_state(&self) -> DragState {
    self.drag
  }

  /// The cell of a row, once the row has been rendered.
  pub fn cell(&self, kind: CellKind, row_key: &str) -> Option<Entity<AutocompleteCell>> {
    self
      .cells
      .get(&(kind, row_key.to_string()))
      .map(|(cell, _)| cell.clone())
  }

  /// The element id of this table, unique in the window.
  pub fn element_id(scope: &TableScope) -> SharedString {
    format!("tracks:{}", scope.id()).into()
  }

  /// Merging or waiting: the table takes no edits and no keys.
  pub fn is_disabled(&self, cx: &App) -> bool {
    let queue = self.stores.queue.read(cx);
    self
      .scope
      .roots()
      .any(|root| queue.item(root).is_some_and(|item| item.status.is_active()))
  }

  fn row_keys(&self, cx: &App) -> Vec<String> {
    let files = self.stores.files.read(cx);
    self.scope.rows(files).iter().map(CombinedTrack::row_key).collect()
  }

  /// A click on a row: the cursor moves there and its selection flips.
  fn toggle_row(&mut self, key: String, cx: &mut Context<Self>) {
    if !self.selected.remove(&key) {
      self.selected.insert(key.clone());
    }
    self.cursor = Some(key);
    cx.notify();
  }

  /// ↑/↓ on the active card: moves the cursor and flips the selection of
  /// the row it lands on. `false` when there are no rows.
  pub fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
    let keys = self.row_keys(cx);
    let current = self
      .cursor
      .as_ref()
      .and_then(|cursor| keys.iter().position(|key| key == cursor));
    let Some(next) = step_cursor(keys.len(), current, down) else {
      return false;
    };
    self.toggle_row(keys[next].clone(), cx);
    true
  }

  /// Ctrl/Cmd+A selects every row, Ctrl/Cmd+Shift+A none.
  pub fn select_all_rows(&mut self, all: bool, cx: &mut Context<Self>) {
    self.selected = if all {
      self.row_keys(cx).into_iter().collect()
    } else {
      HashSet::new()
    };
    cx.notify();
  }

  /// Space on the active card: flips the merge checkbox of every selected
  /// row. `false` when no row is selected.
  pub fn flip_selected(&mut self, cx: &mut Context<Self>) -> bool {
    let keys = self.row_keys(cx);
    let selected: Vec<String> = keys.into_iter().filter(|key| self.selected.contains(key)).collect();
    if selected.is_empty() {
      return false;
    }
    let scope = self.scope.clone();
    self
      .stores
      .files
      .update(cx, |files, cx| files.flip_rows_checked(&scope, &selected, cx));
    true
  }

  /// The rows an edit of `key` applies to: every selected row when `key` is
  /// selected, else `key` alone.
  fn target_rows(&self, key: &str) -> Vec<String> {
    let mut rows = vec![key.to_string()];
    if self.selected.contains(key) {
      rows.extend(self.selected.iter().filter(|selected| *selected != key).cloned());
    }
    rows
  }

  fn edit(
    &mut self,
    cx: &mut Context<Self>,
    edit: impl FnOnce(&mut crate::state::FileStore, &TableScope, &mut Context<crate::state::FileStore>),
  ) {
    let scope = self.scope.clone();
    self.stores.files.update(cx, |files, cx| edit(files, &scope, cx));
  }

  fn cell_edited(&mut self, kind: CellKind, key: &str, value: &str, cx: &mut Context<Self>) {
    let rows = self.target_rows(key);
    self.edit(cx, |files, scope, cx| match kind {
      CellKind::Language => files.set_rows_language(scope, &rows, value, cx),
      CellKind::TrackName => files.set_rows_name(scope, &rows, value, cx),
    });
  }

  fn on_row_mouse_down(&mut self, row: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    // A click away from the cells takes the focus out of them.
    window.focus(&self.focus, cx);
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
          cx.notify();
        } else if (event.position - origin).magnitude() > f64::from(ACTIVATION_DISTANCE)
          && self.scope.can_reorder()
          && !self.is_disabled(cx)
        {
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

  fn on_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
    match std::mem::replace(&mut self.drag, DragState::Idle) {
      DragState::Idle => return,
      DragState::Pressed { row, .. } => {
        // A press that ends on its row is a click (also when the row cannot
        // be dragged and the pointer wandered).
        let on_row = self
          .row_bounds
          .borrow()
          .get(row)
          .is_none_or(|bounds| bounds.contains(&event.position));
        if on_row && let Some(info) = self.rows.get(row) {
          self.toggle_row(info.key.clone(), cx);
        }
      }
      DragState::Dragging { row, insert_at, .. } => {
        if let Some(target) = move_target(row, insert_at)
          && let (Some(from), Some(to)) = (self.rows.get(row), self.rows.get(target))
        {
          let (from, to) = (from.key.clone(), to.key.clone());
          self.edit(cx, |files, scope, cx| files.reorder_rows(scope, &from, &to, cx));
        }
      }
    }
    cx.notify();
  }

  /// One step of scrolling when the pointer is near the top or bottom of
  /// the list while dragging.
  fn autoscroll(&self, y: Pixels) {
    let Some(scroll) = &self.scroll else {
      return;
    };
    let viewport = scroll.bounds();
    let offset = scroll.offset();
    let delta = if y < viewport.top() + px(AUTOSCROLL_EDGE) {
      AUTOSCROLL_STEP
    } else if y > viewport.bottom() - px(AUTOSCROLL_EDGE) {
      -AUTOSCROLL_STEP
    } else {
      return;
    };
    let max = scroll.max_offset().y;
    scroll.set_offset(point(offset.x, (offset.y + px(delta)).min(px(0.0)).max(-max)));
  }

  /// The language options of a track type for the active profile, built
  /// once per profile change.
  fn language_options(
    &mut self,
    profile: Option<&ConfigProfile>,
    track_type: MediaTrackType,
  ) -> (Rc<Vec<String>>, usize) {
    let preferred = profile.map(|profile| {
      (
        profile.preferred_video_languages.clone(),
        profile.preferred_audio_languages.clone(),
        profile.preferred_subtitle_languages.clone(),
      )
    });
    let cache = &mut self.language_options;
    if cache.preferred != preferred {
      cache.preferred = preferred;
      cache.options.clear();
    }
    cache
      .options
      .entry(track_type)
      .or_insert_with(|| {
        let built = build_language_options(profile, track_type);
        (Rc::new(built.options), built.preferred_count)
      })
      .clone()
  }

  /// Creates the cells of new rows, shows the rows' current values and
  /// options, and drops the cells of rows that are gone.
  fn sync_cells(&mut self, rows: &[CombinedTrack], disabled: bool, window: &mut Window, cx: &mut Context<Self>) {
    let profile = self.stores.files.read(cx).settings().profile.clone();
    let mut live = HashSet::new();
    for row in rows {
      if row.track.kind != MediaTrackKind::Track {
        continue;
      }
      let key = row.row_key();
      let (language_options, preferred) = self.language_options(profile.as_ref(), row.track.track_type);
      let name_options = Rc::new(build_track_name_options(
        profile.as_ref(),
        row.track.track_type,
        &row.track.language,
      ));
      for (kind, value, options, preferred) in [
        (CellKind::Language, &row.track.language, language_options, preferred),
        (CellKind::TrackName, &row.track.track_name, name_options, 0),
      ] {
        live.insert((kind, key.clone()));
        if let Some((cell, _)) = self.cells.get(&(kind, key.clone())) {
          cell.update(cx, |cell, cx| {
            cell.set_options(options, preferred, cx);
            cell.sync_value(value, window, cx);
            cell.set_disabled(disabled, window, cx);
          });
          continue;
        }
        let cell = cx.new(|cx| {
          let mut cell = AutocompleteCell::new(kind, &key, value, options, preferred, window, cx);
          cell.set_disabled(disabled, window, cx);
          cell
        });
        let row_key = key.clone();
        let subscription = cx.subscribe(&cell, move |this, _, CellEdited(value): &CellEdited, cx| {
          this.cell_edited(kind, &row_key, value, cx);
        });
        self.cells.insert((kind, key.clone()), (cell, subscription));
      }
    }
    self.cells.retain(|key, _| live.contains(key));
  }

  fn render_header(
    &self,
    columns: &TrackColumns,
    all: CheckboxState,
    disabled: bool,
    cx: &Context<Self>,
  ) -> impl IntoElement {
    let theme = cx.theme();
    let head = |column: Column| {
      columns.style(
        column,
        kit_base::TableHead::new(("head", column.index()), column.index()),
      )
    };
    let reset_button = |id: &'static str, icon: IconName, label: SharedString, default: bool, cx: &Context<Self>| {
      Button::new(id)
        .ghost()
        .xsmall()
        .icon(icon)
        .accessibility_label(label.clone())
        .tooltip(label)
        .disabled(disabled)
        .on_click(cx.listener(move |this, _, _, cx| {
          this.edit(cx, |files, scope, cx| {
            files.reset_rows_flags(scope, default, !default, cx)
          });
        }))
    };
    kit_base::TableHeader::new("track-head-group").child(
      kit_base::TableRow::new("track-head", 1)
        .flex()
        .h(px(ROW_HEIGHT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.table_head_foreground)
        .child(head(Column::Check).child(select_all_checkbox(all, disabled, cx)))
        .children(
          columns
            .headers
            .iter()
            .map(|(column, label)| head(*column).child(label.clone())),
        )
        .child(head(Column::Default).child(reset_button(
          "track-reset-default",
          IconName::Film,
          tr!("merge.header.defaultTrack"),
          true,
          cx,
        )))
        .child(head(Column::Forced).child(reset_button(
          "track-reset-forced",
          IconName::Tornado,
          tr!("merge.header.forcedDisplay"),
          false,
          cx,
        ))),
    )
  }

  fn render_row(
    &self,
    index: usize,
    (row, texts): (&CombinedTrack, RowTexts),
    checked: bool,
    disabled: bool,
    columns: &TrackColumns,
    cx: &Context<Self>,
  ) -> AnyElement {
    let theme = cx.theme();
    let key = row.row_key();
    let track = &row.track;
    let is_track = track.kind == MediaTrackKind::Track;
    let selected = self.selected.contains(&key);
    let cursor = self.cursor.as_deref() == Some(key.as_str());
    let (dragged, line_before, line_after) = match self.drag {
      DragState::Dragging { row, insert_at, .. } => (
        row == index,
        insert_at == index,
        insert_at == self.rows.len() && index + 1 == self.rows.len(),
      ),
      _ => (false, false, false),
    };
    let draggable = self.scope.can_reorder() && !disabled;
    let cell = |column: Column| {
      columns.style(
        column,
        kit_base::TableCell::new(("cell", column.index()), column.index()),
      )
    };
    // Controls take their own clicks: no row selection, no drag.
    let control = |column: Column| cell(column).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

    let type_name = SharedString::new(texts.type_name);

    let text_cell = |kind: CellKind, value: &str| -> AnyElement {
      match self.cells.get(&(kind, key.clone())) {
        Some((cell, _)) if is_track => cell.clone().into_any_element(),
        _ => div().truncate().child(value.to_string()).into_any_element(),
      }
    };
    let flag_button = |kind: FlagKind, cx: &Context<Self>| -> Option<Button> {
      if !is_track {
        return None;
      }
      let (id, label) = match kind {
        FlagKind::Default => ("track-default", tr!("merge.header.defaultTrack")),
        FlagKind::Forced => ("track-forced", tr!("merge.header.forcedDisplay")),
      };
      let theme = cx.theme();
      let icon = match flag_of(track, kind) {
        TrackFlag::True => Icon::new(IconName::Check).text_color(theme.success),
        TrackFlag::False => Icon::new(IconName::X).text_color(theme.danger),
        TrackFlag::Unspecified => Icon::new(IconName::Square).text_color(theme.muted_foreground.opacity(0.6)),
      };
      let clicked = key.clone();
      Some(
        Button::new((id, index))
          .ghost()
          .xsmall()
          .icon(icon)
          .accessibility_label(label)
          .disabled(disabled)
          .on_click(cx.listener(move |this, _, _, cx| {
            let rows = this.target_rows(&clicked);
            let clicked = clicked.clone();
            this.edit(cx, |files, scope, cx| {
              files.cycle_rows_flag(scope, &clicked, &rows, kind, cx)
            });
          })),
      )
    };
    let check_key = key.clone();
    let default_button = flag_button(FlagKind::Default, cx);
    let forced_button = flag_button(FlagKind::Forced, cx);
    let theme_primary = theme.primary;

    kit_base::TableRow::new(("track-row", index), index + 2)
      .relative()
      .flex()
      .min_h(px(ROW_HEIGHT))
      .border_t_1()
      .border_color(theme.table_row_border)
      .when(selected, |row| row.bg(crate::theme::selection_highlight(theme)))
      .when(!selected && self.drag == DragState::Idle, |row| {
        let hover = theme.table_hover;
        row.hover(move |style| style.bg(hover))
      })
      .when(dragged, |row| row.opacity(0.4))
      .when(draggable, |row| row.cursor(CursorStyle::OpenHand))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, event: &MouseDownEvent, window, cx| this.on_row_mouse_down(index, event, window, cx)),
      )
      .child(
        control(Column::Check).child(
          Checkbox::new(("track-check", index))
            .small()
            .checked(checked)
            .disabled(disabled)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
              let rows = this.target_rows(&check_key);
              let checked = *checked;
              this.edit(cx, |files, scope, cx| files.set_rows_checked(scope, &rows, checked, cx));
            })),
        ),
      )
      .child(cell(Column::Id).child(texts.id))
      .child(
        cell(Column::Type).child(
          div()
            .id(("track-type", index))
            .child(texts.emoji)
            .tooltip(move |window, cx| Tooltip::new(type_name.clone()).build(window, cx)),
        ),
      )
      .child(cell(Column::Codec).child(div().truncate().child(texts.codec)))
      .child(cell(Column::Description).child(div().truncate().child(texts.description)))
      .child(cell(Column::Size).child(texts.size))
      .child(cell(Column::BitRate).child(texts.bit_rate))
      .child(control(Column::Language).child(text_cell(CellKind::Language, &texts.language)))
      .child(control(Column::Name).child(text_cell(CellKind::TrackName, &texts.track_name)))
      .child(control(Column::Default).children(default_button))
      .child(control(Column::Forced).children(forced_button))
      .when(cursor, |row| {
        row.child(
          div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .border_2()
            .border_dashed()
            .border_color(theme_primary),
        )
      })
      .when(line_before || line_after, |row| {
        let line = div().absolute().left_0().right_0().h(px(2.0)).bg(theme_primary);
        row.child(if line_before {
          line.top(px(-1.0))
        } else {
          line.bottom(px(-1.0))
        })
      })
      .into_any_element()
  }

  /// The pointer listeners of a press or a drag, for the whole window: the
  /// pointer may leave the row and the table.
  fn render_pointer_listeners(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
    if self.drag == DragState::Idle {
      return None;
    }
    let entity = cx.entity();
    Some(
      canvas(
        |_, _, _| {},
        move |_, _, window, _| {
          let moves = entity.clone();
          window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
              moves.update(cx, |table, cx| table.on_mouse_move(event, cx));
            }
          });
          let ups = entity.clone();
          window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
              ups.update(cx, |table, cx| table.on_mouse_up(event, cx));
            }
          });
        },
      )
      .absolute()
      .top_0()
      .left_0()
      .size_full(),
    )
  }

  fn render_ghost(&self, cx: &App) -> Option<impl IntoElement> {
    let DragState::Dragging { row, cursor, .. } = self.drag else {
      return None;
    };
    let label = self.rows.get(row)?.ghost.clone();
    let theme = cx.theme();
    Some(
      deferred(
        anchored().position(cursor + point(px(12.0), px(4.0))).child(
          div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .child(label),
        ),
      )
      .with_priority(1),
    )
  }
}

/// The select-all checkbox: ticked when every row is, indeterminate when
/// some are.
fn select_all_checkbox(state: CheckboxState, disabled: bool, cx: &Context<TrackTable>) -> impl IntoElement {
  let table = cx.entity().downgrade();
  crate::ui::checkbox::tri_state_checkbox(
    "track-check-all",
    state,
    disabled,
    tr!("merge.header.selectAll"),
    cx,
    move |checked, _, cx| {
      let _ = table.update(cx, |this, cx| {
        this.edit(cx, |files, scope, cx| files.set_all_rows_checked(scope, checked, cx));
      });
    },
  )
}

impl Render for TrackTable {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let id = Self::element_id(&self.scope);
    let files = self.stores.files.read(cx);
    // Loading wins over an error.
    let mut load_error = None;
    for file in self.scope.files() {
      match files.load_state(file) {
        LoadState::Loading => {
          return div()
            .id(id)
            .test_support()
            .flex()
            .justify_center()
            .py(px(8.0))
            .child(Spinner::new().small())
            .into_any_element();
        }
        LoadState::Failed(error) if load_error.is_none() => load_error = Some(metadata_error_message(error)),
        _ => {}
      }
    }
    if let Some(error) = load_error {
      return div()
        .id(id)
        .test_support()
        .text_color(cx.theme().danger)
        .child(crate::i18n::translate(&error))
        .into_any_element();
    }
    let rows = self.scope.rows(files);
    if rows.is_empty() {
      return div()
        .id(id)
        .test_support()
        .p(px(8.0))
        .text_color(cx.theme().muted_foreground)
        .child(tr!("merge.noTracks"))
        .into_any_element();
    }
    let checked = self.scope.checked_rows(files);
    let disabled = self.is_disabled(cx);

    // Forget rows that are gone.
    let keys: HashSet<String> = rows.iter().map(CombinedTrack::row_key).collect();
    self.selected.retain(|key| keys.contains(key));
    if self.cursor.as_ref().is_some_and(|cursor| !keys.contains(cursor)) {
      self.cursor = None;
    }
    self.rows = rows
      .iter()
      .map(|row| RowInfo {
        key: row.row_key(),
        ghost: format!(
          "{} {} {}",
          row.track.id,
          track_type_emoji(row.track.track_type),
          row.track.codec
        )
        .into(),
      })
      .collect();
    self.sync_cells(&rows, disabled, window, cx);

    let checked_count = rows.iter().filter(|row| checked.contains(&row.row_key())).count();
    let all = match checked_count {
      0 => CheckboxState::Unchecked,
      count if count == rows.len() => CheckboxState::Checked,
      _ => CheckboxState::Indeterminate,
    };
    let formatting = &self.stores.config.read(cx).config().formatting;
    let texts: Vec<RowTexts> = rows
      .iter()
      .map(|row| row_texts(row, self.scope.qualified_ids(), formatting))
      .collect();
    let columns = TrackColumns::measure(&texts, window);
    let row_elements: Vec<AnyElement> = rows
      .iter()
      .zip(texts)
      .enumerate()
      .map(|(index, (row, texts))| {
        let is_checked = checked.contains(&row.row_key());
        self.render_row(index, (row, texts), is_checked, disabled, &columns, cx)
      })
      .collect();
    let row_bounds = self.row_bounds.clone();
    let header = self.render_header(&columns, all, disabled, cx);
    let theme = cx.theme();

    div()
      .id(id)
      .test_support()
      .track_focus(&self.focus)
      .key_context("TrackTable")
      .relative()
      .w_full()
      .overflow_x_scroll()
      // A vertical wheel scrolls the list, not the table sideways.
      .restrict_scroll_to_axis()
      .track_scroll(&self.scroll_x)
      .text_size(super::text::body2())
      // Always registered, unlike the window listeners, which start with the
      // frame after a press: a quick release must still end it.
      .on_mouse_up(
        MouseButton::Left,
        cx.listener(|this, event: &MouseUpEvent, _, cx| this.on_mouse_up(event, cx)),
      )
      .on_mouse_up_out(
        MouseButton::Left,
        cx.listener(|this, event: &MouseUpEvent, _, cx| this.on_mouse_up(event, cx)),
      )
      .child(
        kit_base::Table::new("track-table")
          .accessibility_label(tr!("merge.header.tracks"))
          .w_full()
          .min_w(columns.min_width())
          .bg(theme.table)
          .child(header)
          .child(
            kit_base::TableBody::new("track-body").child(
              v_flex()
                .on_children_prepainted(move |bounds, _, _| *row_bounds.borrow_mut() = bounds)
                .children(row_elements),
            ),
          ),
      )
      .children(self.render_pointer_listeners(cx))
      .children(self.render_ghost(cx))
      .horizontal_scrollbar(&self.scroll_x)
      .into_any_element()
  }
}
