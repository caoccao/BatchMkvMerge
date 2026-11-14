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

//! Phase 2 spike: prototypes of the four pieces gpui-kit has no ready-made
//! component for, plus a closable tab (`Tab::suffix`).
//!
//! `cargo run --release -p BatchMkvMerge --example spike`
//!
//! Environment switches for unattended runs:
//! - `BMM_SPIKE_CAPTURE=<dir>`: render every tab to `<dir>/tab-<n>.png` with
//!   `Window::render_to_image`, then quit.
//! - `BMM_SPIKE_CHECK=1`: run the macOS runtime checks (Dock tile, topmost
//!   window level, key focus and visibility after deactivation), print them
//!   and quit.
//! - `BMM_SPIKE_EXIT_AFTER_FIRST_FRAME=1`: print the time from `main` to the
//!   first presented frame and quit.

mod autocomplete;
mod languages;
#[cfg(target_os = "macos")]
mod macos_checks;
mod progress;
mod reorder;
mod topmost;

use std::rc::Rc;
use std::time::{Duration, Instant};

use batch_mkvmerge::{APP_IDENTIFIER, APP_NAME, assets::AppAssets, main_window_options};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::*;

const TAB_LABELS: [&str; 4] = ["Taskbar progress", "Topmost window", "Autocomplete", "Drag reorder"];

pub struct SpikeView {
  pub active: usize,
  pub about_open: bool,
  pub progress: Entity<progress::ProgressDemo>,
  pub topmost: Entity<topmost::TopmostDemo>,
  pub autocomplete: Entity<autocomplete::AutocompleteDemo>,
  pub reorder: Entity<reorder::ReorderTable>,
}

impl SpikeView {
  pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
    let table = Rc::new(languages::LanguageTable::load(languages::STRESS_OPTION_COUNT));
    Self {
      active: 0,
      about_open: true,
      progress: cx.new(|_| progress::ProgressDemo::new()),
      topmost: cx.new(|_| topmost::TopmostDemo::new()),
      autocomplete: cx.new(|cx| autocomplete::AutocompleteDemo::new(table, window, cx)),
      reorder: cx.new(|_| reorder::ReorderTable::new(40)),
    }
  }

  pub fn tab_count(&self) -> usize {
    TAB_LABELS.len() + usize::from(self.about_open)
  }

  /// Closing a tab selects its left neighbour when it was the active one.
  fn close_about(&mut self, cx: &mut Context<Self>) {
    let about = TAB_LABELS.len();
    if self.about_open {
      self.about_open = false;
      if self.active >= about {
        self.active = about - 1;
      }
      cx.notify();
    }
  }
}

impl Render for SpikeView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let mut tabs: Vec<Tab> = TAB_LABELS.iter().map(|label| Tab::new().label(*label)).collect();
    if self.about_open {
      tabs.push(
        Tab::new()
          .label("About")
          .suffix(
            Button::new("close-about")
              .ghost()
              .xsmall()
              .icon(IconName::X)
              .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_about(cx);
              })),
          ),
      );
    }
    let content: AnyElement = match self.active {
      0 => self.progress.clone().into_any_element(),
      1 => self.topmost.clone().into_any_element(),
      2 => self.autocomplete.clone().into_any_element(),
      3 => self.reorder.clone().into_any_element(),
      _ => v_flex()
        .p_4()
        .gap_2()
        .child(img(batch_mkvmerge::assets::APP_ICON).size(px(96.0)))
        .child(format!("{APP_NAME} spike · {APP_IDENTIFIER}"))
        .into_any_element(),
    };
    v_flex()
      .size_full()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .child(
        TabBar::new("spike-tabs")
          .selected_index(self.active)
          .children(tabs)
          .on_click(cx.listener(|this, index: &usize, _, cx| {
            this.active = *index;
            cx.notify();
          })),
      )
      .child(div().id("spike-content").flex_1().min_h_0().child(content))
  }
}

