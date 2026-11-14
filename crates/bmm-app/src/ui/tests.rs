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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use bmm_core::config::{Config, Language};
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::merge::{MergeEvent, MergeEventSink, MergeOutcome};
use core::prelude::v1::test;
use gpui_kit::component::WindowExt;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::*;

use super::actions;
use super::root::AppView;
use crate::domain::status::QueueItemStatus;
use crate::i18n::tests::LOCALE_LOCK;
use crate::merge_events::{self, MergeBackend};
use crate::platform::close_guard;
use crate::state::files::tests::{TempDir, fixture_source, movie};
use crate::state::ui::NotificationKind;
use crate::state::{Persistence, Services, Stores, TabType};

/// What the shell's merge queue was asked to do.
#[derive(Default)]
pub(super) struct MergeCalls {
  pub(super) enqueued: Vec<(String, Vec<String>)>,
  pub(super) cancelled: Vec<String>,
}

/// The shell in a test window, in English. The locale is process-wide, and a
/// test switching it would change the text width under another test's click,
/// so every shell test holds the locale lock while it runs.
///
/// Files parse from fixtures; merges are recorded, and their events are sent
/// by the test through [`Self::send`] into the same channel the app uses.
pub(super) struct Shell {
  pub(super) handle: AnyWindowHandle,
  pub(super) view: Entity<AppView>,
  pub(super) stores: Stores,
  written: Arc<Mutex<Vec<Config>>>,
  pub(super) merges: Arc<Mutex<MergeCalls>>,
  events: MergeEventSink,
  _locale: MutexGuard<'static, ()>,
}

impl Shell {
  fn open(cx: &mut TestAppContext) -> Self {
    Self::with_files(cx, HashMap::new())
  }

  pub(super) fn with_files(cx: &mut TestAppContext, fixtures: HashMap<String, MediaMetadata>) -> Self {
    Self::with_config(
      cx,
      Config {
        language: Language::EnUS,
        ..Config::default()
      },
      fixtures,
    )
  }

