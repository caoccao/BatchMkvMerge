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

import { getFileName, getParentDir } from "./merge";
import { trackTypeRank } from "./media-metadata";
import type { MediaTrack } from "./media-metadata";
import type { TrackFlag } from "./protocol";

/**
 * One "merge tree" produced by the *Group by file name* feature. The whole tree
 * merges into a single output file named after `root`.
 *
 * `members` is the ordered list of source files: `members[0]` is the root, the
 * rest are its children sorted by stem ascending (so the tree is stable). The
 * tree is "flat under shortest" — the smallest stem in a directory is the root
 * and every other file whose stem contains it is a direct child (no deeper
 * nesting), so `childCount === members.length - 1`.
 */
export interface FileTree {
  root: string;
  members: string[];
  childCount: number;
}

/**
 * A combined track row for a multi-file unit: a parsed [`MediaTrack`] annotated
 * with the source file it came from and that file's index within the unit's
 * `members` (0 = root). The selection table renders these; edits map back to
 * `sourceFile` + the bare `${type}:${id}` key via [`parseRowKey`].
 */
export interface CombinedTrack extends MediaTrack {
  sourceFile: string;
  memberIndex: number;
}

/**
 * The file name without its final extension (basename only). Only the last
 * extension is stripped, so `Movie.en.srt` → `Movie.en` and `Movie.mkv` →
 * `Movie`. A leading dot (dotfile) is treated as "no extension".
 */
