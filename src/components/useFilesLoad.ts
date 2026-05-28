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

import { useEffect, useRef, useState } from "react";
import { makeTrackSelector, trackKey } from "../merge";
import { formatMetadataError } from "../metadataError";
import { getMediaMetadata } from "../service";
import { useMkvStore } from "../store";
import { buildTrackNameOptions } from "./TrackCellAutocomplete";

type TranslateFn = (
  key: string,
  options?: Record<string, string | number>,
) => string;

/**
 * Ensure every file in `files` is parsed, auto-selected (once), and run through
 * the active profile's automation — then report aggregate load state. A card
 * renders only its tree's root, but the whole tree's member files must be
 * loaded so the combined table and merge are complete; this hook drives that
 * for all members at once (the single-file card passes a one-element list).
 *
 * Loading lives here rather than per rendered card because, with *Group by file
 * name*, child files are never rendered on their own — only as members of a
 * root's unit. Each file belongs to exactly one card, so there is no
 * double-loading across cards.
 */
export function useFilesLoad(
  files: string[],
  t: TranslateFn,
): { loading: boolean; error: string | null } {
  const setFileMetadata = useMkvStore((s) => s.setFileMetadata);
  const applyAutomationToFile = useMkvStore((s) => s.applyAutomationToFile);
  const setFileSelectedIds = useMkvStore((s) => s.setFileSelectedIds);
  const fileTracksMap = useMkvStore((s) => s.fileTracks);
  const fileSelectedIdsMap = useMkvStore((s) => s.fileSelectedIds);
  const activeProfile = useMkvStore((s) => {
    const cfg = s.config;
    if (!cfg) {
      return null;
    }
    return (
      cfg.profiles.find((p) => p.name === cfg.activeProfile) ??
      cfg.profiles[0] ??
      null
    );
  });
  const [errors, setErrors] = useState<Record<string, string>>({});
  const inFlight = useRef<Set<string>>(new Set());

  // Parse any not-yet-loaded file, then apply the active profile's automation
  // once. Guarded by an in-flight set + the store's presence check so a
  // re-render (new `files` identity) never relaunches an in-progress load. The
  // resolved result is written to the global store unconditionally — there is no
  // cancellation flag, because (a) the write target is the store, not component
  // state, so it's safe after unmount, and (b) under React StrictMode's
  // mount→unmount→remount the in-flight guard makes the remount reuse the first
  // fetch; cancelling it would drop the result and the table would never load.
  useEffect(() => {
    for (const file of files) {
      if (useMkvStore.getState().fileTracks[file] !== undefined) {
        continue;
      }
      if (inFlight.current.has(file) || errors[file]) {
        continue;
      }
      inFlight.current.add(file);
      getMediaMetadata(file)
        .then((metadata) => {
          inFlight.current.delete(file);
          setFileMetadata(file, metadata);
          const cfg = useMkvStore.getState().config;
          const profile = cfg
            ? cfg.profiles.find((p) => p.name === cfg.activeProfile) ??
              cfg.profiles[0] ??
              null
            : null;
          const automation = profile?.automation;
          if (
            profile &&
            automation &&
            (automation.reset_und_language.enabled ||
              automation.set_track_name.enabled ||
              automation.reset_default_track.enabled ||
              automation.reset_forced_display.enabled)
          ) {
            applyAutomationToFile(file, automation, (type, language) =>
              buildTrackNameOptions(profile, type, language)[0],
            );
          }
        })
        .catch((err: unknown) => {
          inFlight.current.delete(file);
          setErrors((prev) => ({ ...prev, [file]: formatMetadataError(err, t) }));
        });
    }
  }, [files, t, setFileMetadata, applyAutomationToFile, errors]);

  // Auto-select tracks once per file (only while its selection is still unset).
  useEffect(() => {
    if (!activeProfile) {
      return;
    }
    const selectTrack = makeTrackSelector(activeProfile);
    for (const file of files) {
      if (fileSelectedIdsMap[file] !== undefined) {
        continue;
      }
      const tracks = fileTracksMap[file];
      if (!tracks || tracks.length === 0) {
        continue;
      }
      const auto: string[] = [];
      for (const track of tracks) {
        if (selectTrack(track)) {
          auto.push(trackKey(track));
        }
      }
      setFileSelectedIds(file, auto);
    }
  }, [
    files,
    activeProfile,
    fileTracksMap,
    fileSelectedIdsMap,
    setFileSelectedIds,
  ]);

  const loading = files.some(
    (f) => fileTracksMap[f] === undefined && !errors[f],
  );
  const error = files.map((f) => errors[f]).find((e) => e) ?? null;
  return { loading, error };
}
