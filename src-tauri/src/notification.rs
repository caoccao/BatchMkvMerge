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

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

const TOPMOST_NOTIFICATION_EVENT: &str = "topmost-notification";
const TOPMOST_NOTIFICATION_LABEL: &str = "notification";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopmostNotification {
  title: String,
  file: String,
  detail: String,
  close_label: String,
}

#[derive(Default)]
pub struct NotificationState {
  notification: Mutex<Option<TopmostNotification>>,
  window_lock: Mutex<()>,
}

fn center_on_main_window_monitor(
  app: &tauri::AppHandle,
  notification_window: &tauri::WebviewWindow,
) -> tauri::Result<()> {
  let Some(main_window) = app.get_webview_window("main") else {
    return notification_window.center();
  };
  let Some(monitor) = main_window.current_monitor()? else {
    return notification_window.center();
  };
  let work_area = monitor.work_area();
  let window_size = notification_window.outer_size()?;
  let (x, y) = centered_window_position(
    work_area.position.x,
    work_area.position.y,
    work_area.size.width,
    work_area.size.height,
    window_size.width,
    window_size.height,
  );
  notification_window.set_position(tauri::PhysicalPosition::new(x, y))
}

fn centered_window_position(
  work_area_x: i32,
  work_area_y: i32,
  work_area_width: u32,
  work_area_height: u32,
  window_width: u32,
  window_height: u32,
) -> (i32, i32) {
  let offset_x = work_area_width.saturating_sub(window_width) / 2;
  let offset_y = work_area_height.saturating_sub(window_height) / 2;
  (
    work_area_x.saturating_add(offset_x as i32),
    work_area_y.saturating_add(offset_y as i32),
  )
}

pub fn close_topmost_notification(window: &tauri::WebviewWindow, state: &NotificationState) -> Result<()> {
  if window.label() != TOPMOST_NOTIFICATION_LABEL {
    return Err(anyhow!("Only the topmost notification window can call this command"));
  }
  let _window_guard = state.window_lock.lock().map_err(|err| anyhow!(err.to_string()))?;
  window.destroy()?;
  Ok(())
}

pub fn get_topmost_notification(state: &NotificationState) -> Result<Option<TopmostNotification>> {
  state
    .notification
    .lock()
    .map(|notification| notification.clone())
    .map_err(|err| anyhow!(err.to_string()))
}

pub fn show_topmost_notification(
  app: &tauri::AppHandle,
  state: &NotificationState,
  notification: TopmostNotification,
) -> Result<()> {
  *state.notification.lock().map_err(|err| anyhow!(err.to_string()))? = Some(notification.clone());

  let _window_guard = state.window_lock.lock().map_err(|err| anyhow!(err.to_string()))?;
  let notification_window = if let Some(window) = app.get_webview_window(TOPMOST_NOTIFICATION_LABEL) {
    window.emit(TOPMOST_NOTIFICATION_EVENT, &notification)?;
    window
  } else {
    tauri::WebviewWindowBuilder::new(
      app,
      TOPMOST_NOTIFICATION_LABEL,
      tauri::WebviewUrl::App("index.html?view=notification".into()),
    )
    .title(&notification.title)
    .inner_size(440.0, 200.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()?
  };

  notification_window.set_always_on_top(true)?;
  center_on_main_window_monitor(app, &notification_window)?;
  notification_window.show()?;
  notification_window.set_focus()?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn notification_window_is_centered_in_negative_monitor_work_area() {
    assert_eq!(centered_window_position(-1920, 0, 1920, 1040, 440, 200), (-1180, 420));
  }

  #[test]
  fn oversized_notification_window_stays_at_work_area_origin() {
    assert_eq!(centered_window_position(40, 60, 400, 180, 440, 200), (40, 60));
  }
}
