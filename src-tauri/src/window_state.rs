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

use std::sync::atomic::AtomicBool;

pub static WINDOW_READY: AtomicBool = AtomicBool::new(false);

pub const MIN_WINDOW_WIDTH: u32 = 600;
pub const MIN_WINDOW_HEIGHT: u32 = 450;

pub fn is_persistable_window_size(width: u32, height: u32) -> bool {
  width >= MIN_WINDOW_WIDTH && height >= MIN_WINDOW_HEIGHT
}

pub fn sanitize_window_size(width: u32, height: u32) -> (u32, u32) {
  (width.max(MIN_WINDOW_WIDTH), height.max(MIN_WINDOW_HEIGHT))
}

#[cfg(target_os = "linux")]
pub fn configure_linux_webkit_renderer() {
  if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
    // SAFETY: This runs during process startup before Tauri or Tokio spawn threads.
    unsafe {
      std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn persistable_window_size_respects_configured_minimums() {
    assert!(is_persistable_window_size(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
    assert!(!is_persistable_window_size(MIN_WINDOW_WIDTH - 1, MIN_WINDOW_HEIGHT));
    assert!(!is_persistable_window_size(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT - 1));
  }

  #[test]
  fn sanitize_window_size_clamps_poisoned_config_values() {
    assert_eq!(sanitize_window_size(1, 2), (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));
    assert_eq!(
      sanitize_window_size(MIN_WINDOW_WIDTH + 100, MIN_WINDOW_HEIGHT + 100),
      (MIN_WINDOW_WIDTH + 100, MIN_WINDOW_HEIGHT + 100)
    );
  }
}
