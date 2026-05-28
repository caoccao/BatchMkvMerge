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

export interface About {
  appVersion: string;
}

export const DisplayMode = {
  Auto: "Auto",
  Light: "Light",
  Dark: "Dark",
} as const;
export type DisplayMode = (typeof DisplayMode)[keyof typeof DisplayMode];

export const Theme = {
  Ocean: "Ocean",
  Aqua: "Aqua",
  Sky: "Sky",
  Arctic: "Arctic",
  Glacier: "Glacier",
  Mist: "Mist",
  Slate: "Slate",
  Charcoal: "Charcoal",
  Midnight: "Midnight",
  Indigo: "Indigo",
  Violet: "Violet",
  Lavender: "Lavender",
  Rose: "Rose",
  Blush: "Blush",
  Coral: "Coral",
  Sunset: "Sunset",
  Amber: "Amber",
  Sand: "Sand",
  Forest: "Forest",
  Emerald: "Emerald",
} as const;
export type Theme = (typeof Theme)[keyof typeof Theme];

export const Language = {
  De: "de",
  EnUS: "en-US",
  Es: "es",
  Fr: "fr",
  It: "it",
  Ja: "ja",
  ZhCN: "zh-CN",
  ZhHK: "zh-HK",
  ZhTW: "zh-TW",
} as const;
export type Language = (typeof Language)[keyof typeof Language];

export interface ConfigWindowPosition {
  x: number;
  y: number;
}

export interface ConfigWindowSize {
  width: number;
  height: number;
}

export interface ConfigWindow {
  position: ConfigWindowPosition;
  size: ConfigWindowSize;
}

export const UpdateCheckInterval = {
  Daily: "Daily",
  Weekly: "Weekly",
  Monthly: "Monthly",
} as const;
export type UpdateCheckInterval =
  (typeof UpdateCheckInterval)[keyof typeof UpdateCheckInterval];

export const GroupMode = {
  None: "None",
  TrackCount: "TrackCount",
  TrackCountAndLanguage: "TrackCountAndLanguage",
} as const;
export type GroupMode = (typeof GroupMode)[keyof typeof GroupMode];

export interface ConfigUpdate {
  checkInterval: UpdateCheckInterval;
  lastChecked: number;
  lastVersion: string;
  ignoreVersion: string;
}

export interface UpdateCheckResult {
  hasUpdate: boolean;
  latestVersion: string | null;
}

export interface ConfigExternalTools {
  mkvToolNixPath: string;
  betterMediaInfoPath: string;
}

export interface ConfigProfile {
  name: string;
  selectVideo: boolean;
  selectAudio: boolean;
  selectSubtitle: boolean;
  selectChapters: boolean;
  selectAttachments: boolean;
  videoLanguages: string;
  audioLanguages: string;
  subtitleLanguages: string;
}

export interface ConfigParser {
  timeoutMs: number;
}

export const PARSER_DEFAULT_TIMEOUT_MS = 10000;
export const PARSER_MIN_TIMEOUT_MS = 100;
export const PARSER_MAX_TIMEOUT_MS = 60000;

export function createDefaultParserConfig(): ConfigParser {
  return { timeoutMs: PARSER_DEFAULT_TIMEOUT_MS };
}

export interface Config {
  displayMode: DisplayMode;
  theme: Theme;
  language: Language;
  externalTools: ConfigExternalTools;
  profiles: ConfigProfile[];
  activeProfile: string;
  window: ConfigWindow;
  update: ConfigUpdate;
  parser: ConfigParser;
  groupMode: GroupMode;
}

export const DEFAULT_PROFILE_NAME = "Default";
export const DEFAULT_LANGUAGES = "eng, chi, spa, ger, fre, jpn";

export function createDefaultProfile(name = DEFAULT_PROFILE_NAME): ConfigProfile {
  const isDefault = name === DEFAULT_PROFILE_NAME;
  return {
    name,
    selectVideo: false,
    selectAudio: isDefault,
    selectSubtitle: isDefault,
    selectChapters: isDefault,
    selectAttachments: false,
    videoLanguages: "",
    audioLanguages: DEFAULT_LANGUAGES,
    subtitleLanguages: DEFAULT_LANGUAGES,
  };
}

export interface MkvToolNixStatus {
  found: boolean;
  mkvToolNixPath: string;
}

export interface BetterMediaInfoStatus {
  found: boolean;
  path: string;
}

/**
 * Re-export the auto-generated parser types so most components import them
 * from a single module. The generated file is the source of truth; never
 * edit it by hand. Regenerate with:
 *   BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript
 */
export type {
  Attachment,
  ChapterSummary,
  Container,
  ContainerFormat,
  ContainerProperties,
  MediaMetadata,
  Track,
  TrackType,
  TrackFlag,
  CodecInfo,
  TrackProperties,
  CommonTrackProperties,
  AudioTrackProperties,
  VideoTrackProperties,
  SubtitleTrackProperties,
} from "./protocol.generated";

/**
 * Wire shape of the `get_media_metadata` rejection. The frontend switches on
 * `kind` to pick an i18n message; `detail` is a one-line fallback summary.
 */
export type MediaMetadataError =
  | { kind: "io"; detail: string }
  | { kind: "unexpectedEof"; detail: string }
  | { kind: "unrecognised"; detail: string }
  | { kind: "timeout"; budgetMs: number; stage: string; detail: string }
  | { kind: "malformed"; detail: string }
  | { kind: "oversizedElement"; detail: string }
  | { kind: "internal"; detail: string };

export enum QueueItemStatus {
  Waiting = "Waiting",
  Merging = "Merging",
  Completed = "Completed",
  Cancelled = "Cancelled",
  Failed = "Failed",
}

export type MergeActiveStatus =
  | QueueItemStatus.Waiting
  | QueueItemStatus.Merging;

export type MergeOutcome =
  | QueueItemStatus.Completed
  | QueueItemStatus.Cancelled
  | QueueItemStatus.Failed;

export interface MergeEntry {
  file: string;
  status: MergeActiveStatus;
  progress: number;
}

export interface MergeSnapshot {
  entries: MergeEntry[];
}

export interface MergeFinishedEvent {
  file: string;
  outcome: MergeOutcome;
  error: string | null;
}

export function getDisplayModes(): DisplayMode[] {
  return [DisplayMode.Auto, DisplayMode.Light, DisplayMode.Dark];
}

export function getThemes(): Theme[] {
  return Object.values(Theme);
}

export function getLanguages(): Language[] {
  return Object.values(Language);
}

export function getGroupModes(): GroupMode[] {
  return [
    GroupMode.None,
    GroupMode.TrackCount,
    GroupMode.TrackCountAndLanguage,
  ];
}

/** i18n key for a group mode's human-readable label. */
export function groupModeLabelKey(mode: GroupMode): string {
  switch (mode) {
    case GroupMode.None:
      return "groupMode.none";
    case GroupMode.TrackCountAndLanguage:
      return "groupMode.trackCountAndLanguage";
    default:
      return "groupMode.trackCount";
  }
}

const LANGUAGE_LABELS: Record<Language, string> = {
  "de": "Deutsch",
  "en-US": "English (US)",
  "es": "Español",
  "fr": "Français",
  "it": "Italiano",
  "ja": "日本語",
  "zh-CN": "简体中文",
  "zh-HK": "繁體中文 (香港)",
  "zh-TW": "繁體中文 (台灣)",
};

export function getLanguageLabel(language: Language): string {
  return LANGUAGE_LABELS[language];
}
