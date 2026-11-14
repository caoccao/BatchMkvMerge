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

//! BatchMkvMerge desktop application on gpui-kit.
//!
//! Everything lives in this library so that tests run against it; the binary
//! only calls [`run`].

/// `t!` as a [`gpui_kit::SharedString`], ready to become an element.
macro_rules! tr {
  ($($all:tt)*) => {
    gpui_kit::SharedString::from(rust_i18n::t!($($all)*).into_owned())
  };
}

pub mod actions;
pub mod assets;
pub mod domain;
pub mod i18n;
pub mod merge_events;
pub mod notifications;
pub mod platform;
pub mod state;
pub mod theme;
pub mod tools;
pub mod ui;
pub mod updates;

use gpui_kit::*;

// The nine locale files under `locales/`; a key missing in a locale falls
// back to en-US, as i18next did.
rust_i18n::i18n!("locales", fallback = "en-US");

/// Product name: window title, menus, notifications.
pub const APP_NAME: &str = "BatchMkvMerge";
/// Bundle identifier / Windows AppUserModelID; equal to 0.1.0's.
pub const APP_IDENTIFIER: &str = "com.caoccao.batch-mkvmerge";
/// Linux desktop id: basename of the installed `.desktop` file, the Wayland
/// app id, `StartupWMClass` and the launcher-progress URI all use it.
pub const DESKTOP_ID: &str = "BatchMkvMerge";
/// The application version, shown in the title and About.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The window title: `BatchMkvMerge v<version>`.
pub fn window_title() -> SharedString {
  format!("{APP_NAME} v{APP_VERSION}").into()
}

/// Starts the application and blocks until it quits.
pub fn run() {
  // Read before the event loop starts; creates the default file on first run.
  let config = bmm_core::config::get_config();
  gpui_kit::application().with_assets(assets::AppAssets).run(move |cx| {
    // gpui-kit's own strings (dialog buttons, input menus) look up the
    // application's locale files first; registered before `init`.
    i18n::install_component_strings();
    gpui_kit::init(cx);
    cx.set_app_identity(APP_IDENTIFIER, APP_NAME);
    i18n::set_language(config.language);
    theme::install(cx);
    theme::apply(&config, cx.window_appearance(), cx);
    let (merge_sink, merge_events) = merge_events::channel();
    let services = state::Services {
      persistence: state::Persistence::disk(),
      metadata: state::MetadataSource::parser(),
      merges: merge_events::MergeBackend::system(merge_sink),
      taskbar: platform::taskbar::Taskbar::system(),
      tools: tools::ToolDetector::system(),
    };
    let stores = state::Stores::new(config, services, cx);
    merge_events::start(&stores, merge_events, cx);
    ui::actions::bind_keys(cx);
    ui::actions::install(&stores, cx);
    platform::menus::install(cx);
    let Some(handle) = platform::window::open_main(&stores, cx) else {
      cx.quit();
      return;
    };
    platform::install_quit(handle, &stores, cx);
    platform::ingest_launch_args(&stores, cx);
    actions::watch_better_media_info(&stores, cx);
    updates::start(&stores, updates::UpdateSource::github(), cx);
    // Bring the app to the front also when started from a terminal; GPUI
    // delivers focus events only to the active window.
    cx.activate(true);
    // The app ends with its main window; an open topmost notification does
    // not keep it running.
    let main = handle.window_id();
    cx.on_window_closed(move |cx, closed| {
      if closed == main || cx.windows().is_empty() {
        cx.quit();
      }
    })
    .detach();
  });
}
