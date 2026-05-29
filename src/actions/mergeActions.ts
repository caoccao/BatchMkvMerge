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

import {
  buildMergeArgs,
  buildMergeArgsMulti,
  resolveOutputDir,
  trackKey,
} from "../merge";
import type { MergeInput } from "../merge";
import type { MediaTrack } from "../media-metadata";
import type { Config, ConfigProfile } from "../protocol";
import { QueueItemStatus } from "../protocol";
import {
  cancelMerge as invokeCancelMerge,
  enqueueMerge,
  resolveMergeOutputPath,
  resolveOverriddenOutputPath,
} from "../service";
import { useMkvStore } from "../store";

type MkvStoreState = ReturnType<typeof useMkvStore.getState>;

function isActiveStatus(status: QueueItemStatus | undefined): boolean {
  return (
    status === QueueItemStatus.Waiting || status === QueueItemStatus.Merging
  );
}

export function getActiveProfile(config: Config | null): ConfigProfile | null {
  if (!config) {
    return null;
  }
  return (
    config.profiles.find((profile) => profile.name === config.activeProfile) ??
    config.profiles[0] ??
    null
  );
}

export function getSelectedTracksForFile(
  file: string,
  state: MkvStoreState = useMkvStore.getState(),
): MediaTrack[] {
  const tracks = state.fileTracks[file] ?? [];
  const selectedIds = new Set<string>(state.fileSelectedIds[file] ?? []);
  if (tracks.length === 0 || selectedIds.size === 0) {
    return [];
  }
  return tracks.filter((track) => selectedIds.has(trackKey(track)));
}

export interface EnqueueSelectedTracksOptions {
  file: string;
  selectedTracks: MediaTrack[];
  profile: ConfigProfile;
  skipIfActive?: boolean;
}

/**
 * Resolve the merge output path for a single-output card keyed by `file`.
 * A full-path override (set by the file-mode output dialog on single-root
 * cards) is honoured: if it names a file it's used verbatim (overwriting an
 * existing file is fine — the OS save dialog already confirmed that); if it
 * points at a directory the input file's original name is appended with no
 * " (1)" dedup. Otherwise fall back to `<resolved dir>/<stem>.mkv` with the
 * backend's auto-rename dedup.
 *
 * The app never creates the output directory — mkvmerge creates any missing
 * path components when it writes the merged file.
 */
async function resolveMergeOutputFor(
  state: MkvStoreState,
  file: string,
): Promise<string> {
  const override = state.fileOutputPaths[file];
  if (override && override.length > 0) {
    return await resolveOverriddenOutputPath(override, file);
  }
  const outputDir = await resolveOutputDir(
    file,
    state.fileOutputDirs[file],
    state.globalOutputDir,
  );
  return await resolveMergeOutputPath(outputDir, file);
}

export async function enqueueSelectedTracksForFile(
  options: EnqueueSelectedTracksOptions,
): Promise<boolean> {
  const { file, selectedTracks, profile, skipIfActive = true } = options;
  if (selectedTracks.length === 0) {
    return false;
  }
  const state = useMkvStore.getState();
  const status = state.queueItems[file]?.status;
  if (skipIfActive && isActiveStatus(status)) {
    return false;
  }
  const outputPath = await resolveMergeOutputFor(state, file);
  const args = buildMergeArgs(file, outputPath, selectedTracks, profile);
  await enqueueMerge(file, args);
  state.addToQueue(file);
  return true;
}

export interface EnqueueSelectedTracksForUnitOptions {
  /** Root file of the merge tree — the queue key and the output base name. */
  root: string;
  /** Member files with their selected tracks, root first. Members with no
   *  selected tracks are dropped so mkvmerge isn't handed a useless input. */
  inputs: MergeInput[];
  profile: ConfigProfile;
  skipIfActive?: boolean;
}

/**
 * Enqueue a multi-file merge tree as ONE output named after `root`. The whole
 * tree's selected tracks are flattened into a single multi-input mkvmerge
 * invocation ([`buildMergeArgsMulti`]); the queue is keyed by `root` (an
 * existing file, so the backend's path validation passes).
 */
export async function enqueueSelectedTracksForUnit(
  options: EnqueueSelectedTracksForUnitOptions,
): Promise<boolean> {
  const { root, inputs, profile, skipIfActive = true } = options;
  const nonEmpty = inputs.filter((input) => input.tracks.length > 0);
  if (nonEmpty.length === 0) {
    return false;
  }
  const state = useMkvStore.getState();
  const status = state.queueItems[root]?.status;
  if (skipIfActive && isActiveStatus(status)) {
    return false;
  }
  const outputPath = await resolveMergeOutputFor(state, root);
  const args = buildMergeArgsMulti(nonEmpty, outputPath, profile);
  await enqueueMerge(root, args);
  state.addToQueue(root);
  return true;
}

export async function cancelMerge(
  file: string,
  onError?: (error: unknown, file: string) => void,
): Promise<void> {
  useMkvStore.getState().markCancelRequested(file);
  try {
    await invokeCancelMerge(file);
  } catch (error) {
    onError?.(error, file);
  }
}

export async function cancelMerges(
  files: string[],
  onError?: (error: unknown, file: string) => void,
): Promise<void> {
  await Promise.all(files.map((file) => cancelMerge(file, onError)));
}
