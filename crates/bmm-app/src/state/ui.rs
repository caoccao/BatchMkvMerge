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

//! Navigation state: the tabs, the in-app toast and the completion
//! notifications (`activeTab`, `showSettings`, `showAbout`,
//! `showNotification`, `systemNotificationEnabled`,
//! `topmostNotificationEnabled` of the React store).

use gpui_kit::{Context, EventEmitter};

/// The main tabs, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabType {
  FileList,
  Queue,
  Settings,
  About,
}

impl TabType {
  /// The i18n key of the tab label.
  pub fn label_key(self) -> &'static str {
    match self {
      Self::FileList => "tabs.fileList",
      Self::Queue => "tabs.queue",
      Self::Settings => "tabs.settings",
      Self::About => "tabs.about",
    }
  }

  /// Settings and About are opened from the toolbar and can be closed.
  pub fn is_closable(self) -> bool {
    matches!(self, Self::Settings | Self::About)
  }

  /// A stable name for element ids.
  pub fn name(self) -> &'static str {
    match self {
      Self::FileList => "file-list",
      Self::Queue => "queue",
      Self::Settings => "settings",
      Self::About => "about",
    }
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
  Success,
  Error,
}

/// A top-centre toast: the file as the title, the detail below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
  pub kind: NotificationKind,
  pub file: String,
  pub detail: String,
}

/// A card's short message (`Command copied`, a launch error): a
/// bottom-centre toast without a title, as the cards' snackbars were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snackbar {
  pub kind: NotificationKind,
  pub message: String,
}

/// What a drained queue reports through the OS notification and the
/// topmost window (`TopmostNotification` of the React protocol, without the
/// close label).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionNotice {
  pub title: String,
  /// The file of a one-file batch, else the counts.
  pub line: String,
  pub detail: String,
  /// A merge of the batch failed: the topmost window shows an alert.
  pub failed: bool,
}

/// A completion to deliver, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
  pub notice: CompletionNotice,
  /// Post it to the OS notification centre.
  pub system: bool,
  /// Show it in the topmost window.
  pub topmost: bool,
}

/// Events for the window that shows the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
  /// Show a toast in the main window.
  Toast(Toast),
  /// Show a card's message at the bottom of the main window.
  Snackbar(Snackbar),
  /// The queue drained: notify the OS and/or show the topmost window.
  Completion(Completion),
}

pub struct UiState {
  active_tab: TabType,
  show_settings: bool,
  show_about: bool,
  /// The queue's two notification toggles; off at every start, as in React.
  system_notification: bool,
  topmost_notification: bool,
  /// The main window is closing: no more completions.
  closing: bool,
}

impl EventEmitter<UiEvent> for UiState {}

impl Default for UiState {
  fn default() -> Self {
    Self::new()
  }
}

impl UiState {
  pub fn new() -> Self {
    Self {
      active_tab: TabType::FileList,
      show_settings: false,
      show_about: false,
      system_notification: false,
      topmost_notification: false,
      closing: false,
    }
  }

  pub fn active_tab(&self) -> TabType {
    self.active_tab
  }

  /// The open tabs: File List and Queue always, then Settings and About.
  pub fn tabs(&self) -> Vec<TabType> {
    let mut tabs = vec![TabType::FileList, TabType::Queue];
    if self.show_settings {
      tabs.push(TabType::Settings);
    }
    if self.show_about {
      tabs.push(TabType::About);
    }
    tabs
  }

  /// Index of the active tab among [`Self::tabs`].
  pub fn active_index(&self) -> usize {
    self.tabs().iter().position(|tab| *tab == self.active_tab).unwrap_or(0)
  }

  pub fn set_active_tab(&mut self, tab: TabType, cx: &mut Context<Self>) {
    if self.tabs().contains(&tab) && self.active_tab != tab {
      self.active_tab = tab;
      cx.notify();
    }
  }

  /// Opens (or reselects) the Settings tab.
  pub fn open_settings(&mut self, cx: &mut Context<Self>) {
    self.show_settings = true;
    self.active_tab = TabType::Settings;
    cx.notify();
  }

  /// Opens (or reselects) the About tab.
  pub fn open_about(&mut self, cx: &mut Context<Self>) {
    self.show_about = true;
    self.active_tab = TabType::About;
    cx.notify();
  }

