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

import { useEffect, useMemo, useRef, useState } from "react";
import { applyUnitFlagAutomation } from "../file-tree";
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
 * Ensure every file in `units` is parsed and run through the active profile's
 * automation pipeline — then report aggregate load state. The pipeline runs in
 * this order: language, track name, default/forced flags, track auto-selection.
 * The future selection is calculated before the flag steps so those steps stay
 * scoped to the tracks that will be checked, but selected IDs are committed
 * only after every automation step has completed.
 *
 * Loading lives here rather than per rendered card because, with *Group by file
 * name*, child files are never rendered on their own — only as members of a
 * root's unit. Each file belongs to exactly one card, so there is no
 * double-loading across cards.
 */
export function useFilesLoad(
  units: string[][],
  t: TranslateFn,
  unitTrackOrder?: string[],
): { loading: boolean; error: string | null } {
  const files = useMemo(() => units.flat(), [units]);
  const setFileMetadata = useMkvStore((s) => s.setFileMetadata);
  const applyAutomationToFile = useMkvStore((s) => s.applyAutomationToFile);
  const setFileSelectedIds = useMkvStore((s) => s.setFileSelectedIds);
  const setTrackFlag = useMkvStore((s) => s.setTrackFlag);
  const fileTracksMap = useMkvStore((s) => s.fileTracks);
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
  const automatedUnits = useRef<Set<string>>(new Set());
  const inFlight = useRef<Set<string>>(new Set());

  // Parse any not-yet-loaded file. Guarded by an in-flight set + the store's
  // presence check so a re-render never relaunches an in-progress load. The
  // resolved result is written to the global store unconditionally — there is
  // no cancellation flag, because (a) the write target is the store, not
  // component state, so it's safe after unmount, and (b) under React
  // StrictMode's mount→unmount→remount the in-flight guard makes the remount
  // reuse the first fetch; cancelling it would drop the result and the table
  // would never load.
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
        })
        .catch((err: unknown) => {
          inFlight.current.delete(file);
          setErrors((prev) => ({ ...prev, [file]: formatMetadataError(err, t) }));
        });
    }
  }, [files, t, setFileMetadata, errors]);

  // Run the complete automation pipeline once a whole merge unit is loaded.
  // For a newly-loaded file, calculate its future selected IDs first, use that
  // projection to scope default/forced automation, and commit the IDs last.
  // Existing selections are never replaced. A new unit signature (for example
  // after a manual card merge) re-runs only the unit-level flag steps.
  useEffect(() => {
    if (!activeProfile) {
      return;
    }
    const selectTrack = makeTrackSelector(activeProfile);
    for (const unit of units) {
      const stateBeforeAutomation = useMkvStore.getState();
      if (
        !unit.every(
          (file) => stateBeforeAutomation.fileTracks[file] !== undefined,
        )
      ) {
        continue;
      }

      const signature = JSON.stringify(unit);
      const pendingFiles = unit.filter(
        (file) => stateBeforeAutomation.fileSelectedIds[file] === undefined,
      );
      if (
        pendingFiles.length === 0 &&
        automatedUnits.current.has(signature)
      ) {
        continue;
      }
      automatedUnits.current.add(signature);

      for (const file of pendingFiles) {
        applyAutomationToFile(
          file,
          activeProfile.automation,
          (type, language) =>
            buildTrackNameOptions(activeProfile, type, language)[0],
        );
      }

      const stateAfterTrackAutomation = useMkvStore.getState();
      const prospectiveSelections = {
        ...stateAfterTrackAutomation.fileSelectedIds,
      };
      for (const file of pendingFiles) {
        prospectiveSelections[file] = (
          stateAfterTrackAutomation.fileTracks[file] ?? []
        )
          .filter(selectTrack)
          .map(trackKey);
      }

      applyUnitFlagAutomation(
        unit,
        stateAfterTrackAutomation.fileTracks,
        prospectiveSelections,
        {
          resetDefault:
            activeProfile.automation.reset_default_track.enabled,
          resetForced:
            activeProfile.automation.reset_forced_display.enabled,
        },
        setTrackFlag,
        units.length === 1 ? unitTrackOrder : undefined,
      );

      for (const file of pendingFiles) {
        const selectedIds = prospectiveSelections[file];
        if (selectedIds) {
          setFileSelectedIds(file, selectedIds);
        }
      }
    }
  }, [
    units,
    activeProfile,
    fileTracksMap,
    applyAutomationToFile,
    setFileSelectedIds,
    setTrackFlag,
    unitTrackOrder,
  ]);

  const loading = files.some(
    (f) => fileTracksMap[f] === undefined && !errors[f],
  );
  const error = files.map((f) => errors[f]).find((e) => e) ?? null;
  return { loading, error };
}
