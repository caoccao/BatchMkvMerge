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
import {
  Alert,
  Badge,
  Box,
  Button,
  IconButton,
  LinearProgress,
  List,
  ListItem,
  Paper,
  Snackbar,
  Tooltip,
  Typography,
} from "@mui/material";
import CancelIcon from "@mui/icons-material/Cancel";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import HubIcon from "@mui/icons-material/Hub";
import DeleteIcon from "@mui/icons-material/Delete";
import FolderOpenIcon from "@mui/icons-material/FolderOpen";
import betterMediaInfoIcon from "../assets/bettermediainfo.png";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useTranslation } from "react-i18next";
import {
  cancelMerge,
  enqueueSelectedTracksForFile,
  enqueueSelectedTracksForUnit,
} from "../actions/mergeActions";
import {
  buildCommandString,
  buildCommandStringMulti,
  formatHMS,
  getFileName,
  getParentDir,
  resolveOutputDir,
  trackKey,
} from "../merge";
import type { MergeInput } from "../merge";
import {
  applyUnitFlagAutomation,
  combineUnitTracks,
  parseRowKey,
  rowKeyOf,
} from "../file-tree";
import type { CombinedTrack } from "../file-tree";
import { mediaTrackCounts } from "../media-metadata";
import { QueueItemStatus } from "../protocol";
import { launchBetterMediaInfo, resolveMergeOutputPath } from "../service";
import { nextTrackFlag, useMkvStore } from "../store";
import type { TrackFlagKind } from "../store";
import { CardSummary } from "./CardSummary";
import { FileStatusIcon } from "./FileStatusIcon";
import { OutputPathDialog } from "./OutputPathDialog";
import { TrackSelectionTable } from "./TrackSelectionTable";
import {
  buildLanguageOptions,
  buildTrackNameOptions,
} from "./TrackCellAutocomplete";
import { useCardRowSelection } from "./useCardRowSelection";
import { useFilesLoad } from "./useFilesLoad";

interface GroupCardProps {
  /** The structurally-identical merge units in this track-count group. Each
   *  unit is its own member-file list (root first) and merges into its own
   *  output; edits broadcast to the matching position across every unit. */
  units: string[][];
}

