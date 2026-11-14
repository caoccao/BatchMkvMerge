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
//! otherwise one plain row per file with its load state (parsing, track
//! counts or the parse error) and its queue status. The rows become merge
//! cards with the cards.

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;

use super::welcome::Welcome;
use crate::domain::format::format_track_counts;
use crate::domain::metadata_error::metadata_error_message;
use crate::domain::status::QueueItemStatus;
use crate::state::Stores;
use crate::state::files::{FileEvent, LoadState};
use crate::state::queue::QueueEvent;

pub struct FileListView {
  stores: Stores,
  _subscriptions: Vec<Subscription>,
}

impl FileListView {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      // The plain rows show every file's state, so any change repaints them.
      cx.subscribe(&stores.files, |_, _, event: &FileEvent, cx| {
        if matches!(event, FileEvent::Changed { .. }) {
          cx.notify();
        }
      }),
      cx.subscribe(&stores.queue, |_, _, event: &QueueEvent, cx| {
        if matches!(event, QueueEvent::ItemChanged { .. }) {
          cx.notify();
        }
      }),
    ];
    Self {
      stores,
      _subscriptions: subscriptions,
    }
  }
}

impl Render for FileListView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let files = self.stores.files.read(cx);
    if !files.has_files() {
      return Welcome::new(self.stores.clone()).into_any_element();
    }
    let queue = self.stores.queue.read(cx);
    let theme = cx.theme();
    let rows: Vec<AnyElement> = files
      .files()
      .iter()
      .enumerate()
      .map(|(index, file)| {
        let load = match files.load_state(file) {
          LoadState::Loading => Spinner::new().xsmall().into_any_element(),
          LoadState::Loaded => {
            let counts = files.track_counts(file).map(|counts| format_track_counts(&counts));
            let label = counts
              .filter(|counts| !counts.is_empty())
              .map(SharedString::from)
              .unwrap_or_else(|| tr!("merge.noTracks"));
            div().text_color(theme.muted_foreground).child(label).into_any_element()
          }
          LoadState::Failed(error) => div()
            .text_color(theme.danger)
            .child(crate::i18n::translate(&metadata_error_message(error)))
            .into_any_element(),
        };
        let status = queue.item(file).map(|item| {
          let color = match item.status {
            QueueItemStatus::Merging | QueueItemStatus::Completed => theme.success,
            QueueItemStatus::Cancelled | QueueItemStatus::Failed => theme.danger,
            QueueItemStatus::Waiting => theme.foreground,
          };
          let mut label = tr!(item.status.label_key()).to_string();
          if item.status == QueueItemStatus::Merging {
            label.push_str(&format!(" {}%", item.progress));
          }
          div().text_color(color).child(label)
        });
        h_flex()
          .id(("file-row", index))
          .test_support()
          .gap(super::space(1.0))
          .child(div().child(file.clone()))
          .child(load)
          .children(status)
          .into_any_element()
      })
      .collect();
    v_flex()
      .id("file-rows")
      .test_support()
      .gap(super::space(0.5))
      .text_size(super::text::body2())
      .children(rows)
      .into_any_element()
  }
}
