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

//! Prototype 2: the topmost completion notification, a second GPUI window of
//! kind `PopUp` (440×200, undecorated, centred on the main window's display
//! work area). A second completion reuses the open window and updates it in
//! place; Esc or Close removes it.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

pub const KEY_CONTEXT: &str = "TopmostNotification";
pub const WIDTH: f32 = 440.0;
pub const HEIGHT: f32 = 200.0;

actions!(spike_topmost, [Dismiss]);

pub fn bind_keys(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(KEY_CONTEXT))]);
}

pub struct TopmostNotification {
  pub title: SharedString,
  pub file: SharedString,
  pub detail: SharedString,
  focus: FocusHandle,
}

impl TopmostNotification {
  fn new(cx: &mut Context<Self>) -> Self {
    Self {
      title: SharedString::default(),
      file: SharedString::default(),
      detail: SharedString::default(),
      focus: cx.focus_handle(),
    }
  }
}

impl Render for TopmostNotification {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    v_flex()
      .id("topmost-notification")
      .track_focus(&self.focus)
      .key_context(KEY_CONTEXT)
      .on_action(cx.listener(|_, _: &Dismiss, window, _| window.remove_window()))
      .size_full()
      .p_4()
      .gap_2()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .border_1()
      .border_color(cx.theme().border)
      .child(
        div()
          .text_lg()
          .font_weight(FontWeight::SEMIBOLD)
          .child(self.title.clone()),
      )
      .child(div().truncate().child(self.file.clone()))
      .child(
        div()
          .flex_1()
          .text_color(cx.theme().muted_foreground)
          .child(self.detail.clone()),
      )
      .child(
        h_flex().justify_end().child(
          Button::new("topmost-close")
            .primary()
            .label("Close")
            .on_click(|_, window, _| window.remove_window()),
        ),
      )
  }
}

pub struct TopmostDemo {
  window: Option<AnyWindowHandle>,
  view: Option<Entity<TopmostNotification>>,
  shown: usize,
  pub opened_windows: usize,
}

impl TopmostDemo {
  pub fn new() -> Self {
    Self {
      window: None,
      view: None,
      shown: 0,
      opened_windows: 0,
    }
  }

  pub fn popup(&self) -> Option<AnyWindowHandle> {
    self.window
  }

  /// Shows the next completion, reusing the popup while it is open.
  pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.shown += 1;
    let title: SharedString = if self.shown % 2 == 1 {
      "Merge completed".into()
    } else {
      "Merge failed".into()
    };
    let file: SharedString = format!("/Volumes/Media/Show/Episode {:02}.mkv", self.shown).into();
    let detail: SharedString = format!("Completion #{} of this session", self.shown).into();

    if let (Some(handle), Some(view)) = (self.window, self.view.clone()) {
      let reused = handle.update(cx, |_, popup, cx| {
        view.update(cx, |view, cx| {
          view.title = title.clone();
          view.file = file.clone();
          view.detail = detail.clone();
          cx.notify();
        });
        popup.activate_window();
      });
      if reused.is_ok() {
        cx.notify();
        return;
      }
    }

    let center = window
      .display(cx)
      .map(|display| display.visible_bounds().center())
      .unwrap_or_else(|| window.bounds().center());
    let options = WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::centered_at(
        center,
        size(px(WIDTH), px(HEIGHT)),
      ))),
      titlebar: None,
      kind: WindowKind::PopUp,
      is_resizable: false,
      is_minimizable: false,
      is_movable: false,
      focus: true,
      show: true,
      display_id: window.display(cx).map(|display| display.id()),
      ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |popup, cx| {
      let view = cx.new(|cx| {
        let mut view = TopmostNotification::new(cx);
        view.title = title;
        view.file = file;
        view.detail = detail;
        view
      });
      view.read(cx).focus.clone().focus(popup, cx);
      view
    });
    match opened {
      Ok((handle, view)) => {
        self.window = Some(handle);
        self.view = Some(view);
        self.opened_windows += 1;
      }
      Err(error) => log::error!("Failed to open the topmost window: {error}"),
    }
    cx.notify();
  }
}

impl Render for TopmostDemo {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    v_flex()
      .gap_3()
      .p_4()
      .child("A PopUp-kind window that should stay above other applications and close on Esc.")
      .child(
        h_flex().gap_2().child(
          Button::new("topmost-show")
            .primary()
            .label("Show completion")
            .on_click(cx.listener(|this, _, window, cx| this.show(window, cx))),
        ),
      )
      .child(div().text_color(cx.theme().muted_foreground).child(format!(
        "Completions shown: {}, windows opened: {}",
        self.shown, self.opened_windows
      )))
  }
}