  /// Closes a closable tab; when it was active, its left neighbour becomes
  /// active (`MainContent.tsx::closeTab`).
  pub fn close_tab(&mut self, tab: TabType, cx: &mut Context<Self>) {
    let tabs = self.tabs();
    let Some(index) = tabs.iter().position(|open| *open == tab) else {
      return;
    };
    if !tab.is_closable() {
      return;
    }
    if self.active_tab == tab {
      self.active_tab = tabs[index.saturating_sub(1)];
    }
    match tab {
      TabType::Settings => self.show_settings = false,
      TabType::About => self.show_about = false,
      TabType::FileList | TabType::Queue => {}
    }
    cx.notify();
  }

  /// Closes the active tab if it is closable (Ctrl+W).
  pub fn close_active_tab(&mut self, cx: &mut Context<Self>) {
    self.close_tab(self.active_tab, cx);
  }

  /// Moves the selection by `delta` tabs, wrapping around (Ctrl+Tab,
  /// Ctrl+Shift+Tab).
  pub fn select_relative(&mut self, delta: isize, cx: &mut Context<Self>) {
    let tabs = self.tabs();
    if tabs.len() <= 1 {
      return;
    }
    let count = tabs.len() as isize;
    let next = (self.active_index() as isize + delta).rem_euclid(count) as usize;
    self.set_active_tab(tabs[next], cx);
  }