export function GroupCard({ units }: GroupCardProps) {
  const { t } = useTranslation();
  const [snackbar, setSnackbar] = useState<{
    message: string;
    severity: "success" | "error";
  } | null>(null);
  const [leftWidth, setLeftWidth] = useState(240);
  const [now, setNow] = useState(() => Date.now());
  const [outputDialogOpen, setOutputDialogOpen] = useState(false);
  const [outputDialogInitial, setOutputDialogInitial] = useState("");
  const splitContainerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 200);
    return () => clearInterval(id);
  }, []);

  const startResize = (e: React.MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const container = splitContainerRef.current;
    if (!container) {
      return;
    }
    const onMove = (ev: MouseEvent) => {
      const rect = container.getBoundingClientRect();
      const min = 160;
      const max = Math.max(min, rect.width - 240);
      const next = Math.max(min, Math.min(max, ev.clientX - rect.left));
      setLeftWidth(next);
    };
    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  const mkvToolNixPath = useMkvStore(
    (s) => s.config?.externalTools?.mkvToolNixPath ?? "",
  );
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
  const fileTracksMap = useMkvStore((s) => s.fileTracks);
  const fileSelectedIdsMap = useMkvStore((s) => s.fileSelectedIds);
  const fileOutputDirs = useMkvStore((s) => s.fileOutputDirs);
  const globalOutputDir = useMkvStore((s) => s.globalOutputDir);
  const betterMediaInfoAvailable = useMkvStore(
    (s) => s.betterMediaInfoAvailable,
  );
  const formatting = useMkvStore((s) => s.config?.formatting ?? null);
  const setGroupOutputDir = useMkvStore((s) => s.setGroupOutputDir);
  const clearGroupOutputDir = useMkvStore((s) => s.clearGroupOutputDir);
  const queueItems = useMkvStore((s) => s.queueItems);
  const removeFile = useMkvStore((s) => s.removeFile);
  const removeFromQueue = useMkvStore((s) => s.removeFromQueue);
  const setFileSelectedIds = useMkvStore((s) => s.setFileSelectedIds);
  const setTrackFlag = useMkvStore((s) => s.setTrackFlag);
  const reorderTracks = useMkvStore((s) => s.reorderTracks);
  const setTrackLanguage = useMkvStore((s) => s.setTrackLanguage);
  const setTrackName = useMkvStore((s) => s.setTrackName);

  const roots = useMemo(() => units.map((u) => u[0]), [units]);
  const allFiles = useMemo(() => units.flat(), [units]);
  const repMembers = units[0] ?? [];
  const reorderDisabled = units.some((u) => u.length > 1);

  const { loading, error } = useFilesLoad(allFiles, t);

  const repCombined = useMemo<CombinedTrack[]>(
    () => combineUnitTracks(repMembers, fileTracksMap),
    [repMembers, fileTracksMap],
  );
  const repCounts = useMemo(() => mediaTrackCounts(repCombined), [repCombined]);
  const allRowKeys = useMemo(() => repCombined.map(rowKeyOf), [repCombined]);
  const cardId = useMemo(() => roots.join("\n"), [roots]);

  // Representative selection lifted from the first unit's per-file selections.
  const selectedIds = useMemo(() => {
    const ids = new Set<string>();
    repMembers.forEach((file, memberIndex) => {
      for (const bareKey of fileSelectedIdsMap[file] ?? []) {
        ids.add(`${memberIndex}:${bareKey}`);
      }
    });
    return ids;
  }, [repMembers, fileSelectedIdsMap]);

  const hasSelection = selectedIds.size > 0;
  const hasActiveInGroup = roots.some((r) => {
    const status = queueItems[r]?.status;
    return (
      status === QueueItemStatus.Waiting || status === QueueItemStatus.Merging
    );
  });
  const hasWaitingInGroup = roots.some(
    (r) => queueItems[r]?.status === QueueItemStatus.Waiting,
  );
  const canMergeAll = hasSelection && !hasActiveInGroup;
  const canCopyAll = hasSelection;
  const canClearAll = units.length > 0 && !hasActiveInGroup;

  const parentDir = roots[0] ? getParentDir(roots[0]) : "";

  const groupOutputDir = useMemo(() => {
    const first = fileOutputDirs[roots[0]];
    if (!first || first.length === 0) {
      return undefined;
    }
    for (let i = 1; i < roots.length; i += 1) {
      if (fileOutputDirs[roots[i]] !== first) {
        return undefined;
      }
    }
    return first;
  }, [roots, fileOutputDirs]);

  // All members at the same position across every unit — an edit to one
  // combined row fans out to that position in all structurally-identical units.
  const filesForMember = (memberIndex: number): string[] =>
    units
      .map((u) => u[memberIndex])
      .filter((f): f is string => Boolean(f));

  const {
    cardActive,
    activate,
    selectedRowKeys,
    toggleRowSelection,
    cursorKey,
  } = useCardRowSelection(
    cardId,
    hasActiveInGroup,
    (keys) => flipMergeSelection(keys),
    allRowKeys,
  );

  const resolveTargetRowKeys = (key: string): string[] =>
    selectedRowKeys.has(key)
      ? [key, ...[...selectedRowKeys].filter((k) => k !== key)]
      : [key];

  function flipMergeSelection(rowKeys: string[]) {
    for (const rk of rowKeys) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      for (const file of filesForMember(memberIndex)) {
        const current = new Set(fileSelectedIdsMap[file] ?? []);
        if (current.has(bareKey)) {
          current.delete(bareKey);
        } else {
          current.add(bareKey);
        }
        setFileSelectedIds(file, [...current]);
      }
    }
  }

  const toggleAll = (checked: boolean) => {
    for (const file of allFiles) {
      const tracks = fileTracksMap[file] ?? [];
      setFileSelectedIds(file, checked ? tracks.map((tk) => trackKey(tk)) : []);
    }
  };

  const toggleOne = (rowKey: string, checked: boolean) => {
    for (const rk of resolveTargetRowKeys(rowKey)) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      for (const file of filesForMember(memberIndex)) {
        const current = fileSelectedIdsMap[file] ?? [];
        if (checked) {
          if (!current.includes(bareKey)) {
            setFileSelectedIds(file, [...current, bareKey]);
          }
        } else {
          setFileSelectedIds(
            file,
            current.filter((k) => k !== bareKey),
          );
        }
      }
    }
  };

  const onTrackLanguageChange = (rowKey: string, value: string) => {
    for (const rk of resolveTargetRowKeys(rowKey)) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      setTrackLanguage(filesForMember(memberIndex), [bareKey], value);
    }
  };

  const onTrackNameChange = (rowKey: string, value: string) => {
    for (const rk of resolveTargetRowKeys(rowKey)) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      setTrackName(filesForMember(memberIndex), [bareKey], value);
    }
  };

  const onCycleFlag = (rowKey: string, kind: TrackFlagKind) => {
    const clicked = repCombined.find((tk) => rowKeyOf(tk) === rowKey);
    if (!clicked) {
      return;
    }
    const value = nextTrackFlag(
      kind === "default" ? clicked.defaultTrack : clicked.forced,
    );
    for (const rk of resolveTargetRowKeys(rowKey)) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      setTrackFlag(filesForMember(memberIndex), [bareKey], kind, value);
    }
  };

  // Reset default / forced per unit: each root is evaluated against its own
  // checked tracks (grouped roots may have different layouts, so a shared
  // representative would be wrong).
  const onDefaultHeaderClick = () => {
    for (const unit of units) {
      applyUnitFlagAutomation(
        unit,
        fileTracksMap,
        fileSelectedIdsMap,
        { resetDefault: true, resetForced: false },
        setTrackFlag,
      );
    }
  };

  const onForcedHeaderClick = () => {
    for (const unit of units) {
      applyUnitFlagAutomation(
        unit,
        fileTracksMap,
        fileSelectedIdsMap,
        { resetDefault: false, resetForced: true },
        setTrackFlag,
      );
    }
  };

  // Apply the profile's default/forced automation once, after every unit in the
  // group is loaded and auto-selected, so it operates on the flattened tree's
  // checked tracks (broadcast to the matching position in each unit).
  const flagAutomationDone = useRef(false);
  useEffect(() => {
    if (flagAutomationDone.current || !activeProfile) {
      return;
    }
    const ready = allFiles.every(
      (f) =>
        fileTracksMap[f] !== undefined && fileSelectedIdsMap[f] !== undefined,
    );
    if (!ready) {
      return;
    }
    flagAutomationDone.current = true;
    const resetDefault =
      activeProfile.automation?.reset_default_track.enabled ?? false;
    const resetForced =
      activeProfile.automation?.reset_forced_display.enabled ?? false;
    for (const unit of units) {
      applyUnitFlagAutomation(
        unit,
        fileTracksMap,
        fileSelectedIdsMap,
        { resetDefault, resetForced },
        setTrackFlag,
      );
    }
  }, [units, allFiles, activeProfile, fileTracksMap, fileSelectedIdsMap, setTrackFlag]);

  const onReorder = (fromRowKey: string, toRowKey: string) => {
    if (reorderDisabled) {
      return;
    }
    // Only reachable for one-member units; reorder every unit's root in lock-step.
    reorderTracks(
      roots,
      parseRowKey(fromRowKey).bareKey,
      parseRowKey(toRowKey).bareKey,
    );
  };

  const inputsForUnit = (unit: string[]): MergeInput[] =>
    unit.map((file) => {
      const sel = new Set(fileSelectedIdsMap[file] ?? []);
      const tracks = (fileTracksMap[file] ?? []).filter((tk) =>
        sel.has(trackKey(tk)),
      );
      return { file, tracks };
    });

  const enqueueUnit = async (unit: string[]) => {
    if (!activeProfile) {
      return;
    }
    const inputs = inputsForUnit(unit);
    if (inputs.every((i) => i.tracks.length === 0)) {
      return;
    }
    if (unit.length > 1) {
      await enqueueSelectedTracksForUnit({
        root: unit[0],
        inputs,
        profile: activeProfile,
        t,
      });
    } else {
      await enqueueSelectedTracksForFile({
        file: unit[0],
        selectedTracks: inputs[0]?.tracks ?? [],
        profile: activeProfile,
        t,
      });
    }
  };

  const handleCopyAll = async () => {
    if (!activeProfile || !hasSelection) {
      return;
    }
    const commands: string[] = [];
    try {
      for (const unit of units) {
        const inputs = inputsForUnit(unit).filter((i) => i.tracks.length > 0);
        if (inputs.length === 0) {
          continue;
        }
        const outputDir = await resolveOutputDir(
          unit[0],
          fileOutputDirs[unit[0]],
          globalOutputDir,
        );
        const outputPath = await resolveMergeOutputPath(outputDir, unit[0]);
        const command =
          unit.length > 1
            ? buildCommandStringMulti(
                inputs,
                outputPath,
                mkvToolNixPath,
                activeProfile,
              )
            : buildCommandString(
                unit[0],
                outputPath,
                mkvToolNixPath,
                inputs[0].tracks,
                activeProfile,
              );
        commands.push(command);
      }
      if (commands.length === 0) {
        return;
      }
      await writeText(commands.join("\n"));
      setSnackbar({ message: t("merge.commandCopied"), severity: "success" });
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleMergeAll = async () => {
    if (!activeProfile || !hasSelection) {
      return;
    }
    for (const unit of units) {
      try {
        await enqueueUnit(unit);
      } catch (err) {
        setSnackbar({ message: String(err), severity: "error" });
        return;
      }
    }
  };

  const handleCancel = async (root: string) => {
    await cancelMerge(root);
  };

  const handleClearAll = async () => {
    for (const unit of units) {
      const root = unit[0];
      const current = useMkvStore.getState().queueItems[root];
      if (current?.status === QueueItemStatus.Merging) {
        continue;
      }
      if (current?.status === QueueItemStatus.Waiting) {
        await cancelMerge(root);
      }
      removeFromQueue(root);
      for (const file of unit) {
        removeFile(file);
      }
    }
  };

  const handleOpenOutputDialog = () => {
    setOutputDialogInitial(groupOutputDir ?? parentDir);
    setOutputDialogOpen(true);
  };

  const handleOpenInBetterMediaInfo = async () => {
    try {
      await launchBetterMediaInfo(roots);
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleOutputConfirm = (value: string) => {
    if (value.length === 0) {
      clearGroupOutputDir(roots);
    } else {
      setGroupOutputDir(roots, value);
    }
  };

  return (
    <Paper
      variant="outlined"
      onClickCapture={activate}
      sx={{
        mt: 1,
        p: 1,
        borderColor: cardActive ? "primary.main" : undefined,
        bgcolor: hasWaitingInGroup ? "action.hover" : undefined,
      }}
    >
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 1 }}>
        <Box sx={{ flex: 1, minWidth: 0, ml: 2 }}>
          <Typography
            variant="body2"
            sx={{ wordBreak: "break-all", color: "text.secondary" }}
          >
            {parentDir}
          </Typography>
          <CardSummary counts={repCounts} outputPath={groupOutputDir} />
        </Box>
        <Tooltip title={t("merge.setOutputPath")}>
          <span>
            <IconButton
              size="small"
              disabled={hasActiveInGroup}
              onClick={handleOpenOutputDialog}
            >
              <FolderOpenIcon fontSize="small" />
            </IconButton>
          </span>
        </Tooltip>
        {betterMediaInfoAvailable && (
          <Tooltip title={t("merge.openInBetterMediaInfo")}>
            <span>
              <IconButton size="small" onClick={handleOpenInBetterMediaInfo}>
                <Box
                  component="img"
                  src={betterMediaInfoIcon}
                  alt="BetterMediaInfo"
                  sx={{ width: 20, height: 20 }}
                />
              </IconButton>
            </span>
          </Tooltip>
        )}
        <Tooltip title={t("group.copyAllCommands")}>
          <span>
            <IconButton
              size="small"
              disabled={!canCopyAll}
              onClick={handleCopyAll}
            >
              <ContentCopyIcon fontSize="small" />
            </IconButton>
          </span>
        </Tooltip>
        <Tooltip title={t("group.mergeAll")}>
          <span>
            <Button
              variant="outlined"
              size="small"
              startIcon={<HubIcon />}
              disabled={!canMergeAll}
              onClick={handleMergeAll}
              sx={{ textTransform: "none", whiteSpace: "nowrap" }}
            >
              {t("group.mergeAll")}
            </Button>
          </span>
        </Tooltip>
        <Tooltip title={t("group.clearAll")}>
          <span>
            <IconButton
              size="small"
              color="error"
              disabled={!canClearAll}
              onClick={handleClearAll}
            >
              <DeleteIcon fontSize="small" />
            </IconButton>
          </span>
        </Tooltip>
      </Box>
      <Box ref={splitContainerRef} sx={{ display: "flex", position: "relative" }}>
        <Box sx={{ width: leftWidth, flexShrink: 0, overflow: "auto" }}>
          <List dense>
            {units.map((unit) => {
              const root = unit[0];
              const childCount = unit.length - 1;
              const entry = queueItems[root];
              const isMerging = entry?.status === QueueItemStatus.Merging;
              const startedAt = entry?.mergeStartedAt ?? null;
              const elapsedMs =
                isMerging && startedAt !== null ? now - startedAt : 0;
              const progressPct = entry?.progress ?? 0;
              const elapsedStr = isMerging ? formatHMS(elapsedMs) : "--:--:--";
              const etaStr =
                isMerging && progressPct > 0 && progressPct < 100
                  ? formatHMS((elapsedMs * (100 - progressPct)) / progressPct)
                  : "--:--:--";
              return (
                <ListItem
                  key={root}
                  sx={{
                    py: 0.5,
                    alignItems: "flex-start",
                    flexDirection: "column",
                    gap: 0.5,
                  }}
                >
                  <Box
                    sx={{
                      display: "flex",
                      alignItems: "center",
                      gap: 0.5,
                      width: "100%",
                    }}
                  >
                    <FileStatusIcon status={entry?.status} />
                    {childCount > 0 ? (
                      <Badge
                        color="primary"
                        badgeContent={childCount}
                        max={999}
                        sx={{
                          flex: 1,
                          "& .MuiBadge-badge": {
                            position: "static",
                            transform: "none",
                            fontSize: "0.6rem",
                            height: 15,
                            minWidth: 15,
                            px: 0.5,
                          },
                        }}
                      >
                        <Typography
                          variant="body2"
                          sx={{ wordBreak: "break-all", mr: 1 }}
                        >
                          {getFileName(root)}
                        </Typography>
                      </Badge>
                    ) : (
                      <Typography
                        variant="body2"
                        sx={{ wordBreak: "break-all", flex: 1 }}
                      >
                        {getFileName(root)}
                      </Typography>
                    )}
                  </Box>
                  {isMerging ? (
                    <>
                      <Box
                        sx={{
                          display: "flex",
                          alignItems: "center",
                          gap: 0.5,
                          width: "100%",
                        }}
                      >
                        <LinearProgress
                          variant="determinate"
                          value={progressPct}
                          sx={{
                            flex: 1,
                            height: 6,
                            borderRadius: 1,
                            bgcolor: "action.hover",
                            "& .MuiLinearProgress-bar": {
                              bgcolor: "success.main",
                            },
                          }}
                        />
                        <Typography
                          variant="caption"
                          sx={{
                            fontFamily:
                              "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                            color: "text.secondary",
                            minWidth: 34,
                            textAlign: "right",
                          }}
                        >
                          {progressPct}%
                        </Typography>
                        <Tooltip title={t("merge.cancel")}>
                          <IconButton
                            size="small"
                            color="error"
                            onClick={() => handleCancel(root)}
                          >
                            <CancelIcon fontSize="small" />
                          </IconButton>
                        </Tooltip>
                      </Box>
                      <Typography
                        variant="caption"
                        sx={{
                          fontFamily:
                            "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                          color: "text.secondary",
                          width: "100%",
                        }}
                      >
                        {elapsedStr} / {etaStr}
                      </Typography>
                    </>
                  ) : null}
                </ListItem>
              );
            })}
          </List>
        </Box>
        <Box
          onMouseDown={startResize}
          sx={{
            width: 6,
            flexShrink: 0,
            cursor: "col-resize",
            bgcolor: "divider",
            "&:hover": { bgcolor: "action.hover" },
            transition: "background-color 0.15s",
          }}
        />
        <Box sx={{ flex: 1, minWidth: 0, ml: 1 }}>
          <TrackSelectionTable
            tracks={repCombined}
            selectedIds={selectedIds}
            selectedRowKeys={selectedRowKeys}
            cursorKey={cursorKey}
            formatting={formatting}
            disabled={hasActiveInGroup}
            loading={loading}
            errorText={error}
            rowKey={(tk) => rowKeyOf(tk as CombinedTrack)}
            idLabel={
              reorderDisabled
                ? (tk) => `${(tk as CombinedTrack).memberIndex}:${tk.id}`
                : undefined
            }
            reorderDisabled={reorderDisabled}
            emptyText={t("merge.noTracks")}
            headers={{
              id: t("merge.header.id"),
              type: t("merge.header.type"),
              codec: t("merge.header.codec"),
              description: t("merge.header.description"),
              size: t("merge.header.size"),
              bitRate: t("merge.header.bitRate"),
              trackName: t("merge.header.trackName"),
              language: t("merge.header.language"),
              defaultTrack: t("merge.header.defaultTrack"),
              forcedDisplay: t("merge.header.forcedDisplay"),
            }}
            onToggleAll={toggleAll}
            onToggleOne={toggleOne}
            onToggleRowSelection={toggleRowSelection}
            languageOptionsFor={(type) =>
              buildLanguageOptions(activeProfile, type)
            }
            trackNameOptionsFor={(type, language) =>
              buildTrackNameOptions(activeProfile, type, language)
            }
            onTrackLanguageChange={onTrackLanguageChange}
            onTrackNameChange={onTrackNameChange}
            onCycleFlag={onCycleFlag}
            onDefaultHeaderClick={onDefaultHeaderClick}
            onForcedHeaderClick={onForcedHeaderClick}
            onReorder={onReorder}
          />
        </Box>
      </Box>
      <Snackbar
        open={snackbar !== null}
        autoHideDuration={3000}
        onClose={() => setSnackbar(null)}
        anchorOrigin={{ vertical: "bottom", horizontal: "center" }}
      >
        <Alert
          onClose={() => setSnackbar(null)}
          severity={snackbar?.severity ?? "success"}
          variant="filled"
        >
          {snackbar?.message}
        </Alert>
      </Snackbar>
      <OutputPathDialog
        open={outputDialogOpen}
        initialValue={outputDialogInitial}
        onConfirm={handleOutputConfirm}
        onClose={() => setOutputDialogOpen(false)}
      />
    </Paper>
  );
}
