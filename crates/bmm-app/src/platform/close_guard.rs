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

//! The exit confirmation (`CloseGuard.tsx`): closing the main window while a
//! merge is waiting or running asks first.

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::{App, Window};

/// Decides a close request. Without active merges the window closes. With
/// them, a dialog asks (No, the default, keeps the window; Yes runs
/// `on_confirm`, which cancels the merges, then closes the window) and the
/// request is refused for now.
pub fn should_close(
  has_active_merges: bool,
  window: &mut Window,
  cx: &mut App,
  on_confirm: impl Fn(&mut Window, &mut App) + 'static,
) -> bool {
  if !has_active_merges {
    return true;
  }
  if !window.has_active_dialog(cx) {
    let on_confirm = std::rc::Rc::new(on_confirm);
    window.open_alert_dialog(cx, move |dialog, _, _| {
      let on_confirm = on_confirm.clone();
      dialog
        .title(tr!("confirm.exitTitle"))
        .description(tr!("confirm.exitMessage"))
        .show_cancel(true)
        .cancel_text(tr!("confirm.no"))
        .cancel_variant(ButtonVariant::Success)
        .ok_text(tr!("confirm.yes"))
        .ok_variant(ButtonVariant::Danger)
        .on_ok(move |_, window, cx| {
          on_confirm(window, cx);
          window.remove_window();
          true
        })
    });
  }
  false
}
