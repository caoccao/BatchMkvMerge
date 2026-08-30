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
mod config;
mod constants;
mod controller;
pub mod media_metadata;
mod merge;
mod mkvtoolnix;
mod notification;
mod protocol;
mod window_state;

use media_metadata::model::MediaMetadata;
use protocol::{MediaMetadataErrorPayload, UpdateCheckResult, UpdateCheckState};

#[tauri::command]
async fn cancel_merge(file: String) -> Result<(), String> {
  controller::cancel_merge(file).map_err(convert_error)
}

#[tauri::command]
async fn check_output_path_writable(path: String) -> Result<bool, String> {
  controller::check_output_path_writable(path)
    .await
    .map_err(convert_error)
}

#[tauri::command]
fn close_topmost_notification(
  window: tauri::WebviewWindow,
  state: tauri::State<'_, notification::NotificationState>,
) -> Result<(), String> {
  controller::close_topmost_notification(&window, &state).map_err(convert_error)
}

fn convert_error(error: anyhow::Error) -> String {
  error.to_string()
}

#[tauri::command]
async fn detect_better_media_info(
  path: String,
  check_running: bool,
) -> Result<protocol::BetterMediaInfoStatus, String> {
  controller::detect_better_media_info(path, check_running)
    .await
    .map_err(convert_error)
}

#[tauri::command]
async fn enqueue_merge(file: String, args: Vec<String>) -> Result<(), String> {
  controller::enqueue_merge(file, args).map_err(convert_error)
}

#[tauri::command]
async fn get_about() -> Result<protocol::About, String> {
  controller::get_about().await.map_err(convert_error)
}

#[tauri::command]
async fn get_config() -> Result<config::Config, String> {
  controller::get_config().await.map_err(convert_error)
}

#[tauri::command]
fn get_launch_args() -> Vec<String> {
  controller::get_launch_args()
}

#[tauri::command]
async fn get_media_files(paths: Vec<String>) -> Result<Vec<String>, String> {
  controller::get_media_files(paths).await.map_err(convert_error)
}

#[tauri::command]
async fn get_media_metadata(file: String) -> Result<MediaMetadata, MediaMetadataErrorPayload> {
  controller::get_media_metadata(file).await
}

#[tauri::command]
async fn get_merge_status() -> Result<protocol::MergeSnapshot, String> {
  Ok(controller::get_merge_status())
}

#[tauri::command]
fn get_topmost_notification(
  state: tauri::State<'_, notification::NotificationState>,
) -> Result<Option<notification::TopmostNotification>, String> {
  controller::get_topmost_notification(&state).map_err(convert_error)
}

#[tauri::command]
fn get_update_result(state: tauri::State<'_, UpdateCheckState>) -> Option<UpdateCheckResult> {
  controller::get_update_result(&state)
}

#[tauri::command]
async fn is_mkvtoolnix_found(path: String, check_running: bool) -> Result<protocol::MkvToolNixStatus, String> {
  controller::is_mkvtoolnix_found(path, check_running)
    .await
    .map_err(convert_error)
}

#[tauri::command]
async fn launch_better_media_info(paths: Vec<String>) -> Result<(), String> {
  controller::launch_better_media_info(paths).await.map_err(convert_error)
}

#[tauri::command]
async fn output_path_exists(path: String) -> Result<bool, String> {
  controller::output_path_exists(path).await.map_err(convert_error)
}

#[tauri::command]
fn resolve_merge_output_path(output_dir: String, source_file: String) -> String {
  controller::resolve_merge_output_path(output_dir, source_file)
}

#[tauri::command]
fn resolve_overridden_output_path(output_path: String, source_file: String) -> String {
  controller::resolve_overridden_output_path(output_path, source_file)
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
  controller::set_config(config).await.map_err(convert_error)
}

#[tauri::command]
fn show_system_notification(app: tauri::AppHandle, title: String, file: String, detail: String) -> Result<(), String> {
  controller::show_system_notification(&app, title, file, detail).map_err(convert_error)
}

#[tauri::command]
async fn show_topmost_notification(
  app: tauri::AppHandle,
  state: tauri::State<'_, notification::NotificationState>,
  notification: notification::TopmostNotification,
) -> Result<(), String> {
  controller::show_topmost_notification(&app, &state, notification).map_err(convert_error)
}

#[tauri::command]
fn skip_version(version: String) -> Result<(), String> {
  controller::skip_version(version).map_err(convert_error)
}
