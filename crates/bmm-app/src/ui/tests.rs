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

//! Headless smoke tests of the application shell.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bmm_core::config::{Config, Language};
use core::prelude::v1::test;
use gpui_kit::component::WindowExt;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::*;

use super::actions;
use super::root::AppView;
use crate::i18n::tests::LOCALE_LOCK;
use crate::platform::close_guard;
use crate::state::files::tests::TempDir;
use crate::state::ui::NotificationKind;
use crate::state::{Persistence, Stores, TabType};

/// The shell in a test window, in English. The locale is process-wide, and a
/// test switching it would change the text width under another test's click,
/// so every shell test holds the locale lock while it runs.
struct Shell {
  handle: AnyWindowHandle,
  stores: Stores,
  written: Arc<Mutex<Vec<Config>>>,
  _locale: MutexGuard<'static, ()>,
}

impl Shell {
  fn open(cx: &mut TestAppContext) -> Self {
    let locale = LOCALE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::i18n::set_language(Language::EnUS);
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    let (handle, stores) = cx.update(|cx| {
      gpui_kit::init(cx);
      crate::theme::install(cx);
      let config = Config {
        language: Language::EnUS,
        ..Config::default()
      };
      let stores = Stores::new(
        config,
        Persistence::new(move |config| sink.lock().unwrap().push(config)),
        cx,
      );
      actions::bind_keys(cx);
      actions::install(&stores, cx);
      let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
          point(px(40.0), px(30.0)),
          size(px(1000.0), px(760.0)),
        ))),
        ..Default::default()
      };
      let view_stores = stores.clone();
      let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| AppView::new(view_stores, window, cx))
      })
      .expect("open the main window");
      (handle, stores)
    });
    let shell = Self {
      handle,
      stores,
      written,
      _locale: locale,
    };
    shell.window(cx, |window, cx| window.render_frame(cx));
    shell
  }

  fn window<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
    cx.update_window(self.handle, |_, window, cx| f(window, cx))
      .expect("window open")
  }

  fn active_tab(&self, cx: &mut TestAppContext) -> TabType {
    self.stores.ui.read_with(cx, |ui, _| ui.active_tab())
  }

  fn press(&self, cx: &mut TestAppContext, key: &str) {
    self.window(cx, |window, cx| window.press(key, cx));
    cx.run_until_parked();
  }

  fn click(&self, cx: &mut TestAppContext, id: &'static str) {
    self.window(cx, |window, cx| window.click(id, cx));
    cx.run_until_parked();
  }

  /// Whether an element with this accessible name (a tab, a labelled button)
  /// is on screen.
  fn shows(&self, cx: &mut TestAppContext, label: &str) -> bool {
    self.window(cx, |window, cx| {
      window.render_frame(cx);
      window.try_find_by_label(label).is_some()
    })
  }

  fn add_files(&self, cx: &mut TestAppContext, files: &[&str]) {
    let files: Vec<String> = files.iter().map(|file| (*file).to_owned()).collect();
    self.stores.files.update(cx, |store, cx| store.add_files(files, cx));
    self.window(cx, |window, cx| window.render_frame(cx));
  }
}

#[gpui_kit::test]
fn the_shell_mounts_with_the_welcome_page_while_empty(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.window(cx, |window, _| {
    for id in [
      "app-root",
      "toolbar",
      "main-content",
      "welcome",
      "welcome-add-files",
      "welcome-add-folder",
      "footer",
    ] {
      window.find(id);
    }
    assert!(window.try_find("file-rows").is_none());
  });
  shell.add_files(cx, &["/media/a.mkv", "/media/b.mkv"]);
  shell.window(cx, |window, _| {
    assert!(window.try_find("welcome").is_none());
    window.find("file-rows");
    window.find(("file-row", 1usize));
  });
}

