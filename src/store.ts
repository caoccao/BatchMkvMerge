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

import { create } from "zustand";
import { getDriveKey, trackKey } from "./merge";
import type { MediaTrack } from "./media-metadata";
import { mediaTrackCounts, metadataToMediaTracks } from "./media-metadata";
import type {
  About,
  Config,
  ConfigAutomation,
  ConfigProfile,
  MergeEntry,
  MergeOutcome,
  MediaMetadata,
  TrackFlag,
} from "./protocol";

/** Which track flag a `setTrackFlag` call targets. */
export type TrackFlagKind = "default" | "forced";

/** Cycle a tri-state flag: checked → unchecked → unspecified → checked. */
export function nextTrackFlag(flag: TrackFlag): TrackFlag {
  if (flag === "true") {
    return "false";
  }
  if (flag === "false") {
    return "unspecified";
  }
  return "true";
}
import {
  DEFAULT_PROFILE_NAME,
  QueueItemStatus,
  createDefaultProfile,
} from "./protocol";
export { QueueItemStatus } from "./protocol";
import { getAbout, getConfig, setConfig } from "./service";

export type TabType = "fileList" | "queue" | "settings" | "about";

export interface TrackCounts {
  video: number;
  audio: number;
  subtitles: number;
  chapters: number;
  attachments: number;
}

function isTerminalStatus(status: QueueItemStatus): boolean {
  return (
    status === QueueItemStatus.Completed ||
    status === QueueItemStatus.Cancelled ||
    status === QueueItemStatus.Failed
  );
}

export interface QueueItem {
  file: string;
  drive: string;
  status: QueueItemStatus;
  progress: number;
  mergeStartedAt: number | null;
  mergeEndedAt: number | null;
  cancelRequested: boolean;
  error: string | null;
}

export type NotificationKind = "success" | "error";

export interface Notification {
  id: number;
  kind: NotificationKind;
  file: string;
  detail: string;
}

