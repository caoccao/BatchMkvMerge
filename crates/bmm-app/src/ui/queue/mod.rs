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

//! The Queue tab: one card per drive (`C:`, `\\SERVER\SHARE` on Windows; one
//! "Queue" card elsewhere), titled with the drive, whose header has the System
//! Notification and Topmost Notification toggles, Resume All, Cancel All and
//! Clear Completed, above a table of the drive's files: File Path, Status
//! (with the percentage while merging, coloured by status, the error as a
//! tooltip when failed), Start, End, Elapsed, ETA and Cancel while merging.
//! Without any item one empty card shows.
//!
//! Each row is a [`QueueRow`] entity drawn as a cached view at its measured
//! height, so a progress event or a clock tick re-renders the merging row, not
//! the table; the view itself re-renders on structural changes. A table
//! narrower than its columns scrolls sideways.

pub mod row;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use gpui_kit::assets::IconName;
use gpui_kit::base::{TestSupportExt as _, TestTextExt as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

pub use row::QueueRow;

use crate::domain::status::QueueItemStatus;
use crate::state::Stores;
use crate::ui::cards::icon_button;

/// Width of the Status column: the longest status, German
/// "Zusammenführen 100%", fits on one line.
pub const STATUS_WIDTH: f32 = 160.0;
/// Width of the Start, End, Elapsed and ETA columns: the longest header,
/// Spanish "Transcurrido", fits on one line.
pub const TIME_WIDTH: f32 = 92.0;
/// Width of the column holding a merging row's Cancel.
pub const ACTION_WIDTH: f32 = 36.0;
/// The narrowest File Path column; a narrower table scrolls sideways.
pub const PATH_MIN_WIDTH: f32 = 200.0;

/// The narrowest table: the path's minimum and the fixed columns.
fn table_min_width() -> Pixels {
  px(PATH_MIN_WIDTH + STATUS_WIDTH + 4.0 * TIME_WIDTH + ACTION_WIDTH)
}

/// The title of a drive's card: its key on Windows, "Queue" for the single
/// drive elsewhere (and for the empty card).
pub fn drive_label(drive: &str) -> SharedString {
  if drive.is_empty() || drive == "default" {
    tr!("queue.defaultDrive")
  } else {
    drive.to_string().into()
  }
}

pub struct QueueView {
  stores: Stores,
  rows: HashMap<String, Entity<QueueRow>>,
  /// The sideways scroll position of each drive's table.
  scrolls: HashMap<String, ScrollHandle>,
  _subscriptions: Vec<Subscription>,
}

impl QueueView {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      // Items added, removed or with a new status.
      cx.observe(&stores.queue, |this, _, cx| {
        this.sync_rows(cx);
        cx.notify();
      }),
      // The notification toggles.
      cx.observe(&stores.ui, |_, _, cx| cx.notify()),
    ];
    let mut view = Self {
      stores,
      rows: HashMap::new(),
      scrolls: HashMap::new(),
      _subscriptions: subscriptions,
    };
    view.sync_rows(cx);
    view
  }

  /// The row of `file`, if it is listed.
  pub fn row(&self, file: &str) -> Option<&Entity<QueueRow>> {
    self.rows.get(file)
  }

  /// Keeps a row for every listed file.
  fn sync_rows(&mut self, cx: &mut Context<Self>) {
    let queue = self.stores.queue.read(cx);
    let files: Vec<String> = queue.items().map(|item| item.file.clone()).collect();
    self.rows.retain(|file, _| files.contains(file));
    for file in files {
      if !self.rows.contains_key(&file) {
        let stores = self.stores.clone();
        let row = cx.new(|cx| QueueRow::new(stores, file.clone(), cx));
        self.rows.insert(file, row);
      }
    }
  }

  fn row_element(&self, file: &str, cache: bool, cx: &App) -> Option<AnyElement> {
    let row = self.rows.get(file)?;
    Some(row.read(cx).height().view(row, cache))
  }

  fn drive_card(&self, drive: &str, files: &[String], cache: bool, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let (primary, success, danger, border) = (theme.primary, theme.success, theme.danger, theme.border);
    let queue = self.stores.queue.read(cx);
    let statuses: Vec<QueueItemStatus> = files
      .iter()
      .filter_map(|file| queue.item(file).map(|item| item.status))
      .collect();
    let has_finished = statuses.iter().any(|status| status.is_terminal());
    let has_active = statuses.iter().any(|status| status.is_active());
    let has_resumable = statuses
      .iter()
      .any(|status| matches!(status, QueueItemStatus::Cancelled | QueueItemStatus::Failed));
    let ui = self.stores.ui.read(cx);
    let (system, topmost) = (ui.system_notification(), ui.topmost_notification());
    let id = |name: &str| SharedString::from(format!("{name}:{drive}"));
    let actions = h_flex()
      .flex_none()
      .child(
        icon_button(
          id("queue-system"),
          IconName::Bell,
          system.then_some(primary),
          tr!("queue.systemNotificationTooltip"),
          true,
        )
        .on_click(cx.listener(|this, _, _, cx| this.stores.ui.update(cx, |ui, cx| ui.toggle_system_notification(cx)))),
      )
      .child(
        icon_button(
          id("queue-topmost"),
          IconName::PictureInPicture2,
          topmost.then_some(primary),
          tr!("queue.topmostNotificationTooltip"),
          true,
        )
        .on_click(cx.listener(|this, _, _, cx| this.stores.ui.update(cx, |ui, cx| ui.toggle_topmost_notification(cx)))),
      )
      .child({
        let drive = drive.to_string();
        icon_button(
          id("queue-resume"),
          IconName::RotateCcw,
          Some(success),
          tr!("queue.resumeAll"),
          has_resumable,
        )
        .on_click(cx.listener(move |this, _, _, cx| crate::actions::resume_drive(&this.stores, &drive, cx).detach()))
      })
      .child({
        let drive = drive.to_string();
        icon_button(
          id("queue-cancel-all"),
          IconName::CircleX,
          Some(danger),
          tr!("queue.cancelAll"),
          has_active,
        )
        .on_click(cx.listener(move |this, _, _, cx| crate::actions::cancel_drive(&this.stores, &drive, cx)))
      })
      .child({
        let drive = drive.to_string();
        icon_button(
          id("queue-clear"),
          IconName::BrushCleaning,
          None,
          tr!("queue.clearCompleted"),
          has_finished,
        )
        .on_click(cx.listener(move |this, _, _, cx| crate::actions::clear_completed_in_drive(&this.stores, &drive, cx)))
      });
    let title = drive_label(drive);
    let header_cell = |key: &str| -> AnyElement {
      let label = tr!(key);
      // One line: a narrow column clips its header rather than breaking a
      // word in two.
      div()
        .px(px(8.0))
        .py(px(6.0))
        .whitespace_nowrap()
        .overflow_hidden()
        .font_weight(FontWeight::MEDIUM)
        .child(label.clone())
        .test_text(label)
        .into_any_element()
    };
    let fixed = |key: &str, width: f32| div().flex_none().w(px(width)).child(header_cell(key));
    let header = h_flex()
      .w_full()
      .border_b_1()
      .border_color(border)
      .child(div().flex_1().min_w_0().child(header_cell("queue.header.filePath")))
      .child(fixed("queue.header.status", STATUS_WIDTH))
      .child(fixed("queue.header.start", TIME_WIDTH))
      .child(fixed("queue.header.end", TIME_WIDTH))
      .child(fixed("queue.header.elapsed", TIME_WIDTH))
      .child(fixed("queue.header.eta", TIME_WIDTH))
      .child(div().flex_none().w(px(ACTION_WIDTH)));
    let rows: Vec<AnyElement> = files
      .iter()
      .filter_map(|file| self.row_element(file, cache, cx))
      .collect();
    let scroll = self.scrolls.get(drive).cloned().unwrap_or_default();
    v_flex()
      .id(id("queue-drive"))
      .test_support()
      .w_full()
      .border_1()
      .rounded(px(4.0))
      .border_color(border)
      .text_size(super::text::body2())
      .child(
        h_flex()
          .items_center()
          .gap(super::space(1.0))
          .px(super::space(2.0))
          .pt(super::space(1.0))
          .child(
            div()
              .flex_1()
              .min_w_0()
              .truncate()
              .text_size(super::text::h6())
              .child(title.clone())
              .test_text(title),
          )
          .child(actions),
      )
      .child(
        div().w_full().px(super::space(2.0)).pb(super::space(2.0)).child(
          div()
            .id(id("queue-table"))
            .relative()
            .w_full()
            .overflow_x_scroll()
            // A vertical wheel scrolls the tab, not the table sideways.
            .restrict_scroll_to_axis()
            .track_scroll(&scroll)
            .child(v_flex().w_full().min_w(table_min_width()).child(header).children(rows))
            .horizontal_scrollbar(&scroll),
        ),
      )
  }
}

impl Render for QueueView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let mut drives = self.stores.queue.read(cx).drives();
    if drives.is_empty() {
      drives.push((String::new(), Vec::new()));
    }
    self
      .scrolls
      .retain(|drive, _| drives.iter().any(|(listed, _)| listed == drive));
    for (drive, _) in &drives {
      self.scrolls.entry(drive.clone()).or_default();
    }
    // A screen reader sees cached rows only on the frames that render them.
    let cache = !window.is_a11y_active();
    let cards: Vec<AnyElement> = drives
      .iter()
      .map(|(drive, files)| self.drive_card(drive, files, cache, cx).into_any_element())
      .collect();
    v_flex()
      .id("queue")
      .test_support()
      .w_full()
      .gap(super::space(2.0))
      .p(super::space(1.0))
      .children(cards)
  }
}
