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

//! The main window: initial geometry from the config, geometry persistence,
//! the title with the queue count, and the close request.
//!
//! Positions are logical pixels; sizes are the content size. GPUI's macOS
//! window reports its frame (title bar included) but opens with a content
//! size, so the saved size comes from the viewport, not the window bounds.

use bmm_core::config::{ConfigWindow, ConfigWindowPosition, ConfigWindowSize};
use gpui_kit::{
  AnyWindowHandle, App, AppContext, Bounds, Pixels, Size, TitlebarOptions, Window, WindowBounds, WindowOptions, point,
  px, size,
};

use super::close_guard;
use crate::state::Stores;
use crate::state::queue::QueueSummary;
use crate::ui::root::AppView;

/// The smallest window, and the smallest geometry that is saved.
pub const MIN_WINDOW_SIZE: (u32, u32) = (600, 450);

/// The saved size, raised to the minimum (a hand-edited config may hold less).
pub fn sanitize_size(size: &ConfigWindowSize) -> (u32, u32) {
  (size.width.max(MIN_WINDOW_SIZE.0), size.height.max(MIN_WINDOW_SIZE.1))
}

/// Whether a geometry of this size is worth saving.
pub fn is_persistable_size(width: u32, height: u32) -> bool {
  width >= MIN_WINDOW_SIZE.0 && height >= MIN_WINDOW_SIZE.1
}

/// Where the window opens: the saved position when it is not negative (the
/// "never saved" value) and lies on one of `displays`; `None` centres it.
pub fn initial_bounds(saved: &ConfigWindow, displays: &[Bounds<Pixels>]) -> Option<Bounds<Pixels>> {
  let (width, height) = sanitize_size(&saved.size);
  let position = &saved.position;
  if position.x < 0 || position.y < 0 {
    return None;
  }
  let origin = point(px(position.x as f32), px(position.y as f32));
  if !displays.iter().any(|display| display.contains(&origin)) {
    return None;
  }
  Some(Bounds::new(origin, size(px(width as f32), px(height as f32))))
}

/// The geometry to save for a window in `bounds` showing `content`: only a
/// normal (not maximized, not fullscreen) window of at least the minimum size
/// is saved, so the restore geometry survives maximizing.
pub fn geometry_to_save(bounds: WindowBounds, content: Size<Pixels>) -> Option<ConfigWindow> {
  let WindowBounds::Windowed(bounds) = bounds else {
    return None;
  };
  let width = f32::from(content.width).round().max(0.0) as u32;
  let height = f32::from(content.height).round().max(0.0) as u32;
  if !is_persistable_size(width, height) {
    return None;
  }
  Some(ConfigWindow {
    position: ConfigWindowPosition {
      x: f32::from(bounds.origin.x).round() as i32,
      y: f32::from(bounds.origin.y).round() as i32,
    },
    size: ConfigWindowSize { width, height },
  })
}

fn same_geometry(a: &ConfigWindow, b: &ConfigWindow) -> bool {
  a.position.x == b.position.x
    && a.position.y == b.position.y
    && a.size.width == b.size.width
    && a.size.height == b.size.height
}

/// Options of the main window: saved or centred geometry, the minimum size,
/// the title, the desktop id as the Wayland app id.
pub fn main_window_options(saved: &ConfigWindow, cx: &App) -> WindowOptions {
  let displays: Vec<Bounds<Pixels>> = cx.displays().iter().map(|display| display.bounds()).collect();
  let bounds = initial_bounds(saved, &displays).unwrap_or_else(|| {
    let (width, height) = sanitize_size(&saved.size);
    Bounds::centered(None, size(px(width as f32), px(height as f32)), cx)
  });
  WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(bounds)),
    titlebar: Some(TitlebarOptions {
      title: Some(crate::window_title()),
      ..Default::default()
    }),
    window_min_size: Some(size(px(MIN_WINDOW_SIZE.0 as f32), px(MIN_WINDOW_SIZE.1 as f32))),
    app_id: Some(crate::DESKTOP_ID.into()),
    ..Default::default()
  }
}

/// Opens the main window with the application shell; `None` if it failed.
pub fn open_main(stores: &Stores, cx: &mut App) -> Option<AnyWindowHandle> {
  let options = main_window_options(&stores.config.read(cx).config().window, cx);
  let stores = stores.clone();
  match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| AppView::new(stores, window, cx))) {
    Ok((handle, _)) => Some(handle),
    Err(error) => {
      log::error!("Failed to open the main window: {error}");
      None
    }
  }
}

