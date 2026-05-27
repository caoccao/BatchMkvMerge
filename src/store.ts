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
  ConfigProfile,
  MergeEntry,
  MergeOutcome,
  MediaMetadata,
  TrackFlag,
} from "./protocol";

/** Which track flag a `cycleTrackFlag` call targets. */
export type TrackFlagKind = "default" | "forced";

/** Cycle a tri-state flag: checked → unchecked → unspecified → checked. */
function nextTrackFlag(flag: TrackFlag): TrackFlag {
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
  groupByFile: boolean;
  betterMediaInfoAvailable: boolean;
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
  setGroupSelectedIds: (files: string[], ids: string[]) => void;
  /** Cycle the default/forced flag (true → false → unspecified → true) on the
   *  matching track across all given files. */
  cycleTrackFlag: (files: string[], key: string, kind: TrackFlagKind) => void;
  /** Header action: set the first video/audio/subtitle track's default flag to
   *  true and every other track's to false. */
  setDefaultTrackByType: (files: string[]) => void;
  /** Header action: reset the forced flag to unspecified on every track. */
  clearForcedFlags: (files: string[]) => void;
  /** Drag-reorder: move the `fromKey` row to `toKey`'s position in every given
   *  file that contains both rows. The track `id` is intrinsic and unchanged —
   *  only the row order changes. */
  reorderTracks: (files: string[], fromKey: string, toKey: string) => void;
  setFileOutputDir: (file: string, dir: string) => void;
  clearFileOutputDir: (file: string) => void;
  setGroupOutputDir: (files: string[], dir: string) => void;
  clearGroupOutputDir: (files: string[]) => void;
  setGroupByFile: (value: boolean) => void;
  setBetterMediaInfoAvailable: (value: boolean) => void;
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
  groupByFile: false,
  betterMediaInfoAvailable: false,
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
      return {
        files: state.files.filter((f) => f !== path),
        fileMetadata: nextMetadata,
        fileTracks: nextTracks,
        fileTrackCounts: nextCounts,
        fileSelectedIds: nextSelected,
        fileOutputDirs: nextOutputDirs,
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
      const active =
        config.profiles.find((p) => p.name === config.activeProfile) ??
        config.profiles[0];
      const groupByFile = active?.defaultGroupMode ?? false;
      set({ config, groupByFile });
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
    if (patch.defaultGroupMode !== undefined) {
      set({ groupByFile: patch.defaultGroupMode });
    }
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
    set({ groupByFile: fresh.defaultGroupMode });
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
    const fallback = profiles.find((p) => p.name === DEFAULT_PROFILE_NAME);
    set({ groupByFile: fallback?.defaultGroupMode ?? false });
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
    set({ groupByFile: target.defaultGroupMode });
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
    set({ groupByFile: fresh.defaultGroupMode });
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
  setGroupSelectedIds: (files, ids) =>
    set((state) => {
      const next = { ...state.fileSelectedIds };
      for (const f of files) {
        next[f] = ids;
      }
      return { fileSelectedIds: next };
    }),
  cycleTrackFlag: (files, key, kind) =>
    set((state) => {
      const field = kind === "default" ? "defaultTrack" : "forced";
      const ref = (state.fileTracks[files[0]] ?? []).find(
        (t) => trackKey(t) === key,
      );
      if (!ref) {
        return {};
      }
      const value = nextTrackFlag(ref[field]);
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        fileTracks[file] = list.map((t) =>
          trackKey(t) === key ? { ...t, [field]: value } : t,
        );
      }
      return { fileTracks };
    }),
  setDefaultTrackByType: (files) =>
    set((state) => {
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        const claimed = new Set<string>();
        fileTracks[file] = list.map((t): MediaTrack => {
          if (t.kind !== "track") {
            return t;
          }
          const isPrimaryType =
            t.type === "video" || t.type === "audio" || t.type === "subtitles";
          if (isPrimaryType && !claimed.has(t.type)) {
            claimed.add(t.type);
            return { ...t, defaultTrack: "true" };
          }
          return { ...t, defaultTrack: "false" };
        });
      }
      return { fileTracks };
    }),
  clearForcedFlags: (files) =>
    set((state) => {
      const fileTracks = { ...state.fileTracks };
      for (const file of files) {
        const list = fileTracks[file];
        if (!list) {
          continue;
        }
        fileTracks[file] = list.map((t): MediaTrack =>
          t.kind === "track" ? { ...t, forced: "unspecified" } : t,
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
  setGroupByFile: (value) => set({ groupByFile: value }),
  setBetterMediaInfoAvailable: (value) =>
    set({ betterMediaInfoAvailable: value }),
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
