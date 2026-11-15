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

//! The output path dialog: a path field with a clear button and Browse, a
//! warning while the directory does not exist (checked 300 ms after the last
//! edit), and a writable check on OK.
//!
//! In directory mode the field holds an output folder and Browse picks one;
//! in file mode it holds a full output file path, Browse opens a save dialog
//! and the warning looks at the file's directory. An empty field confirms
//! "no override". Enter confirms, Escape cancels.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{TestSupportExt as _, TestTextExt as _};
use gpui_kit::component::WindowExt;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// How long the field must rest before the existence check runs.
const EXISTS_CHECK_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
  /// An output folder (group cards, the toolbar's global path).
  Directory,
  /// A full output file path (a file card).
  File,
}

/// Receives the confirmed path, empty to clear the override.
pub type OnConfirm = Rc<dyn Fn(String, &mut App)>;

/// What the dialog edits and what happens on OK.
#[derive(Clone)]
pub struct OutputDialogRequest {
  pub title: SharedString,
  pub initial: String,
  pub mode: OutputMode,
  /// The save dialog's suggested name in file mode (the input file's).
  pub default_file_name: String,
  pub on_confirm: OnConfirm,
}

/// The directory the existence warning looks at: the path itself, or in
/// file mode the file's directory (`.` for a bare name).
pub fn checked_dir(value: &str, mode: OutputMode) -> String {
  match mode {
    OutputMode::Directory => value.to_string(),
    OutputMode::File => match Path::new(value).parent() {
      Some(parent) if !parent.as_os_str().is_empty() => parent.to_string_lossy().into_owned(),
      _ => ".".to_string(),
    },
  }
}

/// The save dialog's starting directory and suggested name in file mode:
/// from the field when it holds a path, else the default name.
fn save_prompt(value: &str, default_file_name: &str) -> (std::path::PathBuf, Option<String>) {
  if value.is_empty() {
    return (
      std::env::current_dir().unwrap_or_default(),
      Some(default_file_name.to_string()),
    );
  }
  let path = Path::new(value);
  let dir = path
    .parent()
    .filter(|parent| !parent.as_os_str().is_empty())
    .map(Path::to_path_buf)
    .unwrap_or_default();
  let name = path
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .or_else(|| Some(default_file_name.to_string()));
  (dir, name)
}

pub struct OutputPathDialog {
  input: Entity<InputState>,
  request: OutputDialogRequest,
  /// The directory does not exist yet (a warning, not an error).
  missing: bool,
  error: Option<SharedString>,
  check: Option<Task<()>>,
  confirming: Option<Task<()>>,
  /// Confirmed and closed: later Enter presses do nothing.
  done: bool,
  _subscription: Subscription,
}