  /// Selects the tab at `index` if it exists (Ctrl+1..9).
  pub fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
    if let Some(tab) = self.tabs().get(index).copied() {
      self.set_active_tab(tab, cx);
    }
  }

  /// Shows a toast in the main window (`showNotification`).
  pub fn show_toast(&mut self, kind: NotificationKind, file: String, detail: String, cx: &mut Context<Self>) {
    cx.emit(UiEvent::Toast(Toast { kind, file, detail }));
  }

  /// Shows a card's message at the bottom of the main window (the cards'
  /// `Snackbar`).
  pub fn show_snackbar(&mut self, kind: NotificationKind, message: String, cx: &mut Context<Self>) {
    cx.emit(UiEvent::Snackbar(Snackbar { kind, message }));
  }

  /// Whether a drained queue posts an OS notification.
  pub fn system_notification(&self) -> bool {
    self.system_notification
  }

  /// Whether a drained queue shows the topmost window.
  pub fn topmost_notification(&self) -> bool {
    self.topmost_notification
  }

  /// The queue's System Notification toggle. React asked the OS for
  /// permission first; the OS asks on the first post instead.
  pub fn toggle_system_notification(&mut self, cx: &mut Context<Self>) {
    self.system_notification = !self.system_notification;
    cx.notify();
  }

  /// The queue's Topmost Notification toggle.
  pub fn toggle_topmost_notification(&mut self, cx: &mut Context<Self>) {
    self.topmost_notification = !self.topmost_notification;
    cx.notify();
  }

  /// The main window is closing (its exit dialog said Yes): the merges it
  /// cancels drain the queue, which must not notify.
  pub fn set_closing(&mut self) {
    self.closing = true;
  }

  /// Delivers the notice of a drained queue through the enabled channels;
  /// nothing when both are off or the window is closing.
  pub fn show_completion(&mut self, notice: CompletionNotice, cx: &mut Context<Self>) {
    if self.closing || !(self.system_notification || self.topmost_notification) {
      return;
    }
    cx.emit(UiEvent::Completion(Completion {
      notice,
      system: self.system_notification,
      topmost: self.topmost_notification,
    }));
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::{AppContext, Entity, TestAppContext};

  fn state(cx: &mut TestAppContext) -> Entity<UiState> {
    cx.new(|_| UiState::new())
  }

  #[gpui_kit::test]
  fn settings_and_about_open_after_the_fixed_tabs(cx: &mut TestAppContext) {
    let ui = state(cx);
    ui.update(cx, |ui, cx| {
      assert_eq!(ui.tabs(), [TabType::FileList, TabType::Queue]);
      ui.open_about(cx);
      ui.open_settings(cx);
      assert_eq!(
        ui.tabs(),
        [TabType::FileList, TabType::Queue, TabType::Settings, TabType::About]
      );
      assert_eq!(ui.active_tab(), TabType::Settings);
      assert_eq!(ui.active_index(), 2);
    });
  }

  #[gpui_kit::test]
  fn closing_the_active_tab_selects_its_left_neighbour(cx: &mut TestAppContext) {
    let ui = state(cx);
    ui.update(cx, |ui, cx| {
      ui.open_settings(cx);
      ui.open_about(cx);
      ui.close_tab(TabType::About, cx);
      assert_eq!(ui.active_tab(), TabType::Settings);
      ui.close_tab(TabType::Settings, cx);
      assert_eq!(ui.active_tab(), TabType::Queue);
    });
  }

  #[gpui_kit::test]
  fn closing_an_inactive_tab_keeps_the_selection(cx: &mut TestAppContext) {
    let ui = state(cx);
    ui.update(cx, |ui, cx| {
      ui.open_settings(cx);
      ui.open_about(cx);
      ui.close_tab(TabType::Settings, cx);
      assert_eq!(ui.active_tab(), TabType::About);
      assert_eq!(ui.tabs(), [TabType::FileList, TabType::Queue, TabType::About]);
    });
  }

  #[gpui_kit::test]
  fn fixed_tabs_do_not_close(cx: &mut TestAppContext) {
    let ui = state(cx);
    ui.update(cx, |ui, cx| {
      ui.set_active_tab(TabType::Queue, cx);
      ui.close_active_tab(cx);
      ui.close_tab(TabType::FileList, cx);
      assert_eq!(ui.tabs().len(), 2);
      assert_eq!(ui.active_tab(), TabType::Queue);
      // A tab that is not open cannot be selected or closed.
      ui.set_active_tab(TabType::About, cx);
      ui.close_tab(TabType::About, cx);
      assert_eq!(ui.active_tab(), TabType::Queue);
    });
  }

  #[gpui_kit::test]
  fn relative_and_indexed_selection_wrap_and_ignore_missing_tabs(cx: &mut TestAppContext) {
    let ui = state(cx);
    ui.update(cx, |ui, cx| {
      ui.select_relative(-1, cx);
      assert_eq!(ui.active_tab(), TabType::Queue);
      ui.select_relative(1, cx);
      assert_eq!(ui.active_tab(), TabType::FileList);
      ui.open_about(cx);
      ui.select_relative(1, cx);
      assert_eq!(ui.active_tab(), TabType::FileList);
      ui.select_index(1, cx);
      assert_eq!(ui.active_tab(), TabType::Queue);
      ui.select_index(5, cx);
      assert_eq!(ui.active_tab(), TabType::Queue);
    });
  }

  #[gpui_kit::test]
  fn completions_go_only_through_enabled_channels(cx: &mut TestAppContext) {
    let ui = state(cx);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = seen.clone();
    let _subscription =
      cx.update(|cx| cx.subscribe(&ui, move |_, event: &UiEvent, _| sink.borrow_mut().push(event.clone())));
    let notice = CompletionNotice {
      title: "Merge complete.".into(),
      line: "a.mkv".into(),
      detail: "Completed in 00:00:01".into(),
      failed: false,
    };
    ui.update(cx, |ui, cx| {
      assert!(!ui.system_notification() && !ui.topmost_notification(), "off at start");
      ui.show_completion(notice.clone(), cx);
      ui.toggle_topmost_notification(cx);
      ui.show_completion(notice.clone(), cx);
      ui.toggle_system_notification(cx);
      ui.toggle_topmost_notification(cx);
      ui.show_completion(notice.clone(), cx);
      ui.set_closing();
      ui.show_completion(notice.clone(), cx);
    });
    let completion = |system, topmost| {
      UiEvent::Completion(Completion {
        notice: notice.clone(),
        system,
        topmost,
      })
    };
    assert_eq!(
      seen.borrow().as_slice(),
      [completion(false, true), completion(true, false)]
    );
  }

  #[gpui_kit::test]
  fn show_toast_emits_an_event(cx: &mut TestAppContext) {
    let ui = state(cx);
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = seen.clone();
    let _subscription =
      cx.update(|cx| cx.subscribe(&ui, move |_, event: &UiEvent, _| sink.borrow_mut().push(event.clone())));
    ui.update(cx, |ui, cx| {
      ui.show_toast(NotificationKind::Error, "a.mkv".into(), "boom".into(), cx)
    });
    assert_eq!(
      seen.borrow().as_slice(),
      [UiEvent::Toast(Toast {
        kind: NotificationKind::Error,
        file: "a.mkv".into(),
        detail: "boom".into(),
      })]
    );
  }
}