fn main() {
  let started = Instant::now();
  gpui_kit::application().with_assets(AppAssets).run(move |cx| {
    gpui_kit::init(cx);
    cx.set_app_identity(APP_IDENTIFIER, APP_NAME);
    topmost::bind_keys(cx);
    cx.on_window_closed(|cx, _| {
      if cx.windows().is_empty() {
        cx.quit();
      }
    })
    .detach();

    let mut options = main_window_options(cx);
    options.titlebar = Some(TitlebarOptions {
      title: Some(format!("{APP_NAME} spike").into()),
      ..Default::default()
    });
    let (handle, view) = match gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| SpikeView::new(window, cx)))
    {
      Ok(opened) => opened,
      Err(error) => {
        eprintln!("failed to open the spike window: {error}");
        cx.quit();
        return;
      }
    };

    cx.activate(true);
    if std::env::var_os("BMM_SPIKE_EXIT_AFTER_FIRST_FRAME").is_some() {
      let _ = handle.update(cx, |_, window, _| {
        window.on_next_frame(move |_, cx| {
          println!("first-frame-ms {}", started.elapsed().as_millis());
          cx.quit();
        });
      });
    }
    if let Some(dir) = std::env::var_os("BMM_SPIKE_CAPTURE") {
      capture_tabs(handle, view.clone(), dir.into(), cx);
    }
    #[cfg(target_os = "macos")]
    if std::env::var_os("BMM_SPIKE_CHECK").is_some() {
      macos_checks::run(handle, view, cx);
    }
  });
}

/// Renders every tab to a PNG (no Screen Recording permission needed), with
/// the bottom autocomplete list open on the Autocomplete tab, then quits.
fn capture_tabs(handle: AnyWindowHandle, view: Entity<SpikeView>, dir: std::path::PathBuf, cx: &mut App) {
  cx.spawn(async move |cx| {
    let _ = std::fs::create_dir_all(&dir);
    let tab_count = cx.update(|cx| view.read(cx).tab_count());
    for tab in 0..tab_count {
      let _ = handle.update(cx, |_, window, cx| {
        view.update(cx, |view, cx| {
          view.active = tab;
          cx.notify();
        });
        // Draw explicitly: an occluded window gets no display-link frames.
        window.render_frame(cx);
        if tab == 2 {
          let cell = view.read(cx).autocomplete.read(cx).bottom[1].clone();
          cell.update(cx, |cell, cx| cell.focus(window, cx));
        }
        window.render_frame(cx);
      });
      cx.background_executor().timer(Duration::from_millis(400)).await;
      let saved = handle.update(cx, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window
          .render_to_image()
          .map_err(|error| error.to_string())
          .and_then(|image| {
            image
              .save(dir.join(format!("tab-{tab}.png")))
              .map_err(|error| error.to_string())
          })
      });
      if tab == 2 {
        let _ = handle.update(cx, |_, window, cx| {
          let cell = view.read(cx).autocomplete.read(cx).bottom[1].clone();
          let cell = cell.read(cx);
          println!(
            "autocomplete open={} placement={:?} bounds={:?} viewport={:?}",
            cell.is_open(),
            cell.placement(),
            cell.debug_bounds(),
            window.viewport_size()
          );
        });
      }
      match saved {
        Ok(Ok(())) => println!("captured tab {tab}"),
        Ok(Err(error)) => println!("capture of tab {tab} failed: {error}"),
        Err(error) => println!("capture of tab {tab} failed: {error}"),
      }
    }
    cx.update(|cx| cx.quit());
  })
  .detach();
}

#[cfg(test)]
mod tests {
  // `gpui_kit::*` also exports GPUI's `test` macro, and `#[gpui_kit::test]`
  // expands to a plain `#[test]`; name the built-in one.
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::test::TestWindowExt;

  fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<SpikeView>) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      topmost::bind_keys(cx);
      let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
          Point::default(),
          size(px(1000.0), px(760.0)),
        ))),
        ..Default::default()
      };
      gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| SpikeView::new(window, cx))).expect("open window")
    })
  }

  #[gpui_kit::test]
  fn closing_a_tab_through_its_suffix_button_does_not_select_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.click("close-about", cx);
    })
    .unwrap();
    cx.read(|cx| {
      assert!(!view.read(cx).about_open);
      assert_eq!(view.read(cx).active, 0);
    });
  }

  #[gpui_kit::test]
  fn closing_the_active_tab_selects_its_left_neighbour(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
      view.update(cx, |view, cx| {
        view.active = 4;
        cx.notify();
      });
      window.render_frame(cx);
      window.click("close-about", cx);
    })
    .unwrap();
    cx.read(|cx| assert_eq!(view.read(cx).active, 3));
  }
}