impl OutputPathDialog {
  /// Opens the dialog for `request` with the field focused and selected.
  pub fn open(request: OutputDialogRequest, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Self::new(request, window, cx));
    let title = view.read(cx).request.title.clone();
    let focus = view.read(cx).input.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, window, _| {
      let (enter, ok, cancel) = (view.clone(), view.clone(), view.clone());
      dialog
        .title(title.clone())
        .w(window.viewport_size().width * 0.6)
        // The dialog's Enter: the same as OK, which closes the dialog itself
        // once the path is accepted.
        .on_ok(move |_, window, cx| {
          enter.update(cx, |this, cx| this.confirm(window, cx));
          false
        })
        .on_cancel(|_, _, _| true)
        .child(view.clone())
        .footer(
          DialogFooter::new()
            .justify_center()
            .child(
              Button::new("output-ok")
                .primary()
                .label(tr!("merge.ok"))
                .on_click(move |_, window, cx| ok.update(cx, |this, cx| this.confirm(window, cx))),
            )
            .child(
              Button::new("output-cancel")
                .ghost()
                .label(tr!("merge.cancel"))
                .on_click(move |_, window, cx| {
                  cancel.update(cx, |_, cx| cx.notify());
                  window.close_dialog(cx);
                }),
            ),
        )
    });
    window.focus(&focus, cx);
  }

  fn new(request: OutputDialogRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let input = cx.new(|cx| {
      let mut state = InputState::new(window, cx);
      state.set_value(request.initial.clone(), window, cx);
      state
    });
    let subscription = cx.subscribe_in(&input, window, Self::on_input_event);
    let mut dialog = Self {
      input,
      request,
      missing: false,
      error: None,
      check: None,
      confirming: None,
      done: false,
      _subscription: subscription,
    };
    dialog.schedule_check(cx);
    dialog
  }

  pub fn value(&self, cx: &App) -> String {
    self.input.read(cx).value().trim().to_string()
  }

  pub fn is_missing(&self) -> bool {
    self.missing
  }

  pub fn error(&self) -> Option<&SharedString> {
    self.error.as_ref()
  }

  fn on_input_event(
    &mut self,
    input: &Entity<InputState>,
    event: &InputEvent,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    match event {
      InputEvent::Focus => input.update(cx, |state, cx| state.select_all(window, cx)),
      InputEvent::Change => {
        self.error = None;
        self.schedule_check(cx);
        cx.notify();
      }
      InputEvent::PressEnter { .. } => self.confirm(window, cx),
      _ => {}
    }
  }

  /// Checks 300 ms after the last edit whether the directory exists.
  fn schedule_check(&mut self, cx: &mut Context<Self>) {
    let value = self.value(cx);
    if value.is_empty() {
      self.missing = false;
      self.check = None;
      return;
    }
    let dir = checked_dir(&value, self.request.mode);
    self.check = Some(cx.spawn(async move |this, cx| {
      cx.background_executor().timer(EXISTS_CHECK_DELAY).await;
      let exists = cx
        .background_spawn(async move { bmm_core::services::output_path_exists(dir) })
        .await;
      let _ = this.update(cx, |this, cx| {
        // An error is not a missing directory: no warning for it.
        this.missing = matches!(exists, Ok(false));
        cx.notify();
      });
    }));
  }

  fn set_value(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
    self.input.update(cx, |state, cx| state.set_value(value, window, cx));
    self.error = None;
    self.schedule_check(cx);
    cx.notify();
  }

  /// Browse: a folder picker, or in file mode a save dialog (which asks
  /// about an existing file itself).
  fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let value = self.value(cx);
    let chosen = match self.request.mode {
      OutputMode::Directory => {
        let paths = cx.prompt_for_paths(PathPromptOptions {
          files: false,
          directories: true,
          multiple: false,
          prompt: None,
        });
        cx.foreground_executor().spawn(async move {
          match paths.await {
            Ok(Ok(Some(paths))) => Ok(paths.into_iter().next()),
            Ok(Ok(None)) | Err(_) => Ok(None),
            Ok(Err(error)) => Err(error),
          }
        })
      }
      OutputMode::File => {
        let (dir, name) = save_prompt(&value, &self.request.default_file_name);
        let path = cx.prompt_for_new_path(&dir, name.as_deref());
        cx.foreground_executor().spawn(async move {
          match path.await {
            Ok(result) => result,
            Err(_) => Ok(None),
          }
        })
      }
    };
    cx.spawn_in(window, async move |this, cx| {
      let chosen = chosen.await;
      let _ = this.update_in(cx, |this, window, cx| match chosen {
        Ok(Some(path)) => this.set_value(path.to_string_lossy().into_owned(), window, cx),
        Ok(None) => {}
        Err(error) => {
          this.error = Some(error.to_string().into());
          cx.notify();
        }
      });
    })
    .detach();
  }

  /// OK / Enter: an empty field clears the override; any other path must be
  /// writable (or creatable) first.
  fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.done || self.confirming.is_some() {
      return;
    }
    let value = self.value(cx);
    if value.is_empty() {
      self.done = true;
      (self.request.on_confirm)(String::new(), cx);
      window.close_dialog(cx);
      return;
    }
    let checked = value.clone();
    self.confirming = Some(cx.spawn_in(window, async move |this, cx| {
      let writable = cx
        .background_spawn(async move { bmm_core::services::check_output_path_writable(checked) })
        .await;
      let _ = this.update_in(cx, |this, window, cx| {
        this.confirming = None;
        match writable {
          Ok(true) => {
            this.done = true;
            (this.request.on_confirm)(value, cx);
            window.close_dialog(cx);
          }
          Ok(false) => this.error = Some(tr!("merge.outputPathNotWritable")),
          Err(error) => this.error = Some(error.to_string().into()),
        }
        cx.notify();
      });
    }));
  }
}

impl Render for OutputPathDialog {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let danger = cx.theme().danger;
    v_flex()
      .id("output-dialog")
      .test_support()
      .gap(super::space(1.0))
      .child(
        h_flex()
          .gap(super::space(1.0))
          .child(
            div()
              .flex_1()
              .min_w_0()
              .child(Input::new(&self.input).id("output-path").small().cleanable(true)),
          )
          .child(
            Button::new("output-browse")
              .outline()
              .small()
              .label(tr!("merge.browse"))
              .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
          ),
      )
      .when(self.missing, |content| {
        let warning = tr!("merge.outputPathDoesNotExist");
        content.child(
          div()
            .text_size(super::text::body2())
            .text_color(danger)
            .child(warning.clone())
            .test_text(warning),
        )
      })
      .children(
        self
          .error
          .clone()
          .map(|error| Alert::error("output-error", error).into_any_element()),
      )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  #[test]
  fn checks_the_directory_of_a_file_path() {
    assert_eq!(checked_dir("/out/dir", OutputMode::Directory), "/out/dir");
    assert_eq!(checked_dir("/out/dir/Movie.mkv", OutputMode::File), "/out/dir");
    assert_eq!(checked_dir("Movie.mkv", OutputMode::File), ".");
  }

  #[test]
  fn the_save_dialog_starts_from_the_field() {
    assert_eq!(
      save_prompt("/out/dir/Movie.mkv", "Input.mkv"),
      ("/out/dir".into(), Some("Movie.mkv".to_string()))
    );
    let (_, name) = save_prompt("", "Input.mkv");
    assert_eq!(name.as_deref(), Some("Input.mkv"));
  }
}