#[gpui_kit::test]
fn settings_and_about_open_from_the_toolbar_and_close_to_the_left(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.click(cx, "toolbar-settings");
  assert_eq!(shell.active_tab(cx), TabType::Settings);
  shell.click(cx, "toolbar-about");
  assert_eq!(shell.active_tab(cx), TabType::About);
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.find("about-author");
    window.find("about-github");
  });
  shell.click(cx, "tab-close-about");
  assert_eq!(shell.active_tab(cx), TabType::Settings);
  shell.click(cx, "tab-close-settings");
  assert_eq!(shell.active_tab(cx), TabType::Queue);
  assert_eq!(
    shell.stores.ui.read_with(cx, |ui, _| ui.tabs()),
    [TabType::FileList, TabType::Queue]
  );
}

#[gpui_kit::test]
fn tab_shortcuts_cycle_select_and_close(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.press(cx, "f10");
  assert_eq!(shell.active_tab(cx), TabType::Settings);
  shell.press(cx, "ctrl-tab");
  assert_eq!(shell.active_tab(cx), TabType::FileList);
  shell.press(cx, "ctrl-shift-tab");
  assert_eq!(shell.active_tab(cx), TabType::Settings);
  shell.press(cx, "ctrl-2");
  assert_eq!(shell.active_tab(cx), TabType::Queue);
  shell.press(cx, "ctrl-9");
  assert_eq!(shell.active_tab(cx), TabType::Queue);
  shell.press(cx, "ctrl-w");
  assert_eq!(shell.active_tab(cx), TabType::Queue, "Queue cannot be closed");
  shell.press(cx, "ctrl-3");
  assert_eq!(shell.active_tab(cx), TabType::Settings);
  shell.press(cx, "ctrl-w");
  assert_eq!(shell.active_tab(cx), TabType::Queue);
  assert_eq!(shell.stores.ui.read_with(cx, |ui, _| ui.tabs().len()), 2);
  shell.press(cx, "ctrl-1");
  assert_eq!(shell.active_tab(cx), TabType::FileList);
}

#[gpui_kit::test]
fn dropping_files_and_folders_lists_their_media_files(cx: &mut TestAppContext) {
  let dir = TempDir::new("shell-drop");
  let movie = dir.file("movie.mkv");
  dir.file("readme.txt");
  let shell = Shell::open(cx);
  let paths = ExternalPaths(vec![dir.path().to_path_buf()].into());
  let position = point(px(500.0), px(400.0));
  shell.window(cx, |window, cx| {
    window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }), cx);
    window.dispatch_event(PlatformInput::FileDrop(FileDropEvent::Submit { position }), cx);
  });
  cx.run_until_parked();
  shell
    .stores
    .files
    .read_with(cx, |files, _| assert_eq!(files.files(), [movie]));
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.find(("file-row", 0usize));
    assert!(window.try_find(("file-row", 1usize)).is_none());
  });
}

#[gpui_kit::test]
fn clear_empties_the_list_from_the_toolbar_and_ctrl_q(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.window(cx, |window, _| {
    assert_eq!(window.find("toolbar-clear").disabled(), Some(true))
  });
  shell.add_files(cx, &["/media/a.mkv"]);
  shell.window(cx, |window, _| {
    assert_ne!(window.find("toolbar-clear").disabled(), Some(true));
    // Nothing is selected or merging yet.
    assert_eq!(window.find("toolbar-merge-all").disabled(), Some(true));
    assert_eq!(window.find("toolbar-cancel-all").disabled(), Some(true));
  });
  shell.click(cx, "toolbar-clear");
  assert!(!shell.stores.files.read_with(cx, |files, _| files.has_files()));
  shell.add_files(cx, &["/media/b.mkv"]);
  shell.press(cx, "ctrl-q");
  assert!(!shell.stores.files.read_with(cx, |files, _| files.has_files()));
}