interface MkvStore {
  files: string[];
  activeTab: TabType;
  showSettings: boolean;
  showAbout: boolean;
  about: About | null;
  config: Config | null;
  queueItems: Record<string, QueueItem>;
  queueOrder: string[];
  /** Raw parser output, kept for future detail panels. */
  fileMetadata: Record<string, MediaMetadata>;
  /** UI-flattened rows derived from `fileMetadata` so the selection table /
   *  selectors don't recompute on every render. */
  fileTracks: Record<string, MediaTrack[]>;
  fileTrackCounts: Record<string, TrackCounts>;
  fileSelectedIds: Record<string, string[]>;
  fileOutputDirs: Record<string, string>;
  /** Session-only default output dir (not persisted). Consulted dynamically at
   *  merge/command-resolve time as the fallback when a card has no per-file
   *  override; undefined = fall back to each input file's own directory. */
  globalOutputDir: string | undefined;
  betterMediaInfoAvailable: boolean;
  /** Id of the currently active card (file path or group key); null = none. */
  activeCard: string | null;
  notification: Notification | null;
  addFiles: (paths: string[]) => void;
  removeFile: (path: string) => void;
  clearFiles: () => void;
  setActiveTab: (type: TabType) => void;
  openSettings: () => void;
  openAbout: () => void;
  closeSettings: () => void;
  closeAbout: () => void;
  initAbout: () => Promise<void>;
  initConfig: () => Promise<void>;
  updateConfig: (patch: Partial<Config>) => Promise<void>;
  updateActiveProfile: (patch: Partial<ConfigProfile>) => Promise<void>;
  addProfile: (name: string) => Promise<void>;
  deleteActiveProfile: () => Promise<void>;
  setActiveProfile: (name: string) => Promise<void>;
  resetActiveProfileTemplates: () => Promise<void>;
  applyMergeSnapshot: (entries: MergeEntry[]) => void;
  addToQueue: (file: string) => void;
  removeFromQueue: (file: string) => void;
  markCancelRequested: (file: string) => void;
  recordFinishedOutcome: (
    file: string,
    outcome: MergeOutcome,
    error: string | null,
  ) => void;
  clearCompletedInDrive: (drive: string) => void;
  /** Store parsed metadata for `file`; derive UI rows and counts in one shot. */
  setFileMetadata: (file: string, metadata: MediaMetadata) => void;
  setFileTrackCounts: (file: string, counts: TrackCounts) => void;
  setFileSelectedIds: (file: string, ids: string[]) => void;
  /** Set a track flag to an explicit value on matching keys across the given
   *  files. Cards compute the next value (cycling true → false → unspecified)
   *  from the clicked row, then apply it — the combined merge-tree table maps a
   *  single logical edit to heterogeneous per-file keys, so a card-side cycle is
   *  the only consistent option. */
  setTrackFlag: (
    files: string[],
    keys: string[],
    kind: TrackFlagKind,
    value: TrackFlag,
  ) => void;
  /** Drag-reorder: move the `fromKey` row to `toKey`'s position in every given
   *  file that contains both rows. The track `id` is intrinsic and unchanged —
   *  only the row order changes. */
  reorderTracks: (files: string[], fromKey: string, toKey: string) => void;
  /** Set the editable language code on every matching track key across all files. */
  setTrackLanguage: (files: string[], keys: string[], value: string) => void;
  /** Set the editable track name on every matching track key across all files. */
  setTrackName: (files: string[], keys: string[], value: string) => void;
  /** Apply the active profile's *language/name* automation to a freshly-parsed
   *  file's tracks (run once per newly added file): reset-und-language then
   *  set-track-name (which sees the updated language). The default/forced
   *  automation is applied separately by [`applyFlagAutomationToFile`] *after*
   *  auto-selection so it can be scoped to the checked tracks. `presetFor`
   *  resolves the per-language track-name preset (passed in so the store
   *  doesn't depend on the UI's language lookups). */
  applyAutomationToFile: (
    file: string,
    automation: ConfigAutomation,
    presetFor: (trackType: string, language: string) => string | undefined,
  ) => void;
  setFileOutputDir: (file: string, dir: string) => void;
  clearFileOutputDir: (file: string) => void;
  setGroupOutputDir: (files: string[], dir: string) => void;
  clearGroupOutputDir: (files: string[]) => void;
  /** Set the session-only global default output dir for new files. */
  setGlobalOutputDir: (dir: string) => void;
  setBetterMediaInfoAvailable: (value: boolean) => void;
  setActiveCard: (id: string | null) => void;
  showNotification: (kind: NotificationKind, file: string, detail: string) => void;
  dismissNotification: () => void;
}

