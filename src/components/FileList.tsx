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

import { useEffect, useMemo } from "react";
import { Box } from "@mui/material";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useTranslation } from "react-i18next";
import { formatHMS, getParentDir } from "../merge";
import { buildMergeUnits, combineUnitTracks } from "../file-tree";
import { mediaTrackCounts } from "../media-metadata";
import type { MediaTrack } from "../media-metadata";
import type { MergeFinishedEvent } from "../protocol";
import { GroupMode, QueueItemStatus } from "../protocol";
import { getMergeStatus, getMediaFiles } from "../service";
import { useMkvStore } from "../store";
import { GroupCard } from "./GroupCard";
import { MkvFileCard } from "./MkvFileCard";
import Welcome from "./Welcome";

type RenderEntry =
  | { kind: "single"; members: string[] }
  | { kind: "group"; key: string; units: string[][] };

const MERGE_POLL_INTERVAL_MS = 200;

/** Sorted, comma-joined language list for one track type — used so two files
 *  group together only when their per-type languages match (mode 3). */
function languageSignature(tracks: MediaTrack[], type: string): string {
  return tracks
    .filter((t) => t.type === type)
    .map((t) => t.language)
    .sort()
    .join(",");
}

export default function FileList() {
  const { t } = useTranslation();
  const files = useMkvStore((s) => s.files);
  const addFiles = useMkvStore((s) => s.addFiles);
  const applyMergeSnapshot = useMkvStore((s) => s.applyMergeSnapshot);
  const recordFinishedOutcome = useMkvStore((s) => s.recordFinishedOutcome);
  const showNotification = useMkvStore((s) => s.showNotification);
  const groupMode = useMkvStore(
    (s) => s.config?.groupMode ?? GroupMode.TrackCount,
  );
  const groupByFileName = useMkvStore(
    (s) => s.config?.groupByFileName ?? true,
  );
  const fileTrackCounts = useMkvStore((s) => s.fileTrackCounts);
  const fileTracks = useMkvStore((s) => s.fileTracks);
  const mergedRoots = useMkvStore((s) => s.mergedRoots);
  const detachedFiles = useMkvStore((s) => s.detachedFiles);

  const entries = useMemo<RenderEntry[]>(() => {
    // The atomic unit is a merge tree: when grouping by file name it's a forest
    // root plus its children; otherwise every file is its own one-member unit.
    // Manual drag-merges are folded in, then explicitly detached files pulled out.
    const units = buildMergeUnits(
      files,
      groupByFileName,
      mergedRoots,
      detachedFiles,
    );

    if (groupMode === GroupMode.None) {
      return units.map((members) => ({ kind: "single", members }));
    }

    // Track-count grouping is layered on top of the units: a unit's combined
    // (flattened) tracks drive its grouping key. A unit whose members aren't all
    // parsed yet has no key and stays a single card until they load.
    const buckets = new Map<string, string[][]>();
    const bucketOrder: string[] = [];
    const ungroupable: string[][] = [];
    for (const members of units) {
      const allLoaded = members.every(
        (file) => fileTrackCounts[file] !== undefined,
      );
      if (!allLoaded) {
        ungroupable.push(members);
        continue;
      }
      const combined = combineUnitTracks(members, fileTracks);
      const counts = mediaTrackCounts(combined);
      const dir = getParentDir(members[0]);
      // Member count keeps differently-shaped trees apart so the group's
      // per-position batch edits line up across units.
      let key = `${dir}|n=${members.length}`;
      if (groupMode === GroupMode.TrackCountAndLanguage) {
        key += `|v=${languageSignature(combined, "video")}|a=${languageSignature(combined, "audio")}|s=${languageSignature(combined, "subtitles")}|c=${counts.chapters}|t=${counts.attachments}`;
      } else {
        key += `|v=${counts.video}|a=${counts.audio}|s=${counts.subtitles}|c=${counts.chapters}|t=${counts.attachments}`;
      }
      let bucket = buckets.get(key);
      if (!bucket) {
        bucket = [];
        buckets.set(key, bucket);
        bucketOrder.push(key);
      }
      bucket.push(members);
    }
    const result: RenderEntry[] = [];
    for (const key of bucketOrder) {
      const groupUnits = buckets.get(key) ?? [];
      if (groupUnits.length >= 2) {
        result.push({ kind: "group", key, units: groupUnits });
      } else {
        for (const members of groupUnits) {
          result.push({ kind: "single", members });
        }
      }
    }
    for (const members of ungroupable) {
      result.push({ kind: "single", members });
    }
    return result;
  }, [
    files,
    groupByFileName,
    groupMode,
    fileTrackCounts,
    fileTracks,
    mergedRoots,
    detachedFiles,
  ]);

  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        const snap = await getMergeStatus();
        if (!cancelled) {
          applyMergeSnapshot(snap.entries);
        }
      } catch (err) {
        if (!cancelled) {
          console.error("Failed to fetch merge status", err);
        }
      }
    };
    poll();
    const id = setInterval(poll, MERGE_POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [applyMergeSnapshot]);

  useEffect(() => {
    const unlistenPromise = listen<MergeFinishedEvent>(
      "merge-finished",
      (event) => {
        const { file, outcome, error } = event.payload;
        const existing = useMkvStore.getState().queueItems[file];
        const startedAt = existing?.mergeStartedAt ?? null;
        recordFinishedOutcome(file, outcome, error);
        if (outcome === QueueItemStatus.Completed) {
          const elapsedMs = startedAt !== null ? Date.now() - startedAt : 0;
          showNotification(
            "success",
            file,
            t("notification.completedIn", { elapsed: formatHMS(elapsedMs) }),
          );
        } else if (outcome === QueueItemStatus.Failed) {
          showNotification(
            "error",
            file,
            error ?? t("notification.unknownError"),
          );
        }
      },
    );
    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [recordFinishedOutcome, showNotification, t]);

  useEffect(() => {
    const unlistenPromise = getCurrentWebviewWindow().onDragDropEvent(
      async (event) => {
        if (event.payload.type !== "drop") {
          return;
        }
        const paths = event.payload.paths;
        if (!paths || paths.length === 0) {
          return;
        }
        try {
          const mediaFiles = await getMediaFiles(paths);
          if (mediaFiles.length > 0) {
            addFiles(mediaFiles);
          }
        } catch (err) {
          console.error("Failed to resolve dropped paths", err);
        }
      },
    );
    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, [addFiles]);

  if (files.length === 0) {
    return <Welcome />;
  }

  return (
    <Box>
      {entries.map((entry) =>
        entry.kind === "single" ? (
          <MkvFileCard key={entry.members[0]} memberFiles={entry.members} />
        ) : (
          <GroupCard key={entry.key} units={entry.units} />
        ),
      )}
    </Box>
  );
}
