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

pub mod assets;

use gpui_kit::component::{ActiveTheme, v_flex};
use gpui_kit::*;

/// Product name: window title, menus, notifications.
pub const APP_NAME: &str = "BatchMkvMerge";
/// Bundle identifier / Windows AppUserModelID; equal to 0.1.0's.
pub const APP_IDENTIFIER: &str = "com.caoccao.batch-mkvmerge";
/// Linux desktop id: basename of the installed `.desktop` file, the Wayland
/// app id, `StartupWMClass` and the launcher-progress URI all use it.
pub const DESKTOP_ID: &str = "BatchMkvMerge";
/// The application version, shown in the title and About.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const DEFAULT_WINDOW_SIZE: (f32, f32) = (1024.0, 768.0);
const MIN_WINDOW_SIZE: (f32, f32) = (600.0, 450.0);

actions!(batch_mkvmerge, [Quit]);

/// The window title: `BatchMkvMerge v<version>`.
pub fn window_title() -> SharedString {
  format!("{APP_NAME} v{APP_VERSION}").into()
}

/// Starts the application and blocks until it quits.
pub fn run() {
  gpui_kit::application().with_assets(assets::AppAssets).run(|cx| {
    gpui_kit::init(cx);
    cx.set_app_identity(APP_IDENTIFIER, APP_NAME);
    #[cfg(target_os = "macos")]
    install_app_menu(cx);
    open_main_window(cx);
    // Bring the app to the front also when started from a terminal; GPUI
    // delivers focus events only to the active window.
    cx.activate(true);
    cx.on_window_closed(|cx, _| {
      if cx.windows().is_empty() {
        cx.quit();
      }
    })
    .detach();
  });
}

/// macOS application menu. Elsewhere Ctrl+Q is the app's Clear shortcut and
/// closing the last window quits.
#[cfg(target_os = "macos")]
fn install_app_menu(cx: &mut App) {
  cx.on_action(|_: &Quit, cx| cx.quit());
  cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
  cx.set_menus([Menu::new(APP_NAME).items([MenuItem::action("Quit", Quit)])]);
}

/// Window options of the main window: centred default size, the 600×450
/// minimum of 0.2.0, the desktop id as the Wayland app id.
pub fn main_window_options(cx: &App) -> WindowOptions {
  let (width, height) = DEFAULT_WINDOW_SIZE;
  let (min_width, min_height) = MIN_WINDOW_SIZE;
  WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
      None,
      size(px(width), px(height)),
      cx,
    ))),
    titlebar: Some(TitlebarOptions {
      title: Some(window_title()),
      ..Default::default()
    }),
    window_min_size: Some(size(px(min_width), px(min_height))),
    app_id: Some(DESKTOP_ID.into()),
    ..Default::default()
  }
}

fn open_main_window(cx: &mut App) {
  let options = main_window_options(cx);
  if let Err(error) = gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| SkeletonView)) {
    log::error!("Failed to open the main window: {error}");
    cx.quit();
  }
}

/// Placeholder content until the real layout lands.
pub struct SkeletonView;

impl Render for SkeletonView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    v_flex()
      .size_full()
      .items_center()
      .justify_center()
      .gap_3()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .child(img(assets::APP_ICON).size(px(128.0)))
      .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(APP_NAME))
      .child(
        div()
          .text_color(cx.theme().muted_foreground)
          .child(format!("v{APP_VERSION}")),
      )
  }
}
