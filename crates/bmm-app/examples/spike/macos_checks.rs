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

//! Unattended macOS runtime checks of the prototypes (`BMM_SPIKE_CHECK=1`).
//! Each check prints one `check <name> <value>` line.

use std::time::Duration;

use gpui_kit::*;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSView, NSWindow};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::SpikeView;

fn report(name: &str, value: impl std::fmt::Display) {
  println!("check {name} {value}");
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
  let handle = HasWindowHandle::window_handle(window).ok()?;
  let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
    return None;
  };
  // SAFETY: GPUI hands out the live content view of this window.
  let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
  view.window()
}

fn dock_has_content_view() -> bool {
  let mtm = MainThreadMarker::new().expect("main thread");
  NSApplication::sharedApplication(mtm)
    .dockTile()
    .contentView(mtm)
    .is_some()
}

fn app_is_active() -> bool {
  let mtm = MainThreadMarker::new().expect("main thread");
  NSApplication::sharedApplication(mtm).isActive()
}

fn describe(window: &Window) -> String {
  match ns_window(window) {
    Some(ns) => format!(
      "level={} visible={} key={} hidesOnDeactivate={}",
      ns.level(),
      ns.isVisible(),
      ns.isKeyWindow(),
      ns.hidesOnDeactivate()
    ),
    None => "no-ns-window".into(),
  }
}

pub fn run(main: AnyWindowHandle, view: Entity<SpikeView>, cx: &mut App) {
  cx.spawn(async move |cx| {
    let executor = cx.background_executor().clone();
    let pause = |ms| executor.timer(Duration::from_millis(ms));
    pause(500).await;

    // 1. Dock tile progress.
    let progress = cx.update(|cx| view.read(cx).progress.clone());
    let _ = main.update(cx, |_, window, cx| {
      progress.update(cx, |p, cx| p.set(Some(40), window, cx))
    });
    report("dock.content_view_after_set_40", dock_has_content_view());
    let _ = main.update(cx, |_, window, cx| {
      progress.update(cx, |p, cx| p.set(Some(100), window, cx))
    });
    report("dock.content_view_after_set_100", dock_has_content_view());
    let _ = main.update(cx, |_, window, cx| progress.update(cx, |p, cx| p.set(None, window, cx)));
    report("dock.content_view_after_hide", dock_has_content_view());

    // 2. Topmost window: level, key status, reuse.
    let topmost = cx.update(|cx| view.read(cx).topmost.clone());
    let _ = main.update(cx, |_, window, cx| topmost.update(cx, |t, cx| t.show(window, cx)));
    pause(400).await;
    let popup = cx.update(|cx| topmost.read(cx).popup()).expect("popup opened");
    let _ = main.update(cx, |_, window, _| report("topmost.main_window", describe(window)));
    let _ = popup.update(cx, |_, window, _| report("topmost.popup_after_open", describe(window)));
    report("topmost.app_active_after_open", app_is_active());
    let _ = main.update(cx, |_, window, cx| topmost.update(cx, |t, cx| t.show(window, cx)));
    pause(200).await;
    report(
      "topmost.windows_opened_after_second_show",
      cx.update(|cx| topmost.read(cx).opened_windows),
    );

    // 3. Another application in front: deactivate this one.
    cx.update(|_| {
      let mtm = MainThreadMarker::new().expect("main thread");
      NSApplication::sharedApplication(mtm).deactivate();
    });
    pause(800).await;
    report("topmost.app_active_after_deactivate", app_is_active());
    let _ = popup.update(cx, |_, window, _| {
      report("topmost.popup_after_deactivate", describe(window))
    });

    // 4. Esc closes the popup through its key binding.
    let _ = popup.update(cx, |_, window, cx| {
      window.dispatch_keystroke(Keystroke::parse("escape").expect("valid keystroke"), cx);
    });
    pause(300).await;
    let still_open = popup.update(cx, |_, _, _| ()).is_ok();
    report("topmost.popup_open_after_escape", still_open);

    cx.update(|cx| cx.quit());
  })
  .detach();
}
