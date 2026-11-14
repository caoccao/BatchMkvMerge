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

mod application;
mod notification;
mod window_state;

use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::protocol::{self, MediaMetadataErrorPayload, UpdateCheckResult};
use bmm_core::{config, merge, mkvtoolnix, services, update};

use application::UpdateCheckState;

/// Version of the shipped application (this crate), shown in About and the
/// window title and compared against GitHub releases.
pub(crate) const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tauri::command]
async fn cancel_merge(file: String) -> Result<(), String> {
  merge::cancel(file).map_err(convert_error)
}

#[tauri::command]
async fn check_output_path_writable(path: String) -> Result<bool, String> {
  services::check_output_path_writable(path).map_err(convert_error)
}

#[tauri::command]
fn close_topmost_notification(
  window: tauri::WebviewWindow,
  state: tauri::State<'_, notification::NotificationState>,
) -> Result<(), String> {
  notification::close_topmost_notification(&window, &state).map_err(convert_error)
}

fn convert_error(error: anyhow::Error) -> String {
  error.to_string()
}

#[tauri::command]
async fn detect_better_media_info(
  path: String,
  check_running: bool,
) -> Result<protocol::BetterMediaInfoStatus, String> {
  services::detect_better_media_info(path, check_running).map_err(convert_error)
}

#[tauri::command]
async fn enqueue_merge(file: String, args: Vec<String>) -> Result<(), String> {
  merge::enqueue(file, args).map_err(convert_error)
}

#[tauri::command]
async fn get_about() -> Result<protocol::About, String> {
  Ok(protocol::About {
    app_version: APP_VERSION.to_owned(),
  })
}

#[tauri::command]
async fn get_config() -> Result<config::Config, String> {
  Ok(config::get_config())
}

#[tauri::command]
fn get_launch_args() -> Vec<String> {
  services::get_launch_args()
}

#[tauri::command]
async fn get_media_files(paths: Vec<String>) -> Result<Vec<String>, String> {
  services::get_media_files(paths).map_err(convert_error)
}

#[tauri::command]
async fn get_media_metadata(file: String) -> Result<MediaMetadata, MediaMetadataErrorPayload> {
  tauri::async_runtime::spawn_blocking(move || services::get_media_metadata(file))
    .await
    .map_err(|join_err| MediaMetadataErrorPayload::internal(join_err.to_string()))?
    .map_err(|err| MediaMetadataErrorPayload::from_parse_error(&err))
}

#[tauri::command]
async fn get_merge_status() -> Result<protocol::MergeSnapshot, String> {
  Ok(merge::snapshot())
}

#[tauri::command]
fn get_topmost_notification(
  state: tauri::State<'_, notification::NotificationState>,
) -> Result<Option<notification::TopmostNotification>, String> {
  notification::get_topmost_notification(&state).map_err(convert_error)
}

#[tauri::command]
fn get_update_result(state: tauri::State<'_, UpdateCheckState>) -> Option<UpdateCheckResult> {
  state.result.lock().unwrap().clone()
}

#[tauri::command]
async fn is_mkvtoolnix_found(path: String, check_running: bool) -> Result<protocol::MkvToolNixStatus, String> {
  mkvtoolnix::is_mkvtoolnix_found(path, check_running).map_err(convert_error)
}

#[tauri::command]
async fn launch_better_media_info(paths: Vec<String>) -> Result<(), String> {
  services::launch_better_media_info(paths).map_err(convert_error)
}

#[tauri::command]
async fn output_path_exists(path: String) -> Result<bool, String> {
  services::output_path_exists(path).map_err(convert_error)
}

#[tauri::command]
fn resolve_merge_output_path(output_dir: String, source_file: String) -> String {
  services::resolve_merge_output_path(output_dir, source_file)
}

#[tauri::command]
fn resolve_overridden_output_path(output_path: String, source_file: String) -> String {
  services::resolve_overridden_output_path(output_path, source_file)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let _runtime: tokio::runtime::Runtime = application::configure_runtime();
  application::builder()
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

#[tauri::command]
async fn set_config(config: config::Config) -> Result<config::Config, String> {
  config::set_config(config).map_err(convert_error)?;
  Ok(config::get_config())
}

#[tauri::command]
fn show_system_notification(app: tauri::AppHandle, title: String, file: String, detail: String) -> Result<(), String> {
  notification::show_system_notification(&app, title, file, detail).map_err(convert_error)
}

#[tauri::command]
async fn show_topmost_notification(
  app: tauri::AppHandle,
  state: tauri::State<'_, notification::NotificationState>,
  notification: notification::TopmostNotification,
) -> Result<(), String> {
  notification::show_topmost_notification(&app, &state, notification).map_err(convert_error)
}

#[tauri::command]
fn skip_version(version: String) -> Result<(), String> {
  update::skip_version(version).map_err(convert_error)
}
