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

import { dirname, sep as getSep } from "@tauri-apps/api/path";
import type { MediaTrack } from "./media-metadata";
import type { ConfigProfile } from "./protocol";

export function trackKey(track: MediaTrack): string {
  return `${track.type}:${track.id}`;
}

function parseLanguageFilter(filter: string): Set<string> | null {
  const items = filter
    .split(",")
    .map((s) => s.trim().toLowerCase())
    .filter(Boolean);
  return items.length === 0 ? null : new Set(items);
}

function matchesLanguage(filter: Set<string> | null, codes: string[]): boolean {
  if (filter === null) {
    return true;
  }
  // `codes` carries the track's terminologic + bibliographic + alpha-2 forms
  // (already lowercased), so a filter in any form matches.
  return codes.some((code) => filter.has(code));
}

export function makeTrackSelector(
  profile: ConfigProfile,
): (track: MediaTrack) => boolean {
  const videoLangs = parseLanguageFilter(profile.videoLanguagesForTrackSelection);
  const audioLangs = parseLanguageFilter(profile.audioLanguagesForTrackSelection);
  const subtitleLangs = parseLanguageFilter(
    profile.subtitleLanguagesForTrackSelection,
  );
  return (track: MediaTrack) => {
    switch (track.type) {
      // Video / audio / subtitle: unchecked selects every track of that type;
      // checked restricts to the configured language list.
      case "video":
        return profile.selectVideo
          ? matchesLanguage(videoLangs, track.languageCodes)
          : true;
      case "audio":
        return profile.selectAudio
          ? matchesLanguage(audioLangs, track.languageCodes)
          : true;
      case "subtitles":
        return profile.selectSubtitle
          ? matchesLanguage(subtitleLangs, track.languageCodes)
          : true;
      // Chapters / attachments: checked adds every track of that type,
      // unchecked adds none.
      case "chapters":
        return profile.selectChapters;
      case "attachment":
        return profile.selectAttachments;
      default:
        return false;
    }
  };
}

export function getParentDir(path: string): string {
  const lastSlash = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return lastSlash >= 0 ? path.slice(0, lastSlash) : "";
}

export function getFileName(path: string): string {
  const lastSlash = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return lastSlash >= 0 ? path.slice(lastSlash + 1) : path;
}

export function getDriveKey(path: string): string {
  const driveLetter = path.match(/^([a-zA-Z]):/);
  if (driveLetter) {
    return `${driveLetter[1].toUpperCase()}:`;
  }
  const unc = path.match(/^(\\\\[^\\/]+[\\/][^\\/]+)/);
  if (unc) {
    return unc[1].toUpperCase();
  }
  return "default";
}

export async function resolveOutputDir(
  file: string,
  override: string | undefined,
): Promise<string> {
  if (override && override.length > 0) {
    return override;
  }
  return await dirname(file);
}

