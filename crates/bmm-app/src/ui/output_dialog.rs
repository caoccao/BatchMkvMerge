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
use gpui_kit::component::dialog::{DialogEntrance, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::assets::{OUTPUT_DIRECTORY_ICON_NAME, toolbar_icon};

use super::word_navigation::PATH_CONTEXT;
use super::{confirm_button, hex, space, text, text_button};

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

/// What the OK check found for a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathCheck {
  Writable,
  NotWritable,
  /// File mode: a file is there, and a merge replaces no file.
  Exists,
}

/// Whether a merge can write to `value` in `mode`: in directory mode a
/// directory (an existing file is not one), in file mode a file that is not
/// there yet, or a directory (the merge then writes the input's name in it).
fn writable(value: String, mode: OutputMode) -> anyhow::Result<PathCheck> {
  let check = |writable: bool| {
    if writable {
      PathCheck::Writable
    } else {
      PathCheck::NotWritable
    }
  };
  match mode {
    OutputMode::Directory => bmm_core::services::check_output_dir_writable(value).map(check),
    OutputMode::File if Path::new(&value).is_file() => Ok(PathCheck::Exists),
    OutputMode::File => bmm_core::services::check_output_file_writable(value).map(check),
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
    let mode = request.mode;
    let view = cx.new(|cx| Self::new(request, window, cx));
    let title = view.read(cx).request.title.clone();
    let focus = view.read(cx).input.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, window, cx| {
      let (enter, ok) = (view.clone(), view.clone());
      let theme = cx.theme();
      let icon = match mode {
        OutputMode::Directory => toolbar_icon(OUTPUT_DIRECTORY_ICON_NAME).with_size(space(4.0)),
        OutputMode::File => Icon::new(IconName::FileText),
      };
      dialog
        .w((window.viewport_size().width * 0.6).max(px(480.0)))
        .centered()
        .entrance(DialogEntrance::None)
        .px(space(2.5))
        .title(
          h_flex()
            .gap(space(1.5))
            .pr(space(3.0))
            .pb(space(1.0))
            .child(
              h_flex()
                .size(space(4.0))
                .flex_shrink_0()
                .justify_center()
                .rounded(theme.radius)
                .bg(theme.primary.opacity(0.12))
                .child(icon.text_color(theme.primary)),
            )
            .child(div().text_size(text::h6()).child(title.clone())),
        )
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
            .border_t_1()
            .border_color(theme.border)
            .pt(space(1.5))
            .child(
              div()
                .grid()
                .grid_cols(2)
                .gap(space(1.0))
                .child(
                  text_button("output-cancel")
                    .outline()
                    .label(tr!("merge.cancel"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                  confirm_button("output-ok")
                    .label(tr!("merge.ok"))
                    .on_click(move |_, window, cx| ok.update(cx, |this, cx| this.confirm(window, cx))),
                ),
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
      InputEvent::Focus => {
        input.update(cx, |state, cx| state.select_all(window, cx));
        cx.notify();
      }
      InputEvent::Blur => cx.notify(),
      InputEvent::Change => {
        self.error = None;
        self.schedule_check(cx);
        cx.notify();
      }
      InputEvent::PressEnter { .. } => self.confirm(window, cx),
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

  /// Browse: a folder picker, or in file mode a save dialog. The save dialog
  /// asks before it returns an existing file, but OK still refuses one: a
  /// merge replaces no file.
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
    let (checked, mode) = (value.clone(), self.request.mode);
    self.confirming = Some(cx.spawn_in(window, async move |this, cx| {
      let writable = cx.background_spawn(async move { writable(checked, mode) }).await;
      let _ = this.update_in(cx, |this, window, cx| {
        this.confirming = None;
        match writable {
          Ok(PathCheck::Writable) => {
            this.done = true;
            (this.request.on_confirm)(value, cx);
            window.close_dialog(cx);
          }
          Ok(PathCheck::NotWritable) => this.error = Some(tr!("merge.outputPathNotWritable")),
          Ok(PathCheck::Exists) => this.error = Some(crate::actions::output_exists_message(&value).into()),
          Err(error) => this.error = Some(error.to_string().into()),
        }
        cx.notify();
      });
    }));
  }
}

impl Render for OutputPathDialog {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let (message, color) = match (&self.error, self.missing) {
      (Some(error), _) => (error.clone(), theme.danger),
      (None, true) => (tr!("merge.outputPathDoesNotExist"), theme.danger),
      (None, false) => {
        let gray = if theme.is_dark() { hex(0xa6a6a6) } else { hex(0x6e6e6e) };
        (tr!("merge.outputPathHint"), gray)
      }
    };
    let ring = theme.ring;
    let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
    v_flex()
      .id("output-dialog")
      .test_support()
      .gap(space(1.0))
      .child(
        h_flex()
          .gap(space(1.0))
          .child(
            div().flex_1().min_w_0().key_context(PATH_CONTEXT).child(
              Input::new(&self.input)
                .id("output-path")
                .aria_label(tr!("merge.outputPath"))
                .cleanable(true)
                .focus_bordered(false)
                .when(focused, |input| input.border_color(ring)),
            ),
          )
          .child(
            text_button("output-browse")
              .outline()
              .flex_shrink_0()
              .label(tr!("merge.browse"))
              .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
          ),
      )
      .child(
        div()
          .id("output-status")
          .w_full()
          .h(space(2.0))
          .flex_shrink_0()
          .line_height(space(2.0))
          .text_size(text::body2())
          .text_color(color)
          .truncate()
          .when(self.error.is_some(), |row| row.role(Role::Alert))
          .child(message.clone())
          .when(self.error.is_some() || self.missing, |row| {
            let message = message.clone();
            row.tooltip(move |window, cx| Tooltip::new(message.clone()).max_w(px(400.0)).build(window, cx))
          })
          .test_text(message),
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