  /// The shell with `config` (its language is set to English).
  pub(super) fn with_config(cx: &mut TestAppContext, config: Config, fixtures: HashMap<String, MediaMetadata>) -> Self {
    let locale = LOCALE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::i18n::set_language(Language::EnUS);
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    let merges = Arc::new(Mutex::new(MergeCalls::default()));
    let (enqueue_calls, cancel_calls) = (merges.clone(), merges.clone());
    let backend = MergeBackend::new(
      move |file, args| {
        enqueue_calls.lock().unwrap().enqueued.push((file, args));
        Ok(())
      },
      move |file| {
        cancel_calls.lock().unwrap().cancelled.push(file.to_string());
        Ok(())
      },
    );
    let (events, receiver) = merge_events::channel();
    let (metadata, _) = fixture_source(fixtures);
    let (handle, view, stores) = cx.update(|cx| {
      gpui_kit::init(cx);
      crate::theme::install(cx);
      let config = Config {
        language: Language::EnUS,
        ..config
      };
      let services = Services {
        persistence: Persistence::new(move |config| sink.lock().unwrap().push(config)),
        metadata,
        merges: backend,
      };
      let stores = Stores::new(config, services, cx);
      merge_events::start(&stores, receiver, cx);
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
      let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| AppView::new(view_stores, window, cx))
      })
      .expect("open the main window");
      (handle, view, stores)
    });
    let shell = Self {
      handle,
      view,
      stores,
      written,
      merges,
      events,
      _locale: locale,
    };
    shell.window(cx, |window, cx| window.render_frame(cx));
    shell
  }

  /// Sends a merge event as a worker thread of the core would.
  pub(super) fn send(&self, cx: &mut TestAppContext, event: MergeEvent) {
    (self.events)(event);
    cx.run_until_parked();
  }

  /// The title the window shows.
  fn title(&self, cx: &mut TestAppContext) -> String {
    VisualTestContext::from_window(self.handle, cx)
      .window_title()
      .unwrap_or_else(|| crate::window_title().to_string())
  }

  fn toasts(&self, cx: &mut TestAppContext) -> usize {
    self.window(cx, |window, cx| window.notifications(cx).len())
  }

  pub(super) fn window<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
    cx.update_window(self.handle, |_, window, cx| f(window, cx))
      .expect("window open")
  }

  fn active_tab(&self, cx: &mut TestAppContext) -> TabType {
    self.stores.ui.read_with(cx, |ui, _| ui.active_tab())
  }

  pub(super) fn press(&self, cx: &mut TestAppContext, key: &str) {
    self.window(cx, |window, cx| window.press(key, cx));
    cx.run_until_parked();
  }

  pub(super) fn click(&self, cx: &mut TestAppContext, id: &'static str) {
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

  pub(super) fn add_files(&self, cx: &mut TestAppContext, files: &[&str]) {
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
    assert!(window.try_find("file-list").is_none());
  });
  shell.add_files(cx, &["/media/a.mkv", "/media/b.mkv"]);
  shell.window(cx, |window, _| {
    assert!(window.try_find("welcome").is_none());
    window.find("file-list");
    window.find(("file-entry", 1usize));
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
    window.find(("file-entry", 0usize));
    assert!(window.try_find(("file-entry", 1usize)).is_none());
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
  shell.click(cx, "exit-no");
  assert!(!confirmed.get());
  assert!(!shell.window(cx, |window, cx| window.has_active_dialog(cx)));

  let flag = confirmed.clone();
  shell.window(cx, |window, cx| {
    close_guard::should_close(true, window, cx, move |_, _| flag.set(true))
  });
  cx.run_until_parked();
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.click(cx, "exit-yes");
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

fn movies(files: &[&str]) -> HashMap<String, MediaMetadata> {
  files.iter().map(|file| ((*file).to_string(), movie())).collect()
}

pub(super) fn started(file: &str) -> MergeEvent {
  MergeEvent::Started {
    file: file.to_string(),
    at: SystemTime::now(),
  }
}

fn finished(file: &str, outcome: MergeOutcome, error: Option<&str>) -> MergeEvent {
  MergeEvent::Finished {
    file: file.to_string(),
    outcome,
    error: error.map(str::to_string),
    at: SystemTime::now(),
  }
}

impl Shell {
  pub(super) fn enqueued(&self) -> Vec<String> {
    self
      .merges
      .lock()
      .unwrap()
      .enqueued
      .iter()
      .map(|(file, _)| file.clone())
      .collect()
  }

  fn cancelled(&self) -> Vec<String> {
    self.merges.lock().unwrap().cancelled.clone()
  }

  pub(super) fn status(&self, cx: &mut TestAppContext, file: &str) -> Option<QueueItemStatus> {
    self
      .stores
      .queue
      .read_with(cx, |queue, _| queue.item(file).map(|item| item.status))
  }

  fn disabled(&self, cx: &mut TestAppContext, id: &'static str) -> bool {
    self.window(cx, |window, cx| {
      window.render_frame(cx);
      window.find(id).disabled() == Some(true)
    })
  }

  /// Adds parsed files and lets the profile select their tracks.
  pub(super) fn add_parsed(&self, cx: &mut TestAppContext, files: &[&str]) {
    self.add_files(cx, files);
    cx.run_until_parked();
    self.window(cx, |window, cx| window.render_frame(cx));
  }
}

#[gpui_kit::test]
fn merge_all_enqueues_every_card_and_the_title_counts_the_queue(cx: &mut TestAppContext) {
  let (a, b) = ("/media/a.mkv", "/media/b.mkv");
  let shell = Shell::with_files(cx, movies(&[a, b]));
  let base = crate::window_title().to_string();
  assert!(shell.disabled(cx, "toolbar-merge-all"));
  shell.add_parsed(cx, &[a, b]);
  assert!(!shell.disabled(cx, "toolbar-merge-all"), "the profile selected tracks");
  shell.click(cx, "toolbar-merge-all");
  assert_eq!(shell.enqueued(), [a, b], "in file list order");
  let args = shell.merges.lock().unwrap().enqueued[0].1.clone();
  assert_eq!(args[0], "-o");
  assert!(args.contains(&"--track-order".to_string()));
  assert_eq!(shell.status(cx, a), Some(QueueItemStatus::Waiting));
  assert_eq!(shell.title(cx), format!("{base} - 0/2"));
  assert!(!shell.disabled(cx, "toolbar-cancel-all"));
  assert!(shell.disabled(cx, "toolbar-clear"), "no clearing while merges wait");

  shell.send(cx, started(a));
  shell.send(
    cx,
    MergeEvent::Progress {
      file: a.to_string(),
      percent: 50,
    },
  );
  let progress = shell
    .stores
    .queue
    .read_with(cx, |queue, _| queue.item(a).map(|item| item.progress));
  assert_eq!(progress, Some(50));
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.send(cx, finished(a, MergeOutcome::Completed, None));
  assert_eq!(shell.title(cx), format!("{base} - 1/2"));
  assert_eq!(shell.toasts(cx), 1, "a success toast");

  shell.send(cx, started(b));
  shell.send(
    cx,
    finished(b, MergeOutcome::Failed, Some("mkvmerge exited with code Some(2)")),
  );
  assert_eq!(shell.status(cx, b), Some(QueueItemStatus::Failed));
  assert_eq!(shell.title(cx), base, "the plain title once nothing is active");
  assert_eq!(shell.toasts(cx), 2, "an error toast");
  assert!(!shell.disabled(cx, "toolbar-clear"));
  assert!(shell.disabled(cx, "toolbar-cancel-all"));
}

#[gpui_kit::test]
fn f3_merges_once_and_f4_cancels(cx: &mut TestAppContext) {
  let a = "/media/a.mkv";
  let shell = Shell::with_files(cx, movies(&[a]));
  shell.press(cx, "f3");
  assert!(shell.enqueued().is_empty(), "nothing selected yet");
  shell.add_parsed(cx, &[a]);
  shell.press(cx, "f3");
  shell.press(cx, "f3");
  assert_eq!(shell.enqueued(), [a], "a waiting card is not enqueued again");
  shell.press(cx, "ctrl-q");
  assert!(
    shell.stores.files.read_with(cx, |files, _| files.has_files()),
    "Ctrl+Q waits too"
  );
  shell.press(cx, "f4");
  assert_eq!(shell.cancelled(), [a]);
  let requested = shell
    .stores
    .queue
    .read_with(cx, |queue, _| queue.item(a).map(|item| item.cancel_requested));
  assert_eq!(requested, Some(true));
  shell.send(cx, finished(a, MergeOutcome::Cancelled, None));
  assert_eq!(shell.status(cx, a), Some(QueueItemStatus::Cancelled));
  assert_eq!(shell.toasts(cx), 0, "a cancelled merge shows no toast");
  shell.press(cx, "f4");
  assert_eq!(shell.cancelled(), [a], "nothing active to cancel");
  shell.press(cx, "ctrl-q");
  assert!(!shell.stores.files.read_with(cx, |files, _| files.has_files()));
  assert_eq!(shell.status(cx, a), None, "clearing empties the queue too");
}

#[gpui_kit::test]
fn the_rows_follow_loading_and_merging(cx: &mut TestAppContext) {
  let (a, bad) = ("/media/a.mkv", "/media/bad.mkv");
  let shell = Shell::with_files(cx, movies(&[a]));
  shell.add_files(cx, &[a, bad]);
  shell.window(cx, |window, _| {
    window.find(("file-entry", 0usize));
    window.find(("file-entry", 1usize));
  });
  cx.run_until_parked();
  shell.press(cx, "f3");
  shell.send(cx, started(a));
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.find(("file-entry", 1usize));
  });
  shell.stores.files.read_with(cx, |files, _| {
    assert!(matches!(files.load_state(a), crate::state::files::LoadState::Loaded));
    assert!(matches!(
      files.load_state(bad),
      crate::state::files::LoadState::Failed(_)
    ));
  });
}