/** Quote an argument for the copyable shell command (paths with spaces, …). */
function shellQuote(value: string): string {
  if (value.length > 0 && !/[\s"'\\]/.test(value)) {
    return value;
  }
  return `"${value.replace(/"/g, '\\"')}"`;
}

/**
 * Build the mkvmerge argv that merges `sourceFile`'s selected `tracks` into a
 * single Matroska file at `outputPath`. Everything before the source file is a
 * per-file option; `--track-order` (global) trails it. Mirrors mkvtoolnix's own
 * merge command (mkvtoolnix-gui `merge/track.cpp`):
 *
 *   mkvmerge -o <out> [-d/-a/-s <ids> | --no-video/audio/subtitles]
 *     [--default-track-flag <id>:0|1] [--forced-display-flag <id>:0|1]
 *     [--no-chapters] [--attachments <ids> | --no-attachments]
 *     <input> --track-order 0:<id>,…
 *
 * `tracks` are the *selected* rows in the table's (possibly drag-reordered)
 * order. The `_profile` is threaded for future merge tuning but unused today.
 */
export function buildMergeArgs(
  sourceFile: string,
  outputPath: string,
  tracks: MediaTrack[],
  _profile: ConfigProfile,
): string[] {
  const args: string[] = ["-o", outputPath];

  // Per media type: keep the selected ids (`-d/-a/-s`), or drop the whole type
  // (`--no-video/...`) when none are selected.
  const selectByType = (
    type: string,
    selectFlag: string,
    noFlag: string,
  ): MediaTrack[] => {
    const selected = tracks.filter((t) => t.type === type);
    if (selected.length === 0) {
      args.push(noFlag);
    } else {
      args.push(selectFlag, selected.map((t) => String(t.id)).join(","));
    }
    return selected;
  };
  const video = selectByType("video", "-d", "--no-video");
  const audio = selectByType("audio", "-a", "--no-audio");
  const subtitles = selectByType("subtitles", "-s", "--no-subtitles");

  // Per-track language / name / flags. mkvmerge takes each as a `<id>:<value>`
  // option attached to the source file (mirrors mkvtoolnix-gui's
  // merge/track.cpp). Language and name are emitted whenever the track carries
  // a value in the table; default / forced flags only when explicitly set so
  // "unspecified" preserves the source track's own flag.
  for (const track of [...video, ...audio, ...subtitles]) {
    if (track.language) {
      args.push("--language", `${track.id}:${track.language}`);
    }
    if (track.trackName) {
      args.push("--track-name", `${track.id}:${track.trackName}`);
    }
    if (track.defaultTrack !== "unspecified") {
      args.push(
        "--default-track-flag",
        `${track.id}:${track.defaultTrack === "true" ? 1 : 0}`,
      );
    }
    if (track.forced !== "unspecified") {
      args.push(
        "--forced-display-flag",
        `${track.id}:${track.forced === "true" ? 1 : 0}`,
      );
    }
  }

  // Chapters: keep only when the chapters row is selected.
  if (!tracks.some((t) => t.type === "chapters")) {
    args.push("--no-chapters");
  }
  // Attachments: keep only the selected ones, or none at all.
  const attachmentIds = tracks
    .filter((t) => t.type === "attachment")
    .map((t) => String(t.id));
  if (attachmentIds.length === 0) {
    args.push("--no-attachments");
  } else {
    args.push("--attachments", attachmentIds.join(","));
  }

  // Source file — every per-file option above attaches to it.
  args.push(sourceFile);

  // Track order: the selected media tracks, in the table's order. The input
  // file is file id 0.
  const order = tracks
    .filter(
      (t) =>
        t.type === "video" || t.type === "audio" || t.type === "subtitles",
    )
    .map((t) => `0:${t.id}`);
  if (order.length > 0) {
    args.push("--track-order", order.join(","));
  }
  return args;
}

export function buildCommandString(
  sourceFile: string,
  outputPath: string,
  mkvToolNixPath: string,
  tracks: MediaTrack[],
  profile: ConfigProfile,
): string {
  const mkvmergePath = `${mkvToolNixPath}${getSep()}mkvmerge`;
  const args = buildMergeArgs(sourceFile, outputPath, tracks, profile);
  return [mkvmergePath, ...args].map(shellQuote).join(" ");
}

export function formatHMS(ms: number): string {
  if (ms < 0 || !Number.isFinite(ms)) {
    return "--:--:--";
  }
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(h)}:${pad(m)}:${pad(s)}`;
}

/** Human-readable byte size using binary (1024) units. Empty string for
 *  missing / invalid values. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) {
    return "";
  }
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = unit === 0 ? String(value) : value.toFixed(2);
  return `${text} ${units[unit]}`;
}

/** Human-readable bit rate (bits per second) using decimal (1000) units. Empty
 *  string for missing / invalid values. */
export function formatBitRate(bps: number): string {
  if (!Number.isFinite(bps) || bps < 0) {
    return "";
  }
  if (bps >= 1_000_000) {
    return `${(bps / 1_000_000).toFixed(2)} Mb/s`;
  }
  if (bps >= 1000) {
    return `${Math.round(bps / 1000)} kb/s`;
  }
  return `${bps} b/s`;
}
