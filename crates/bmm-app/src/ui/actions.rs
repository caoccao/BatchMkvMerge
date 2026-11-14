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

//! Application actions and their shortcuts. The tab shortcuts are Ctrl on
//! every platform, as the React app read `event.ctrlKey` (Cmd+W and Cmd+1
//! are not used on macOS either). The handlers are global, so the keys work
//! whatever has the focus.

use gpui_kit::{App, KeyBinding, actions};

use crate::state::Stores;

actions!(
  batch_mkvmerge,
  [
    /// Quit the application (macOS Cmd+Q; asks while merges run).
    Quit,
    /// Ctrl+Tab.
    NextTab,
    /// Ctrl+Shift+Tab.
    PreviousTab,
    /// Ctrl+W: close the active tab if it is closable.
    CloseTab,
    /// F10.
    OpenSettings,
    /// Ctrl+Q: remove every file unless a merge is active.
    ClearFiles,
    SelectTab1,
    SelectTab2,
    SelectTab3,
    SelectTab4,
    SelectTab5,
    SelectTab6,
    SelectTab7,
    SelectTab8,
    SelectTab9,
  ]
);

/// Binds the application shortcuts; Merge All (F3), Cancel All (F4) and the
/// group (F8) and profile (F9) menus join with their actions.
pub fn bind_keys(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("ctrl-tab", NextTab, None),
    KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    KeyBinding::new("ctrl-w", CloseTab, None),
    KeyBinding::new("f10", OpenSettings, None),
    KeyBinding::new("ctrl-q", ClearFiles, None),
    KeyBinding::new("ctrl-1", SelectTab1, None),
    KeyBinding::new("ctrl-2", SelectTab2, None),
    KeyBinding::new("ctrl-3", SelectTab3, None),
    KeyBinding::new("ctrl-4", SelectTab4, None),
    KeyBinding::new("ctrl-5", SelectTab5, None),
    KeyBinding::new("ctrl-6", SelectTab6, None),
    KeyBinding::new("ctrl-7", SelectTab7, None),
    KeyBinding::new("ctrl-8", SelectTab8, None),
    KeyBinding::new("ctrl-9", SelectTab9, None),
  ]);
}

/// Whether Clear (toolbar and Ctrl+Q) is available: files listed and no merge
/// waiting or running.
pub fn can_clear(stores: &Stores, cx: &App) -> bool {
  stores.files.read(cx).has_files() && !stores.has_active_merges(cx)
}

/// Clears the file list if [`can_clear`].
pub fn clear_files(stores: &Stores, cx: &mut App) {
  if can_clear(stores, cx) {
    stores.files.update(cx, |files, cx| files.clear_files(cx));
  }
}

/// Registers the global handlers of the actions above (except Quit, which
/// needs the main window).
pub fn install(stores: &Stores, cx: &mut App) {
  let ui = stores.ui.clone();
  cx.on_action({
    let ui = ui.clone();
    move |_: &NextTab, cx| ui.update(cx, |ui, cx| ui.select_relative(1, cx))
  });
  cx.on_action({
    let ui = ui.clone();
    move |_: &PreviousTab, cx| ui.update(cx, |ui, cx| ui.select_relative(-1, cx))
  });
  cx.on_action({
    let ui = ui.clone();
    move |_: &CloseTab, cx| ui.update(cx, |ui, cx| ui.close_active_tab(cx))
  });
  cx.on_action({
    let ui = ui.clone();
    move |_: &OpenSettings, cx| ui.update(cx, |ui, cx| ui.open_settings(cx))
  });
  cx.on_action({
    let stores = stores.clone();
    move |_: &ClearFiles, cx| clear_files(&stores, cx)
  });
  macro_rules! select_tab {
    ($($action:ident => $index:expr),*) => {
      $(
        cx.on_action({
          let ui = ui.clone();
          move |_: &$action, cx| ui.update(cx, |ui, cx| ui.select_index($index, cx))
        });
      )*
    };
  }
  select_tab!(
    SelectTab1 => 0, SelectTab2 => 1, SelectTab3 => 2, SelectTab4 => 3, SelectTab5 => 4,
    SelectTab6 => 5, SelectTab7 => 6, SelectTab8 => 7, SelectTab9 => 8
  );
}
