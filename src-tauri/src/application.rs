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

use tauri::Manager;

use crate::config;
use crate::controller;
use crate::merge;
use crate::notification::NotificationState;
use crate::protocol::{UpdateCheckResult, UpdateCheckState};
use crate::window_state;

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
      crate::show_topmost_notification,
      crate::skip_version
    ])
    .setup(setup)
    .on_window_event(on_window_event)
}

fn check_for_updates_in_background(result_state: Arc<Mutex<Option<UpdateCheckResult>>>) {
  std::thread::spawn(move || {
    let check_result = std::panic::catch_unwind(controller::check_for_updates).unwrap_or_else(|_| {
      log::error!("Update check panicked");
      Err(anyhow::anyhow!("Update check panicked"))
    });
    match check_result {
      Ok(update_result) => {
        let mut updated_config = config::get_config();
        updated_config.update.last_checked = current_timestamp();
        if let Some(ref version) = update_result.latest_version {
          updated_config.update.last_version = version.clone();
        }
        let _ = config::set_config(updated_config.clone());
        let final_result = if update_result.has_update
          && update_result.latest_version.as_deref() == Some(updated_config.update.ignore_version.as_str())
          && !updated_config.update.ignore_version.is_empty()
        {
          UpdateCheckResult {
            has_update: false,
            latest_version: None,
          }
        } else {
          update_result
        };
        *result_state.lock().unwrap() = Some(final_result);
      }
      Err(error) => {
        log::warn!("Update check failed: {}", error);
        *result_state.lock().unwrap() = Some(UpdateCheckResult {
          has_update: false,
          latest_version: None,
        });
      }
    }
  });
}

fn configure_main_window(app: &tauri::App<tauri::Wry>) -> tauri::Result<()> {
  let window = app.get_webview_window("main").unwrap();
  window.set_title(&format!("BatchMkvMerge v{}", env!("CARGO_PKG_VERSION")))?;

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
  let update_config = config::get_config();
  let interval_seconds: i64 = match update_config.update.check_interval {
    config::UpdateCheckInterval::Daily => 86_400,
    config::UpdateCheckInterval::Weekly => 604_800,
    config::UpdateCheckInterval::Monthly => 2_592_000,
  };
  if update_config.update.last_checked == 0
    || current_timestamp() - update_config.update.last_checked > interval_seconds
  {
    check_for_updates_in_background(result);
  } else if !update_config.update.last_version.is_empty()
    && controller::is_newer_version(&update_config.update.last_version, controller::get_app_version())
    && update_config.update.last_version != update_config.update.ignore_version
  {
    *result.lock().unwrap() = Some(UpdateCheckResult {
      has_update: true,
      latest_version: Some(update_config.update.last_version),
    });
  } else {
    *result.lock().unwrap() = Some(UpdateCheckResult {
      has_update: false,
      latest_version: None,
    });
  }
}

fn current_timestamp() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|duration| duration.as_secs() as i64)
    .unwrap_or(0)
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

pub fn setup(app: &mut tauri::App<tauri::Wry>) -> Result<(), Box<dyn std::error::Error>> {
  merge::init_app_handle(app.handle().clone());
  configure_main_window(app)?;
  configure_update_check(app);
  window_state::WINDOW_READY.store(true, Ordering::SeqCst);
  Ok(())
}