/// Saves the window's geometry if it is persistable and changed.
pub fn save_geometry(stores: &Stores, window: &Window, cx: &mut App) {
  let Some(geometry) = geometry_to_save(window.window_bounds(), window.viewport_size()) else {
    return;
  };
  if same_geometry(&stores.config.read(cx).config().window, &geometry) {
    return;
  }
  stores
    .config
    .update(cx, |config, cx| config.update(cx, |config| config.window = geometry));
}

/// The window title: `BatchMkvMerge v<version>`, followed by ` - done/total`
/// while a merge is waiting or running.
pub fn title(summary: &QueueSummary) -> String {
  if summary.active {
    format!("{} - {}/{}", crate::window_title(), summary.done, summary.total)
  } else {
    crate::window_title().to_string()
  }
}

/// A request to close the main window (title bar, Alt+F4, Quit): saves the
/// geometry, then lets the close guard decide; its Yes cancels every active
/// merge, without the completion notifications their end would send.
/// `true` lets the window close.
pub fn close_requested(stores: &Stores, window: &mut Window, cx: &mut App) -> bool {
  save_geometry(stores, window, cx);
  let cancel_stores = stores.clone();
  close_guard::should_close(stores.has_active_merges(cx), window, cx, move |_, cx| {
    cancel_stores.ui.update(cx, |ui, _| ui.set_closing());
    crate::actions::cancel_all(&cancel_stores, cx)
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn saved(x: i32, y: i32, width: u32, height: u32) -> ConfigWindow {
    ConfigWindow {
      position: ConfigWindowPosition { x, y },
      size: ConfigWindowSize { width, height },
    }
  }

  fn display(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
  }

  #[test]
  fn the_title_counts_finished_merges_while_one_is_active() {
    let base = crate::window_title().to_string();
    assert_eq!(title(&QueueSummary::default()), base);
    let summary = QueueSummary {
      done: 1,
      total: 3,
      active: true,
      percent: 40,
    };
    assert_eq!(title(&summary), format!("{base} - 1/3"));
    let finished = QueueSummary {
      done: 3,
      total: 3,
      active: false,
      percent: 100,
    };
    assert_eq!(title(&finished), base);
  }

  #[test]
  fn the_default_config_opens_centred_at_its_size() {
    let config = ConfigWindow::default();
    assert_eq!(initial_bounds(&config, &[display(0.0, 0.0, 1920.0, 1080.0)]), None);
    assert_eq!(sanitize_size(&config.size), (1200, 900));
  }

  #[test]
  fn a_saved_position_on_a_display_is_restored() {
    let displays = [display(0.0, 0.0, 1920.0, 1080.0), display(1920.0, 0.0, 1280.0, 1024.0)];
    let bounds = initial_bounds(&saved(2000, 100, 800, 600), &displays).expect("restored");
    assert_eq!(bounds.origin, point(px(2000.0), px(100.0)));
    assert_eq!(bounds.size, size(px(800.0), px(600.0)));
    // Too small a saved size is raised to the minimum.
    let bounds = initial_bounds(&saved(10, 10, 100, 100), &displays).expect("restored");
    assert_eq!(bounds.size, size(px(600.0), px(450.0)));
  }

  #[test]
  fn a_position_off_every_display_centres_the_window() {
    let displays = [display(0.0, 0.0, 1920.0, 1080.0)];
    assert_eq!(initial_bounds(&saved(5000, 100, 800, 600), &displays), None);
    assert_eq!(initial_bounds(&saved(-1, 100, 800, 600), &displays), None);
  }

  #[test]
  fn only_a_normal_window_of_the_minimum_size_is_saved() {
    let bounds = display(10.4, 20.6, 1000.0, 830.0);
    let content = size(px(1000.0), px(802.0));
    let geometry = geometry_to_save(WindowBounds::Windowed(bounds), content).expect("saved");
    assert!(same_geometry(&geometry, &saved(10, 21, 1000, 802)));
    assert!(geometry_to_save(WindowBounds::Maximized(bounds), content).is_none());
    assert!(geometry_to_save(WindowBounds::Fullscreen(bounds), content).is_none());
    assert!(geometry_to_save(WindowBounds::Windowed(bounds), size(px(599.0), px(800.0))).is_none());
    assert!(geometry_to_save(WindowBounds::Windowed(bounds), size(px(800.0), px(449.0))).is_none());
  }
}
