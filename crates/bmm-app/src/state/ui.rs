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

//! Navigation state: the tabs, and the in-app toast (`activeTab`,
//! `showSettings`, `showAbout`, `showNotification` of the React store).

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

/// Events for the window that shows the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
  /// Show a toast in the main window.
  Toast(Toast),
}

pub struct UiState {
  active_tab: TabType,
  show_settings: bool,
  show_about: bool,
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
