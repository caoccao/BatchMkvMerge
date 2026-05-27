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
  basename,
  dirname,
  extname,
  join,
  sep as getSep,
} from "@tauri-apps/api/path";
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

function matchesLanguage(
  filter: Set<string> | null,
  codes: string[],
): boolean {
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
  const videoLangs = parseLanguageFilter(profile.videoLanguages);
  const audioLangs = parseLanguageFilter(profile.audioLanguages);
  const subtitleLangs = parseLanguageFilter(profile.subtitleLanguages);
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

function attachmentExtension(codec: string): string {
  const lower = codec.toLowerCase();
  switch (lower) {
    case "jpeg":
      return "jpg";
    case "x-truetype-font":
      return "ttf";
    case "x-opentype-font":
    case "font-sfnt":
      return "otf";
    default:
      return lower || "bin";
  }
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

export function getTrackExtension(codecId: string, trackType: string): string {
  if (codecId.startsWith("V_")) {
    switch (codecId) {
      case "V_MPEGH/ISO/HEVC":
        return "h265";
      case "V_MPEG4/ISO/AVC":
        return "h264";
      case "V_MPEG1":
      case "V_MPEG2":
        return "mpg";
      case "V_MPEG4/ISO/SP":
      case "V_MPEG4/ISO/ASP":
      case "V_MPEG4/ISO/AP":
      case "V_MPEG4/MS/V3":
        return "mpeg4";
      case "V_MS/VFW/FOURCC":
        return "avi";
      case "V_VP8":
      case "V_VP9":
      case "V_AV1":
        return "ivf";
      case "V_THEORA":
        return "ogg";
      case "V_PRORES":
        return "prores";
      case "V_FFV1":
        return "ffv1";
    }
    if (codecId.startsWith("V_REAL/")) {
      return "rm";
    }
    return "bin";
  }
  if (codecId.startsWith("A_")) {
    switch (codecId) {
      case "A_AC3":
      case "A_AC3/BSID9":
      case "A_AC3/BSID10":
        return "ac3";
      case "A_EAC3":
        return "eac3";
      case "A_TRUEHD":
        return "thd";
      case "A_MLP":
        return "mlp";
      case "A_MPEG/L1":
        return "mp1";
      case "A_MPEG/L2":
        return "mp2";
      case "A_MPEG/L3":
        return "mp3";
      case "A_FLAC":
        return "flac";
      case "A_VORBIS":
        return "ogg";
      case "A_OPUS":
        return "opus";
      case "A_WAVPACK4":
        return "wv";
      case "A_TTA1":
        return "tta";
      case "A_ALAC":
        return "caf";
      default:
        if (codecId.startsWith("A_PCM/")) {
          return "wav";
        }
        if (codecId.startsWith("A_AAC")) {
          return "aac";
        }
        if (codecId.startsWith("A_DTS")) {
          return "dts";
        }
        if (codecId.startsWith("A_REAL/")) {
          return "rm";
        }
        return "bin";
    }
  }
  if (codecId.startsWith("S_")) {
    switch (codecId) {
      case "S_TEXT/UTF8":
      case "S_TEXT/ASCII":
        return "srt";
      case "S_TEXT/ASS":
      case "S_ASS":
        return "ass";
      case "S_TEXT/SSA":
      case "S_SSA":
        return "ssa";
      case "S_TEXT/WEBVTT":
        return "vtt";
      case "S_TEXT/USF":
        return "usf";
      case "S_VOBSUB":
        return "sub";
      case "S_HDMV/PGS":
        return "sup";
      case "S_HDMV/TEXTST":
        return "textst";
      case "S_KATE":
        return "ogg";
      default:
        return "bin";
    }
  }
  switch (trackType) {
    case "video":
    case "audio":
      return "bin";
    case "subtitles":
      return "srt";
    case "chapters":
      return "xml";
    case "attachment":
      return attachmentExtension(codecId);
    default:
      return "bin";
  }
}

export async function getFileNameWithoutExt(filePath: string): Promise<string> {
  const ext = await extname(filePath);
  return await basename(filePath, ext ? `.${ext}` : undefined);
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

export function buildOutputFileName(
  fileNameWithoutExt: string,
  track: MediaTrack,
  _profile: ConfigProfile,
): string {
  const ext = getTrackExtension(track.codecId, track.type);
  const hasLanguage =
    track.type === "video" ||
    track.type === "audio" ||
    track.type === "subtitles";
  const base = hasLanguage
    ? `${fileNameWithoutExt}.${track.id}.${track.language}`
    : `${fileNameWithoutExt}.${track.id}`;
  return `${base}.${ext}`;
}

interface ModeSegments {
  tracks: string[];
  chapters: string[];
  attachments: string[];
}

async function buildModeSegments(
  outputDir: string,
  fileNameWithoutExt: string,
  tracks: MediaTrack[],
  profile: ConfigProfile,
  quote: (s: string) => string,
): Promise<ModeSegments> {
  const result: ModeSegments = { tracks: [], chapters: [], attachments: [] };
  for (const track of tracks) {
    const outFile = await join(
      outputDir,
      buildOutputFileName(fileNameWithoutExt, track, profile),
    );
    if (track.type === "chapters") {
      result.chapters.push(quote(outFile));
    } else if (track.type === "attachment") {
      result.attachments.push(`${track.id}:${quote(outFile)}`);
    } else {
      result.tracks.push(`${track.id}:${quote(outFile)}`);
    }
  }
  return result;
}

export async function buildMergeArgs(
  file: string,
  outputDir: string,
  tracks: MediaTrack[],
  profile: ConfigProfile,
): Promise<string[]> {
  const fileNameWithoutExt = await getFileNameWithoutExt(file);
  const segments = await buildModeSegments(
    outputDir,
    fileNameWithoutExt,
    tracks,
    profile,
    (s) => s,
  );
  const results: string[] = [];
  if (segments.tracks.length > 0) {
    results.push("tracks");
    results.push(...segments.tracks);
  }
  if (segments.chapters.length > 0) {
    results.push("chapters");
    results.push(segments.chapters[0]);
  }
  if (segments.attachments.length > 0) {
    results.push("attachments");
    results.push(...segments.attachments);
  }
  return results;
}

export async function buildCommandString(
  file: string,
  outputDir: string,
  mkvToolNixPath: string,
  tracks: MediaTrack[],
  profile: ConfigProfile,
): Promise<string> {
  const sep = getSep();
  const mkvmergePath = `${mkvToolNixPath}${sep}mkvmerge`;
  const fileNameWithoutExt = await getFileNameWithoutExt(file);
  const quote = (s: string) => `"${s}"`;
  const segments = await buildModeSegments(
    outputDir,
    fileNameWithoutExt,
    tracks,
    profile,
    quote,
  );
  const parts: string[] = [];
  if (segments.tracks.length > 0) {
    parts.push("tracks", ...segments.tracks);
  }
  if (segments.chapters.length > 0) {
    parts.push("chapters", segments.chapters[0]);
  }
  if (segments.attachments.length > 0) {
    parts.push("attachments", ...segments.attachments);
  }
  return `"${mkvmergePath}" "${file}" ${parts.join(" ")}`;
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