export const useMkvStore = create<MkvStore>((set, get) => ({
  files: [],
  activeTab: "fileList",
  showSettings: false,
  showAbout: false,
  about: null,
  config: null,
  queueItems: {},
  queueOrder: [],
  fileMetadata: {},
  fileTracks: {},
  fileTrackCounts: {},
  fileSelectedIds: {},
  fileOutputDirs: {},
  globalOutputDir: undefined,
  betterMediaInfoAvailable: false,
  activeCard: null,
  notification: null,
  addFiles: (paths) =>
    set((state) => {
      const existing = new Set(state.files);
      const toAdd = paths.filter((p) => !existing.has(p));
      return { files: [...state.files, ...toAdd] };
    }),
  removeFile: (path) =>
    set((state) => {
      const nextMetadata = { ...state.fileMetadata };
      delete nextMetadata[path];
      const nextTracks = { ...state.fileTracks };
      delete nextTracks[path];
      const nextCounts = { ...state.fileTrackCounts };
      delete nextCounts[path];
      const nextSelected = { ...state.fileSelectedIds };
      delete nextSelected[path];
      const nextOutputDirs = { ...state.fileOutputDirs };
      delete nextOutputDirs[path];
      const nextQueueItems = { ...state.queueItems };
      delete nextQueueItems[path];
      return {
        files: state.files.filter((f) => f !== path),
        fileMetadata: nextMetadata,
        fileTracks: nextTracks,
        fileTrackCounts: nextCounts,
        fileSelectedIds: nextSelected,
        fileOutputDirs: nextOutputDirs,
        queueItems: nextQueueItems,
        queueOrder: state.queueOrder.filter((f) => f !== path),
        activeCard: state.activeCard === path ? null : state.activeCard,
      };
    }),
  clearFiles: () =>
    set({
      files: [],
      fileMetadata: {},
      fileTracks: {},
      fileTrackCounts: {},
      fileSelectedIds: {},
      fileOutputDirs: {},
      queueItems: {},
      queueOrder: [],
      activeCard: null,
    }),
  setActiveTab: (type) => set({ activeTab: type }),
  openSettings: () => set({ showSettings: true, activeTab: "settings" }),
  openAbout: () => set({ showAbout: true, activeTab: "about" }),
  closeSettings: () =>
    set((state) => ({
      showSettings: false,
      activeTab: state.activeTab === "settings" ? "fileList" : state.activeTab,
    })),
  closeAbout: () =>
    set((state) => ({
      showAbout: false,
      activeTab: state.activeTab === "about" ? "fileList" : state.activeTab,
    })),
  initAbout: async () => {
    try {
      const about = await getAbout();
      set({ about });
    } catch (err) {
      console.error("Failed to load about info", err);
    }
  },
  initConfig: async () => {
    try {
      const config = await getConfig();
      set({ config });
    } catch (err) {
      console.error("Failed to load config", err);
    }
  },
  updateConfig: async (patch) => {
    const current = get().config;
    if (!current) {
      return;
    }
    const next = { ...current, ...patch };
    set({ config: next });
    try {
      await setConfig(next);
    } catch (err) {
      console.error("Failed to save config", err);
    }
  },
  updateActiveProfile: async (patch) => {
    const current = get().config;
    if (!current) {
      return;
    }
    const profiles = current.profiles.map((p) =>
      p.name === current.activeProfile ? { ...p, ...patch } : p,
    );
    await get().updateConfig({ profiles });
  },
  addProfile: async (name) => {
    const trimmed = name.trim();
    if (!trimmed) {
      return;
    }
    const current = get().config;
    if (!current) {
      return;
    }
    if (current.profiles.some((p) => p.name === trimmed)) {
      return;
    }
    const fresh = createDefaultProfile(trimmed);
    await get().updateConfig({
      profiles: [...current.profiles, fresh],
      activeProfile: trimmed,
    });
  },
  deleteActiveProfile: async () => {
    const current = get().config;
    if (!current) {
      return;
    }
    if (current.activeProfile === DEFAULT_PROFILE_NAME) {
      return;
    }
    const profiles = current.profiles.filter(
      (p) => p.name !== current.activeProfile,
    );
    await get().updateConfig({
      profiles,
      activeProfile: DEFAULT_PROFILE_NAME,
    });
  },
  setActiveProfile: async (name) => {
    const current = get().config;
    if (!current) {
      return;
    }
    const target = current.profiles.find((p) => p.name === name);
    if (!target) {
      return;
    }
    await get().updateConfig({ activeProfile: name });
  },
  resetActiveProfileTemplates: async () => {
    const current = get().config;
    if (!current) {
      return;
    }
    const fresh = createDefaultProfile(current.activeProfile);
    const profiles = current.profiles.map((p) =>
      p.name === current.activeProfile ? fresh : p,
    );
    await get().updateConfig({ profiles });
  },
  applyMergeSnapshot: (entries) => {
    const now = Date.now();
    const snap = new Map(entries.map((e) => [e.file, e]));
    const prev = get().queueItems;
    const prevOrder = get().queueOrder;
    const nextItems: Record<string, QueueItem> = { ...prev };
    const nextOrder = [...prevOrder];

    for (const entry of entries) {
      const existing = nextItems[entry.file];
      if (!existing) {
        nextItems[entry.file] = {
          file: entry.file,
          drive: getDriveKey(entry.file),
          status: entry.status,
          progress: entry.progress,
          mergeStartedAt:
            entry.status === QueueItemStatus.Merging ? now : null,
          mergeEndedAt: null,
          cancelRequested: false,
          error: null,
        };
        nextOrder.push(entry.file);
      } else {
        const wasTerminal = isTerminalStatus(existing.status);
        let startedAt = wasTerminal ? null : existing.mergeStartedAt;
        const endedAt = wasTerminal ? null : existing.mergeEndedAt;
        const cancelRequested = wasTerminal ? false : existing.cancelRequested;
        const error = wasTerminal ? null : existing.error;
        if (entry.status === QueueItemStatus.Merging && startedAt === null) {
          startedAt = now;
        }
        nextItems[entry.file] = {
          ...existing,
          status: entry.status,
          progress: entry.progress,
          mergeStartedAt: startedAt,
          mergeEndedAt: endedAt,
          cancelRequested,
          error,
        };
      }
    }

    for (const file of Object.keys(nextItems)) {
      const item = nextItems[file];
      if (!isTerminalStatus(item.status) && !snap.has(file)) {
        const fallback = item.cancelRequested
          ? QueueItemStatus.Cancelled
          : QueueItemStatus.Completed;
        nextItems[file] = {
          ...item,
          status: fallback,
          mergeEndedAt: item.mergeEndedAt ?? now,
          progress:
            fallback === QueueItemStatus.Completed ? 100 : item.progress,
        };
      }
    }

    set({ queueItems: nextItems, queueOrder: nextOrder });
  },
  addToQueue: (file) => {
    const items = get().queueItems;
    const existing = items[file];
    if (
      existing &&
      (existing.status === QueueItemStatus.Waiting ||
        existing.status === QueueItemStatus.Merging)
    ) {
      return;
    }
    const fresh: QueueItem = {
      file,
      drive: getDriveKey(file),
      status: QueueItemStatus.Waiting,
      progress: 0,
      mergeStartedAt: null,
      mergeEndedAt: null,
      cancelRequested: false,
      error: null,
    };
    if (existing) {
      set({ queueItems: { ...items, [file]: fresh } });
    } else {
      set({
        queueItems: { ...items, [file]: fresh },
        queueOrder: [...get().queueOrder, file],
      });
    }
  },
  removeFromQueue: (file) =>
    set((state) => {
      if (!state.queueItems[file]) {
        return {};
      }
      const nextItems = { ...state.queueItems };
      delete nextItems[file];
      return {
        queueItems: nextItems,
        queueOrder: state.queueOrder.filter((f) => f !== file),
      };
    }),
  markCancelRequested: (file) =>
    set((state) => {
      const existing = state.queueItems[file];
      if (!existing) {
        return {};
      }
      if (isTerminalStatus(existing.status)) {
        return {};
      }
      return {
        queueItems: {
          ...state.queueItems,
          [file]: { ...existing, cancelRequested: true },
        },
      };
    }),
  recordFinishedOutcome: (file, outcome, error) =>
    set((state) => {
      const existing = state.queueItems[file];
      const now = Date.now();
      if (!existing) {
        return {
          queueItems: {
            ...state.queueItems,
            [file]: {
              file,
              drive: getDriveKey(file),
              status: outcome,
              progress: outcome === QueueItemStatus.Completed ? 100 : 0,
              mergeStartedAt: null,
              mergeEndedAt: now,
              cancelRequested: false,
              error,
            },
          },
          queueOrder: [...state.queueOrder, file],
        };
      }
      return {
        queueItems: {
          ...state.queueItems,
          [file]: {
            ...existing,
            status: outcome,
            mergeEndedAt: existing.mergeEndedAt ?? now,
            progress:
              outcome === QueueItemStatus.Completed ? 100 : existing.progress,
            error,
          },
        },
      };
    }),
  clearCompletedInDrive: (drive) =>
    set((state) => {
      const nextItems: Record<string, QueueItem> = { ...state.queueItems };
      const nextOrder: string[] = [];
      for (const file of state.queueOrder) {
        const item = nextItems[file];
        if (!item) {
          continue;
        }
        if (item.drive === drive && isTerminalStatus(item.status)) {
          delete nextItems[file];
        } else {
          nextOrder.push(file);
        }
      }
      return { queueItems: nextItems, queueOrder: nextOrder };
    }),
  setFileMetadata: (file, metadata) =>
    set((state) => {
      const tracks = metadataToMediaTracks(metadata);
      const counts = mediaTrackCounts(tracks);
      return {
        fileMetadata: { ...state.fileMetadata, [file]: metadata },
        fileTracks: { ...state.fileTracks, [file]: tracks },
        fileTrackCounts: { ...state.fileTrackCounts, [file]: counts },
      };
    }),
  setFileTrackCounts: (file, counts) =>
    set((state) => ({
      fileTrackCounts: { ...state.fileTrackCounts, [file]: counts },
    })),
  setFileSelectedIds: (file, ids) =>
    set((state) => ({
      fileSelectedIds: { ...state.fileSelectedIds, [file]: ids },
    })),
  setTrackFlag: (files, keys, kind, value) =>
    set((state) => {
      if (keys.length === 0) {
        return {};
      }
      const field = kind === "default" ? "defaultTrack" : "forced";
      const keySet = new Set(keys);
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        fileTracks[file] = list.map((t) =>
          keySet.has(trackKey(t)) ? { ...t, [field]: value } : t,
        );
      }
      return { fileTracks };
    }),
  reorderTracks: (files, fromKey, toKey) =>
    set((state) => {
      if (fromKey === toKey) {
        return {};
      }
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        const from = list.findIndex((t) => trackKey(t) === fromKey);
        const to = list.findIndex((t) => trackKey(t) === toKey);
        if (from < 0 || to < 0 || from === to) {
          continue;
        }
        const next = list.slice();
        const [moved] = next.splice(from, 1);
        next.splice(to, 0, moved);
        fileTracks[file] = next;
      }
      return { fileTracks };
    }),
  setTrackLanguage: (files, keys, value) =>
    set((state) => {
      const keySet = new Set(keys);
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        fileTracks[file] = list.map((t) =>
          keySet.has(trackKey(t)) ? { ...t, language: value } : t,
        );
      }
      return { fileTracks };
    }),
  setTrackName: (files, keys, value) =>
    set((state) => {
      const keySet = new Set(keys);
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        fileTracks[file] = list.map((t) =>
          keySet.has(trackKey(t)) ? { ...t, trackName: value } : t,
        );
      }
      return { fileTracks };
    }),
  applyAutomationToFile: (file, automation, presetFor) =>
    set((state) => {
      const list = state.fileTracks[file];
      if (!list) {
        return {};
      }
      const resetUnd = automation.reset_und_language;
      const setName = automation.set_track_name.enabled;
      if (!resetUnd.enabled && !setName) {
        return {};
      }
      // Per-track language then name (name sees the updated language). The
      // default/forced steps live in `applyFlagAutomationToFile`, run after
      // auto-selection so they can be scoped to the checked tracks.
      const next = list.map((track): MediaTrack => {
        if (track.kind !== "track") {
          return track;
        }
        let updated = track;
        if (resetUnd.enabled && resetUnd.language && updated.language === "und") {
          updated = { ...updated, language: resetUnd.language };
        }
        if (setName) {
          const preset = presetFor(updated.type, updated.language);
          if (preset) {
            updated = { ...updated, trackName: preset };
          }
        }
        return updated;
      });
      return { fileTracks: { ...state.fileTracks, [file]: next } };
    }),
  setFileOutputDir: (file, dir) =>
    set((state) => ({
      fileOutputDirs: { ...state.fileOutputDirs, [file]: dir },
    })),
  clearFileOutputDir: (file) =>
    set((state) => {
      const next = { ...state.fileOutputDirs };
      delete next[file];
      return { fileOutputDirs: next };
    }),
  setGroupOutputDir: (files, dir) =>
    set((state) => {
      const next = { ...state.fileOutputDirs };
      for (const f of files) {
        next[f] = dir;
      }
      return { fileOutputDirs: next };
    }),
  clearGroupOutputDir: (files) =>
    set((state) => {
      const next = { ...state.fileOutputDirs };
      for (const f of files) {
        delete next[f];
      }
      return { fileOutputDirs: next };
    }),
  setGlobalOutputDir: (dir) =>
    set({ globalOutputDir: dir.length > 0 ? dir : undefined }),
  setBetterMediaInfoAvailable: (value) =>
    set({ betterMediaInfoAvailable: value }),
  setActiveCard: (id) => set({ activeCard: id }),
  showNotification: (kind, file, detail) =>
    set((state) => ({
      notification: {
        id: (state.notification?.id ?? 0) + 1,
        kind,
        file,
        detail,
      },
    })),
  dismissNotification: () => set({ notification: null }),
}));
