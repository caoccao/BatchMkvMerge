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

//! One row of a drive's queue table, an entity that repaints itself when
//! its item changes or, while it merges, when the clock ticks.

use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ACTION_WIDTH, STATUS_WIDTH, TIME_WIDTH};
use crate::domain::format::{format_clock, format_hms};
use crate::domain::status::{QueueItem, QueueItemStatus};
use crate::state::Stores;
use crate::state::queue::QueueEvent;
use crate::ui::cards::{CardHeight, icon_button, text};

/// The Status text: the label, with the percentage while merging.
pub fn status_text(item: &QueueItem) -> String {
  let label = rust_i18n::t!(item.status.label_key());
  if item.status == QueueItemStatus::Merging {
    format!("{label} {}%", item.progress)
  } else {
    label.into_owned()
  }
}

pub struct QueueRow {
  stores: Stores,
  file: String,
  height: CardHeight,
  renders: usize,
  _subscriptions: Vec<Subscription>,
}

impl QueueRow {
  pub fn new(stores: Stores, file: String, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.subscribe(
        &stores.queue,
        |this: &mut Self, queue, event: &QueueEvent, cx| match event {
          QueueEvent::ItemChanged { file } if *file == this.file => cx.notify(),
          QueueEvent::Tick
            if queue
              .read(cx)
              .item(&this.file)
              .is_some_and(|item| item.status == QueueItemStatus::Merging) =>
          {
            cx.notify()
          }
          _ => {}
        },
      ),
    ];
    Self {
      stores,
      file,
      height: CardHeight::default(),
      renders: 0,
      _subscriptions: subscriptions,
    }
  }

  pub fn file(&self) -> &str {
    &self.file
  }

  pub fn height(&self) -> &CardHeight {
    &self.height
  }

  pub fn render_count(&self) -> usize {
    self.renders
  }

  /// The element id of the row of `file`.
  pub fn element_id(file: &str) -> SharedString {
    format!("queue-row:{file}").into()
  }
}

impl Render for QueueRow {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    self.renders += 1;
    let theme = cx.theme();
    let (border, success, danger, foreground) = (theme.border, theme.success, theme.danger, theme.foreground);
    let Some(item) = self.stores.queue.read(cx).item(&self.file).cloned() else {
      return div().id(Self::element_id(&self.file)).into_any_element();
    };
    let now = SystemTime::now();
    let color = match item.status {
      QueueItemStatus::Merging | QueueItemStatus::Completed => success,
      QueueItemStatus::Cancelled | QueueItemStatus::Failed => danger,
      QueueItemStatus::Waiting => foreground,
    };
    let cell = |content: AnyElement| div().px(px(8.0)).py(px(6.0)).child(content);
    let fixed = |width: f32, value: String| {
      div()
        .flex_none()
        .w(px(width))
        .child(cell(text(value).into_any_element()))
    };
    let error = item.error.clone().filter(|_| item.status == QueueItemStatus::Failed);
    let status = div()
      .id("queue-status")
      .test_support()
      .flex_none()
      .w(px(STATUS_WIDTH))
      .text_color(color)
      .child(cell(text(status_text(&item)).into_any_element()))
      .when_some(error, |status, error| {
        status.tooltip(move |window, cx| Tooltip::new(error.clone()).build(window, cx))
      });
    let cancel = (item.status == QueueItemStatus::Merging).then(|| {
      let file = self.file.clone();
      icon_button(
        "queue-cancel",
        IconName::CircleX,
        Some(danger),
        tr!("merge.cancel"),
        true,
      )
      .on_click(cx.listener(move |this, _, _, cx| crate::actions::cancel_merge(&this.stores, &file, cx)))
    });
    h_flex()
      .id(Self::element_id(&self.file))
      .test_support()
      .relative()
      .w_full()
      .items_center()
      .border_b_1()
      .border_color(border)
      .child(
        div()
          .flex_1()
          .min_w_0()
          .child(cell(text(self.file.clone()).into_any_element())),
      )
      .child(status)
      .child(fixed(TIME_WIDTH, format_clock(item.merge_started_at)))
      .child(fixed(TIME_WIDTH, format_clock(item.merge_ended_at)))
      .child(fixed(TIME_WIDTH, format_hms(item.elapsed(now))))
      .child(fixed(TIME_WIDTH, format_hms(item.eta(now))))
      .child(
        h_flex()
          .flex_none()
          .w(px(ACTION_WIDTH))
          .justify_center()
          .children(cancel),
      )
      .child(self.height.probe())
      .into_any_element()
  }
}
