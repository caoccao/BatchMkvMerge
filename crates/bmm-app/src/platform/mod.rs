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

//! What Tauri's plugins and window events did: window geometry, the close
//! guard, the macOS menus, file dialogs and launch arguments.

pub mod close_guard;
pub mod dialogs;
pub mod menus;
pub mod window;

use gpui_kit::{AnyWindowHandle, App};

use crate::state::{Stores, files};
use crate::ui::actions::Quit;

/// Adds the media files named on the command line (`get_launch_args`).
pub fn ingest_launch_args(stores: &Stores, cx: &mut App) {
  files::add_paths(&stores.files, bmm_core::services::get_launch_args(), cx);
}

/// Quit (Cmd+Q on macOS) asks like closing the main window does: the window
/// geometry is saved and an active merge needs a confirmation first.
pub fn install_quit(handle: AnyWindowHandle, stores: &Stores, cx: &mut App) {
  let stores = stores.clone();
  cx.on_action(move |_: &Quit, cx| {
    let stores = stores.clone();
    let closed = handle.update(cx, |_, window, cx| window::close_requested(&stores, window, cx));
    // The window is gone already, or it agreed to close.
    if closed.unwrap_or(true) {
      cx.quit();
    }
  });
}