export function stemOf(path: string): string {
  const name = getFileName(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

/** Sort comparator: stem ascending, then full path ascending (stable tie-break). */
function byStemThenPath(
  a: string,
  b: string,
  stem: Map<string, string>,
): number {
  const sa = stem.get(a) ?? "";
  const sb = stem.get(b) ?? "";
  if (sa !== sb) {
    return sa < sb ? -1 : 1;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

/**
 * Build the merge-tree forest for *Group by file name*. Files are grouped by
 * directory; within each directory, a file's stem being a (case-sensitive)
 * substring of another's makes it the parent. The algorithm:
 *
 *   1. Sort the directory's files by stem length ascending (smallest first),
 *      then stem, then path.
 *   2. Walk that order. The first unclaimed file becomes a root and claims
 *      every still-unclaimed file whose stem contains the root stem as a direct
 *      child; claimed files are removed from the pool so the smallest containing
 *      stem wins and each file is assigned exactly once.
 *   3. Repeat for the next unclaimed file.
 *
 * Equal stems (e.g. `Movie.mkv` / `Movie.srt`) count as substrings, so they
 * land in the same tree. Roots are returned in the original `files` order so the
 * on-screen card order stays stable as files are added.
 */
export function buildForest(files: string[]): FileTree[] {
  const stem = new Map<string, string>();
  for (const f of files) {
    stem.set(f, stemOf(f));
  }

  const byDir = new Map<string, string[]>();
  for (const f of files) {
    const dir = getParentDir(f);
    const arr = byDir.get(dir);
    if (arr) {
      arr.push(f);
    } else {
      byDir.set(dir, [f]);
    }
  }

  const parentOf = new Map<string, string | null>();
  const childrenOf = new Map<string, string[]>();
  for (const dirFiles of byDir.values()) {
    const ordered = [...dirFiles].sort((a, b) => {
      const sa = stem.get(a) ?? "";
      const sb = stem.get(b) ?? "";
      if (sa.length !== sb.length) {
        return sa.length - sb.length;
      }
      return byStemThenPath(a, b, stem);
    });
    const claimed = new Set<string>();
    for (const root of ordered) {
      if (claimed.has(root)) {
        continue;
      }
      claimed.add(root);
      const rootStem = stem.get(root) ?? "";
      const children: string[] = [];
      for (const cand of ordered) {
        if (cand === root || claimed.has(cand)) {
          continue;
        }
        if ((stem.get(cand) ?? "").includes(rootStem)) {
          claimed.add(cand);
          children.push(cand);
        }
      }
      children.sort((a, b) => byStemThenPath(a, b, stem));
      parentOf.set(root, null);
      childrenOf.set(root, children);
      for (const c of children) {
        parentOf.set(c, root);
      }
    }
  }

  const trees: FileTree[] = [];
  for (const f of files) {
    if (parentOf.get(f) === null) {
      const children = childrenOf.get(f) ?? [];
      trees.push({
        root: f,
        members: [f, ...children],
        childCount: children.length,
      });
    }
  }
  return trees;
}

/**
 * Flatten a unit's member files into one combined track list, as if the tree
 * were a single file. Rows are sorted first by track type, then by member index
 * (root before children), then by each file's own track order — a fully
 * determined, stable order (so the table never reshuffles between renders).
 */
export function combineUnitTracks(
  members: string[],
  fileTracks: Record<string, MediaTrack[]>,
): CombinedTrack[] {
  const rows: { track: CombinedTrack; within: number }[] = [];
  members.forEach((file, memberIndex) => {
    const list = fileTracks[file] ?? [];
    list.forEach((t, within) => {
      rows.push({ track: { ...t, sourceFile: file, memberIndex }, within });
    });
  });
  rows.sort((a, b) => {
    const ta = trackTypeRank(a.track.type);
    const tb = trackTypeRank(b.track.type);
    if (ta !== tb) {
      return ta - tb;
    }
    if (a.track.memberIndex !== b.track.memberIndex) {
      return a.track.memberIndex - b.track.memberIndex;
    }
    return a.within - b.within;
  });
  return rows.map((r) => r.track);
}

/** UI row key for a combined track — unique across members and stable across
 *  structurally-identical units. */
export function rowKeyOf(track: CombinedTrack): string {
  return `${track.memberIndex}:${track.type}:${track.id}`;
}

/** Split a combined row key back into its member index and the bare
 *  `${type}:${id}` key used by the per-file store maps (matches `trackKey`). */
export function parseRowKey(rowKey: string): {
  memberIndex: number;
  bareKey: string;
} {
  const firstColon = rowKey.indexOf(":");
  return {
    memberIndex: Number(rowKey.slice(0, firstColon)),
    bareKey: rowKey.slice(firstColon + 1),
  };
}

/**
 * Apply the "reset default track" / "reset forced display" automation to a
 * SINGLE merge unit (one tree), scoped to its *checked* tracks: across the
 * flattened tree the first checked video/audio/subtitle becomes the default
 * (other checked primary tracks cleared), and/or every checked track's forced
 * flag is cleared. Edits are dispatched per source file via `setTrackFlag`.
 *
 * This is per-unit by design — when several units are grouped together they may
 * have different track layouts, so each must be evaluated against its own tracks
 * rather than a shared representative.
 */
export function applyUnitFlagAutomation(
  members: string[],
  fileTracks: Record<string, MediaTrack[]>,
  fileSelectedIds: Record<string, string[]>,
  opts: { resetDefault: boolean; resetForced: boolean },
  setTrackFlag: (
    files: string[],
    keys: string[],
    kind: "default" | "forced",
    value: TrackFlag,
  ) => void,
): void {
  if (!opts.resetDefault && !opts.resetForced) {
    return;
  }
  const combined = combineUnitTracks(members, fileTracks);
  const selected = new Set<string>();
  members.forEach((file, memberIndex) => {
    for (const bareKey of fileSelectedIds[file] ?? []) {
      selected.add(`${memberIndex}:${bareKey}`);
    }
  });
  const push = (map: Map<string, string[]>, file: string, key: string) => {
    const arr = map.get(file);
    if (arr) {
      arr.push(key);
    } else {
      map.set(file, [key]);
    }
  };
  const defaultTrue = new Map<string, string[]>();
  const defaultFalse = new Map<string, string[]>();
  const forcedClear = new Map<string, string[]>();
  const claimed = new Set<string>();
  for (const row of combined) {
    if (row.kind !== "track" || !selected.has(rowKeyOf(row))) {
      continue;
    }
    const bareKey = `${row.type}:${row.id}`;
    if (opts.resetForced) {
      push(forcedClear, row.sourceFile, bareKey);
    }
    if (
      opts.resetDefault &&
      (row.type === "video" ||
        row.type === "audio" ||
        row.type === "subtitles")
    ) {
      const isFirst = !claimed.has(row.type);
      claimed.add(row.type);
      push(isFirst ? defaultTrue : defaultFalse, row.sourceFile, bareKey);
    }
  }
  for (const [file, keys] of defaultTrue) {
    setTrackFlag([file], keys, "default", "true");
  }
  for (const [file, keys] of defaultFalse) {
    setTrackFlag([file], keys, "default", "false");
  }
  for (const [file, keys] of forcedClear) {
    setTrackFlag([file], keys, "forced", "unspecified");
  }
}
