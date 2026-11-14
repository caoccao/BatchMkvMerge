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

//! Prototype 1: taskbar / Dock / launcher progress through `bmm-platform`.

use std::time::Duration;

use batch_mkvmerge::DESKTOP_ID;
use bmm_platform::TaskbarProgress;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

pub struct ProgressDemo {
  taskbar: Option<TaskbarProgress>,
  percent: Option<u8>,
  animation: Option<Task<()>>,
}

impl ProgressDemo {
  pub fn new() -> Self {
    Self {
      taskbar: None,
      percent: None,
      animation: None,
    }
  }

  pub fn percent(&self) -> Option<u8> {
    self.percent
  }

  pub fn set(&mut self, percent: Option<u8>, window: &mut Window, cx: &mut Context<Self>) {
    self.percent = percent;
    self
      .taskbar
      .get_or_insert_with(|| TaskbarProgress::new(window, DESKTOP_ID))
      .set(percent);
    cx.notify();
  }

  fn animate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.set(Some(0), window, cx);
    self.animation = Some(cx.spawn_in(window, async move |this, cx| {
      for percent in (0..=100u8).step_by(2) {
        cx.background_executor().timer(Duration::from_millis(60)).await;
        let updated = this.update_in(cx, |this, window, cx| this.set(Some(percent), window, cx));
        if updated.is_err() {
          return;
        }
      }
      cx.background_executor().timer(Duration::from_millis(800)).await;
      let _ = this.update_in(cx, |this, window, cx| this.set(None, window, cx));
    }));
  }
}

impl Render for ProgressDemo {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let status = match self.percent {
      Some(percent) => format!("Showing {percent} %"),
      None => "Hidden".to_string(),
    };
    v_flex()
      .gap_3()
      .p_4()
      .child("Taskbar button (Windows), Dock tile (macOS) or launcher icon (Linux, Unity LauncherEntry).")
      .child(
        h_flex()
          .gap_2()
          .child(
            Button::new("progress-0")
              .label("0 %")
              .on_click(cx.listener(|this, _, window, cx| {
                this.animation = None;
                this.set(Some(0), window, cx);
              })),
          )
          .child(
            Button::new("progress-plus")
              .label("+10 %")
              .on_click(cx.listener(|this, _, window, cx| {
                this.animation = None;
                let next = this.percent.map_or(10, |p| p.saturating_add(10).min(100));
                this.set(Some(next), window, cx);
              })),
          )
          .child(
            Button::new("progress-animate")
              .primary()
              .label("Animate 0 → 100 %")
              .on_click(cx.listener(|this, _, window, cx| this.animate(window, cx))),
          )
          .child(
            Button::new("progress-hide")
              .label("Hide")
              .on_click(cx.listener(|this, _, window, cx| {
                this.animation = None;
                this.set(None, window, cx);
              })),
          ),
      )
      .child(div().text_color(cx.theme().muted_foreground).child(status))
  }
}
