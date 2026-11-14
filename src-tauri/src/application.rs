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

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use bmm_core::config;
use bmm_core::merge::{self, MergeEvent};
use bmm_core::protocol::{MergeFinishedEvent, UpdateCheckResult};
use bmm_core::update::{self, StartupUpdateCheck};
use tauri::{Emitter, Manager};

use crate::APP_VERSION;
use crate::notification::NotificationState;
use crate::window_state;

const EVENT_MERGE_FINISHED: &str = "merge-finished";

/// Result of the background update check, read by `get_update_result`.
pub struct UpdateCheckState {
  pub result: Arc<Mutex<Option<UpdateCheckResult>>>,
}

pub fn builder() -> tauri::Builder<tauri::Wry> {
  tauri::Builder::default()
    .manage(UpdateCheckState {
      result: Arc::new(Mutex::new(None)),
    })
    .manage(NotificationState::default())
    .plugin(tauri_plugin_clipboard_manager::init())
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_opener::init())
    .invoke_handler(tauri::generate_handler![
      crate::cancel_merge,
      crate::check_output_path_writable,
      crate::close_topmost_notification,
      crate::detect_better_media_info,
      crate::enqueue_merge,
      crate::get_about,
      crate::get_config,
      crate::get_launch_args,
      crate::get_media_files,
      crate::get_media_metadata,
      crate::get_merge_status,
      crate::get_topmost_notification,
      crate::get_update_result,
      crate::is_mkvtoolnix_found,
      crate::launch_better_media_info,
      crate::output_path_exists,
      crate::resolve_merge_output_path,
      crate::resolve_overridden_output_path,
      crate::set_config,
      crate::show_system_notification,
      crate::show_topmost_notification,
      crate::skip_version
    ])
    .setup(setup)
    .on_window_event(on_window_event)
}

fn check_for_updates_in_background(result_state: Arc<Mutex<Option<UpdateCheckResult>>>) {
  std::thread::spawn(move || {
    let result = update::run_check(APP_VERSION);
    *result_state.lock().unwrap() = Some(result);
  });
}
fn configure_main_window(app: &tauri::App<tauri::Wry>) -> tauri::Result<()> {
  let window = app.get_webview_window("main").unwrap();
  window.set_title(&format!("BatchMkvMerge v{}", APP_VERSION))?;

  let mut config = config::get_config();
  let (window_width, window_height) =
    window_state::sanitize_window_size(config.window.size.width, config.window.size.height);
  let should_save_sanitized_size =
    config.window.size.width != window_width || config.window.size.height != window_height;
  config.window.size.width = window_width;
  config.window.size.height = window_height;
  let _ = window.set_size(tauri::LogicalSize::new(window_width, window_height));
  if config.window.position.x < 0 || config.window.position.y < 0 {
    let _ = window.center();
  } else {
    let _ = window.set_position(tauri::LogicalPosition::new(
      config.window.position.x,
      config.window.position.y,
    ));
  }
  if should_save_sanitized_size {
    let _ = config::set_config(config);
  }

  let _ = window.show();
  let _ = window.set_focus();
  Ok(())
}

pub fn configure_runtime() -> tokio::runtime::Runtime {
  #[cfg(target_os = "linux")]
  window_state::configure_linux_webkit_renderer();

  let runtime = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(4)
    .enable_all()
    .build()
    .expect("Failed to build Tokio runtime");
  // Tauri stores only this cloned handle. The caller must retain the Runtime
  // returned below for as long as the application event loop is running.
  tauri::async_runtime::set(runtime.handle().clone());
  runtime
}

fn configure_update_check(app: &tauri::App<tauri::Wry>) {
  let result = app.state::<UpdateCheckState>().result.clone();
  match update::startup_check(&config::get_config(), update::current_timestamp(), APP_VERSION) {
    StartupUpdateCheck::CheckNow => check_for_updates_in_background(result),
    StartupUpdateCheck::Cached(cached) => *result.lock().unwrap() = Some(cached),
  }
}
pub fn on_window_event(window: &tauri::Window<tauri::Wry>, event: &tauri::WindowEvent) {
  if !window_state::WINDOW_READY.load(Ordering::SeqCst) || window.label() != "main" {
    return;
  }
  match event {
    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => save_window_state(window),
    _ => {}
  }
}

fn save_window_state(window: &tauri::Window<tauri::Wry>) {
  if window.is_minimized().unwrap_or(false) {
    return;
  }
  let Ok(scale) = window.scale_factor() else {
    return;
  };
  let Ok(position) = window.outer_position() else {
    return;
  };
  let Ok(size) = window.inner_size() else {
    return;
  };
  let logical_position: tauri::LogicalPosition<i32> = position.to_logical(scale);
  let logical_size: tauri::LogicalSize<u32> = size.to_logical(scale);
  if !window_state::is_persistable_window_size(logical_size.width, logical_size.height) {
    return;
  }
  let mut config = config::get_config();
  config.window.position.x = logical_position.x;
  config.window.position.y = logical_position.y;
  config.window.size.width = logical_size.width;
  config.window.size.height = logical_size.height;
  if let Err(error) = config::set_config(config) {
    log::error!("Couldn't save window state because {}", error);
  }
}

/// Forward `Finished` as the `merge-finished` event the frontend listens for.
/// The React queue still polls `get_merge_status` for progress, so `Started`
/// and `Progress` are not forwarded.
fn merge_event_sink(handle: tauri::AppHandle) -> merge::MergeEventSink {
  Box::new(move |event| {
    if let MergeEvent::Finished {
      file, outcome, error, ..
    } = event
    {
      let payload = MergeFinishedEvent {
        file,
        outcome: outcome.as_str().to_owned(),
        error,
      };
      if let Err(err) = handle.emit(EVENT_MERGE_FINISHED, payload) {
        log::error!("Failed to emit merge-finished event: {}", err);
      }
    }
  })
}

pub fn setup(app: &mut tauri::App<tauri::Wry>) -> Result<(), Box<dyn std::error::Error>> {
  merge::init(merge_event_sink(app.handle().clone()));
  configure_main_window(app)?;
  configure_update_check(app);
  window_state::WINDOW_READY.store(true, Ordering::SeqCst);
  Ok(())
}
