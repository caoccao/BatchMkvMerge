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

import { invoke } from "@tauri-apps/api/core";
import type {
  About,
  BetterMediaInfoStatus,
  Config,
  MergeSnapshot,
  MediaMetadata,
  MkvToolNixStatus,
  UpdateCheckResult,
} from "./protocol";

export async function getAbout(): Promise<About> {
  return await invoke<About>("get_about");
}

export async function getConfig(): Promise<Config> {
  return await invoke<Config>("get_config");
}

export async function setConfig(config: Config): Promise<Config> {
  return await invoke<Config>("set_config", { config });
}

export async function getMediaFiles(paths: string[]): Promise<string[]> {
  return await invoke<string[]>("get_media_files", { paths });
}

export async function getLaunchArgs(): Promise<string[]> {
  return await invoke<string[]>("get_launch_args");
}

export async function isMkvtoolnixFound(
  path: string,
  checkRunning: boolean = false,
): Promise<MkvToolNixStatus> {
  return await invoke<MkvToolNixStatus>("is_mkvtoolnix_found", {
    path,
    checkRunning,
  });
}

export async function getMediaMetadata(file: string): Promise<MediaMetadata> {
  return await invoke<MediaMetadata>("get_media_metadata", { file });
}

export async function enqueueMerge(
  file: string,
  args: string[],
): Promise<void> {
  return await invoke<void>("enqueue_merge", { file, args });
}

export async function cancelMerge(file: string): Promise<void> {
  return await invoke<void>("cancel_merge", { file });
}

export async function getMergeStatus(): Promise<MergeSnapshot> {
  return await invoke<MergeSnapshot>("get_merge_status");
}

export async function checkOutputPathWritable(path: string): Promise<boolean> {
  return await invoke<boolean>("check_output_path_writable", { path });
}

export async function ensureOutputPath(path: string): Promise<void> {
  return await invoke<void>("ensure_output_path", { path });
}

/** `<outputDir>/<sourceStem>.mkv`, with " (1)", " (2)", … appended when a file
 *  with that name already exists, so merging never overwrites. */
export async function resolveMergeOutputPath(
  outputDir: string,
  sourceFile: string,
): Promise<string> {
  return await invoke<string>("resolve_merge_output_path", {
    outputDir,
    sourceFile,
  });
}

export async function detectBetterMediaInfo(
  path: string,
  checkRunning: boolean = false,
): Promise<BetterMediaInfoStatus> {
  return await invoke<BetterMediaInfoStatus>("detect_better_media_info", {
    path,
    checkRunning,
  });
}

export async function launchBetterMediaInfo(paths: string[]): Promise<void> {
  return await invoke<void>("launch_better_media_info", { paths });
}

export async function getUpdateResult(): Promise<UpdateCheckResult | null> {
  return await invoke<UpdateCheckResult | null>("get_update_result");
}

export async function skipVersion(version: string): Promise<void> {
  return await invoke<void>("skip_version", { version });
}