#[gpui_kit::test]
fn closing_while_a_merge_runs_asks_and_yes_cancels_it(cx: &mut TestAppContext) {
  let a = "/media/a.mkv";
  let shell = Shell::with_files(cx, movies(&[a]));
  shell.add_parsed(cx, &[a]);
  shell.press(cx, "f3");
  shell.send(cx, started(a));
  let stores = shell.stores.clone();
  let closes = shell.window(cx, |window, cx| {
    crate::platform::window::close_requested(&stores, window, cx)
  });
  assert!(!closes);
  cx.run_until_parked();
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.click(cx, "exit-yes");
  assert_eq!(shell.cancelled(), [a]);
  assert!(
    cx.update_window(shell.handle, |_, _, _| ()).is_err(),
    "the window closed"
  );
}

/// Quit by keyboard (macOS Cmd+Q) while a merge runs asks first, like the
/// title bar: the key is dispatched while its window is being updated.
#[gpui_kit::test]
fn quitting_by_keyboard_while_a_merge_runs_asks_first(cx: &mut TestAppContext) {
  let a = "/media/a.mkv";
  let shell = Shell::with_files(cx, movies(&[a]));
  cx.update(|cx| {
    cx.bind_keys([KeyBinding::new("cmd-q", actions::Quit, None)]);
    crate::platform::install_quit(shell.handle, &shell.stores, cx);
  });
  shell.add_parsed(cx, &[a]);
  shell.press(cx, "f3");
  shell.send(cx, started(a));
  shell.press(cx, "cmd-q");
  assert!(
    shell.window(cx, |window, cx| window.has_active_dialog(cx)),
    "the exit confirmation is open"
  );
  shell.window(cx, |window, cx| window.render_frame(cx));
  shell.click(cx, "exit-no");
  assert!(shell.cancelled().is_empty());
  assert!(
    cx.update_window(shell.handle, |_, _, _| ()).is_ok(),
    "No keeps the window"
  );
}

#[gpui_kit::test]
fn enter_and_escape_answer_no_in_the_exit_dialog(cx: &mut TestAppContext) {
  let shell = Shell::open(cx);
  let confirmed = std::rc::Rc::new(std::cell::Cell::new(false));
  for key in ["enter", "escape"] {
    let flag = confirmed.clone();
    shell.window(cx, |window, cx| {
      close_guard::should_close(true, window, cx, move |_, _| flag.set(true))
    });
    cx.run_until_parked();
    shell.window(cx, |window, cx| window.render_frame(cx));
    shell.press(cx, key);
    assert!(!confirmed.get(), "{key} does not confirm");
    assert!(
      !shell.window(cx, |window, cx| window.has_active_dialog(cx)),
      "{key} closes the dialog"
    );
  }
}