#[gpui_kit::test]
fn group_by_file_name_toggles_and_persists(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.click(cx, "toolbar-group-by-file-name");
  assert!(
    !shell
      .stores
      .config
      .read_with(cx, |config, _| config.config().group_by_file_name)
  );
  let written = shell.written.lock().unwrap().last().cloned().expect("persisted");
  assert!(!written.group_by_file_name);
}

#[gpui_kit::test]
fn switching_the_language_changes_the_ui_text(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  assert!(shell.shows(cx, "File List"));
  assert!(shell.shows(cx, "Add Files"));
  assert!(
    shell.shows(cx, "Clear (Ctrl + Q)"),
    "icon buttons are named by their tooltip"
  );
  shell.stores.config.update(cx, |config, cx| {
    config.update(cx, |config| config.language = Language::De)
  });
  cx.run_until_parked();
  assert!(shell.shows(cx, "Dateiliste"));
  assert!(shell.shows(cx, "Dateien hinzufügen"));
  assert!(!shell.shows(cx, "File List"));
  shell.stores.config.update(cx, |config, cx| {
    config.update(cx, |config| config.language = Language::EnUS)
  });
  cx.run_until_parked();
  assert!(shell.shows(cx, "File List"));
}

#[gpui_kit::test]
fn a_toast_appears_in_the_window(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  shell.stores.ui.update(cx, |ui, cx| {
    ui.show_toast(
      NotificationKind::Success,
      "/media/a.mkv".into(),
      "Completed in 00:00:01".into(),
      cx,
    )
  });
  cx.run_until_parked();
  let count = shell.window(cx, |window, cx| window.notifications(cx).len());
  assert_eq!(count, 1);
}

#[gpui_kit::test]
fn the_close_guard_asks_only_while_merges_are_active(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  let confirmed = std::rc::Rc::new(std::cell::Cell::new(false));
  let closes = shell.window(cx, |window, cx| close_guard::should_close(false, window, cx, |_, _| {}));
  assert!(closes);
  assert!(!shell.window(cx, |window, cx| window.has_active_dialog(cx)));

  let flag = confirmed.clone();
  let closes = shell.window(cx, |window, cx| {
    close_guard::should_close(true, window, cx, move |_, _| flag.set(true))
  });
  assert!(!closes);
  cx.run_until_parked();
  assert!(shell.window(cx, |window, cx| window.has_active_dialog(cx)));
  // A second request while the dialog is open does not stack another one.
  shell.window(cx, |window, cx| close_guard::should_close(true, window, cx, |_, _| {}));
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.click(cx, "cancel");
  assert!(!confirmed.get());
  assert!(!shell.window(cx, |window, cx| window.has_active_dialog(cx)));

  let flag = confirmed.clone();
  shell.window(cx, |window, cx| {
    close_guard::should_close(true, window, cx, move |_, _| flag.set(true))
  });
  cx.run_until_parked();
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.click(cx, "ok");
  assert!(confirmed.get());
  assert!(
    cx.update_window(shell.handle, |_, _, _| ()).is_err(),
    "the window closed"
  );
}

#[gpui_kit::test]
fn the_geometry_is_saved_once_the_window_is_still(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  cx.simulate_window_resize(shell.handle, size(px(900.0), px(700.0)));
  cx.run_until_parked();
  assert!(shell.written.lock().unwrap().is_empty(), "debounced");
  cx.executor().advance_clock(Duration::from_millis(600));
  cx.run_until_parked();
  let written = shell.written.lock().unwrap().last().cloned().expect("saved");
  assert_eq!((written.window.size.width, written.window.size.height), (900, 700));
  // Too small to keep: the last good geometry stays.
  cx.simulate_window_resize(shell.handle, size(px(500.0), px(400.0)));
  cx.executor().advance_clock(Duration::from_millis(600));
  cx.run_until_parked();
  let config = shell
    .stores
    .config
    .read_with(cx, |config, _| config.config().window.clone());
  assert_eq!((config.size.width, config.size.height), (900, 700));
}
