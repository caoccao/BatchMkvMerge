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
  Card,
  CardContent,
  CardHeader,
  IconButton,
  LinearProgress,
  Popover,
  Snackbar,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Tooltip,
  Typography,
} from "@mui/material";
import CancelIcon from "@mui/icons-material/Cancel";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import HubIcon from "@mui/icons-material/Hub";
import DeleteIcon from "@mui/icons-material/Delete";
import DragIndicatorIcon from "@mui/icons-material/DragIndicator";
import RemoveCircleOutlineIcon from "@mui/icons-material/RemoveCircleOutlined";
import FolderOpenIcon from "@mui/icons-material/FolderOpen";
import betterMediaInfoIcon from "../assets/bettermediainfo.png";
import { basename } from "@tauri-apps/api/path";
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
import { QueueItemStatus } from "../protocol";
import {
  launchBetterMediaInfo,
  resolveMergeOutputPath,
  resolveOverriddenOutputPath,
} from "../service";
import { mediaTrackCounts } from "../media-metadata";
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
import { beginCardDrag, CARD_ROOT_ATTR } from "./cardDrag";

interface MkvFileCardProps {
  /** The merge unit's member files, root first. A single-member unit is the
   *  ordinary one-file card; a multi-member unit is a *Group by file name* tree
   *  flattened into one combined table that merges into one output. */
  memberFiles: string[];
}

/** Child-files dropdown becomes scrollable beyond this many rows. */
const CHILD_MENU_VISIBLE_ROWS = 5;
const CHILD_MENU_ROW_HEIGHT = 36;
/** Max width of the child-files dropdown — it grows to show the full path and
 *  middle-truncates only when a path is longer than this. */
const CHILD_MENU_MAX_WIDTH = "min(90vw, 760px)";
/** Max width of the File Path cell; the table grows to fit shorter paths and
 *  middle-truncates only when a path exceeds this. */
const CHILD_PATH_CELL_MAX_WIDTH = "min(80vw, 680px)";
/** Number of trailing path characters always kept visible when middle-truncating. */
const PATH_TAIL_CHARS = 18;

/**
 * Render a file path that grows to its natural width but, when constrained by
 * the parent's max-width, keeps the start and the last [`PATH_TAIL_CHARS`]
 * characters and collapses the middle to an ellipsis (CSS-only: the head shrinks
 * with `text-overflow: ellipsis`, the tail never shrinks).
 */
function MiddleEllipsisPath({ path }: { path: string }) {
  const tailLength = Math.min(path.length, PATH_TAIL_CHARS);
  const head = path.slice(0, path.length - tailLength);
  const tail = path.slice(path.length - tailLength);
  return (
    <Box sx={{ display: "flex", width: "100%", minWidth: 0, fontSize: "0.875rem" }}>
      <Box
        component="span"
        sx={{
          flex: "0 1 auto",
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {head}
      </Box>
      <Box component="span" sx={{ flex: "0 0 auto", whiteSpace: "nowrap" }}>
        {tail}
      </Box>
    </Box>
  );
}

export function MkvFileCard({ memberFiles }: MkvFileCardProps) {
  const { t } = useTranslation();
  const root = memberFiles[0];
  const isMulti = memberFiles.length > 1;
  const childCount = memberFiles.length - 1;
  const childFiles = memberFiles.slice(1);

  const removeFile = useMkvStore((s) => s.removeFile);
  const detachFileFromRoot = useMkvStore((s) => s.detachFileFromRoot);
  const mkvToolNixPath = useMkvStore(
    (s) => s.config?.externalTools?.mkvToolNixPath ?? "",
  );
  const entry = useMkvStore((s) => s.queueItems[root]);
  const setTrackLanguage = useMkvStore((s) => s.setTrackLanguage);
  const setTrackName = useMkvStore((s) => s.setTrackName);
  const setTrackFlag = useMkvStore((s) => s.setTrackFlag);
  const reorderTracks = useMkvStore((s) => s.reorderTracks);
  const setFileSelectedIds = useMkvStore((s) => s.setFileSelectedIds);
  const setFileOutputPath = useMkvStore((s) => s.setFileOutputPath);
  const clearFileOutputPath = useMkvStore((s) => s.clearFileOutputPath);
  const fileTracksMap = useMkvStore((s) => s.fileTracks);
  const fileSelectedIdsMap = useMkvStore((s) => s.fileSelectedIds);
  const outputPathOverride = useMkvStore((s) => s.fileOutputPaths[root]);
  const globalOutputDir = useMkvStore((s) => s.globalOutputDir);
  const betterMediaInfoAvailable = useMkvStore(
    (s) => s.betterMediaInfoAvailable,
  );
  const formatting = useMkvStore((s) => s.config?.formatting ?? null);
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

  const { loading, error } = useFilesLoad(memberFiles, t);

  const isMerging = entry?.status === QueueItemStatus.Merging;
  const isQueued = entry?.status === QueueItemStatus.Waiting;
  const isActive = isMerging || isQueued;

  const [snackbar, setSnackbar] = useState<{
    message: string;
    severity: "success" | "error";
  } | null>(null);
  const [outputDialogOpen, setOutputDialogOpen] = useState(false);
  const [outputDialogInitial, setOutputDialogInitial] = useState("");
  const [outputDialogDefaultName, setOutputDialogDefaultName] = useState("");
  // Anchor for the child-files dropdown opened from the badge; null = closed.
  const [childMenuAnchor, setChildMenuAnchor] = useState<HTMLElement | null>(
    null,
  );
  // True while a valid card drag is hovering over this card (drop highlight).
  const dropActive = useMkvStore((s) => s.dropTargetRoot === root);

  // The whole tree flattened into one stable, sorted track list.
  const combined = useMemo<CombinedTrack[]>(
    () => combineUnitTracks(memberFiles, fileTracksMap),
    [memberFiles, fileTracksMap],
  );
  const trackCounts = useMemo(() => mediaTrackCounts(combined), [combined]);
  const allRowKeys = useMemo(() => combined.map(rowKeyOf), [combined]);

  // Selection lives per-file in the store; lift each member's bare keys into
  // the combined `${memberIndex}:${type}:${id}` space the table renders.
  const selectedIds = useMemo(() => {
    const ids = new Set<string>();
    memberFiles.forEach((file, memberIndex) => {
      for (const bareKey of fileSelectedIdsMap[file] ?? []) {
        ids.add(`${memberIndex}:${bareKey}`);
      }
    });
    return ids;
  }, [memberFiles, fileSelectedIdsMap]);

  const hasSelection = selectedIds.size > 0;

  // Group a set of combined row keys by their source file → that file's bare
  // keys, so a single logical edit fans out to the right per-file mutation.
  const groupByFile = (rowKeys: string[]): Map<string, string[]> => {
    const map = new Map<string, string[]>();
    for (const rk of rowKeys) {
      const { memberIndex, bareKey } = parseRowKey(rk);
      const file = memberFiles[memberIndex];
      if (!file) {
        continue;
      }
      const arr = map.get(file);
      if (arr) {
        arr.push(bareKey);
      } else {
        map.set(file, [bareKey]);
      }
    }
    return map;
  };

  const {
    cardActive,
    activate,
    selectedRowKeys,
    toggleRowSelection,
    cursorKey,
  } = useCardRowSelection(
    root,
    isActive,
    (keys) => flipMergeSelection(keys),
    allRowKeys,
  );

  const resolveTargetRowKeys = (key: string): string[] =>
    selectedRowKeys.has(key)
      ? [key, ...[...selectedRowKeys].filter((k) => k !== key)]
      : [key];

  function flipMergeSelection(rowKeys: string[]) {
    for (const [file, bareKeys] of groupByFile(rowKeys)) {
      const current = new Set(fileSelectedIdsMap[file] ?? []);
      for (const k of bareKeys) {
        if (current.has(k)) {
          current.delete(k);
        } else {
          current.add(k);
        }
      }
      setFileSelectedIds(file, [...current]);
    }
  }

  const toggleAll = (checked: boolean) => {
    memberFiles.forEach((file) => {
      const tracks = fileTracksMap[file] ?? [];
      setFileSelectedIds(file, checked ? tracks.map((tk) => trackKey(tk)) : []);
    });
  };

  const toggleOne = (rowKey: string, checked: boolean) => {
    for (const [file, bareKeys] of groupByFile(resolveTargetRowKeys(rowKey))) {
      const current = fileSelectedIdsMap[file] ?? [];
      if (checked) {
        const existing = new Set(current);
        setFileSelectedIds(file, [
          ...current,
          ...bareKeys.filter((k) => !existing.has(k)),
        ]);
      } else {
        const remove = new Set(bareKeys);
        setFileSelectedIds(
          file,
          current.filter((k) => !remove.has(k)),
        );
      }
    }
  };

  const onTrackLanguageChange = (rowKey: string, value: string) => {
    for (const [file, bareKeys] of groupByFile(resolveTargetRowKeys(rowKey))) {
      setTrackLanguage([file], bareKeys, value);
    }
  };

  const onTrackNameChange = (rowKey: string, value: string) => {
    for (const [file, bareKeys] of groupByFile(resolveTargetRowKeys(rowKey))) {
      setTrackName([file], bareKeys, value);
    }
  };

  const onCycleFlag = (rowKey: string, kind: TrackFlagKind) => {
    const clicked = combined.find((tk) => rowKeyOf(tk) === rowKey);
    if (!clicked) {
      return;
    }
    const current = kind === "default" ? clicked.defaultTrack : clicked.forced;
    const value = nextTrackFlag(current);
    for (const [file, bareKeys] of groupByFile(resolveTargetRowKeys(rowKey))) {
      setTrackFlag([file], bareKeys, kind, value);
    }
  };

  const onDefaultHeaderClick = () =>
    applyUnitFlagAutomation(
      memberFiles,
      fileTracksMap,
      fileSelectedIdsMap,
      { resetDefault: true, resetForced: false },
      setTrackFlag,
    );

  const onForcedHeaderClick = () =>
    applyUnitFlagAutomation(
      memberFiles,
      fileTracksMap,
      fileSelectedIdsMap,
      { resetDefault: false, resetForced: true },
      setTrackFlag,
    );

  const onReorder = (fromRowKey: string, toRowKey: string) => {
    if (isMulti) {
      return;
    }
    reorderTracks(
      [root],
      parseRowKey(fromRowKey).bareKey,
      parseRowKey(toRowKey).bareKey,
    );
  };

  // Apply the profile's default/forced automation — only after the WHOLE unit
  // (the flattened tree) is loaded and auto-selected, so it picks one default
  // per type across the merged file rather than one per member. Re-runs when the
  // member set changes (e.g. another card is drag-merged in) so the combined
  // unit gets one default per type again.
  const flagAutomationSig = useRef<string | null>(null);
  useEffect(() => {
    if (!activeProfile) {
      return;
    }
    const ready = memberFiles.every(
      (f) =>
        fileTracksMap[f] !== undefined && fileSelectedIdsMap[f] !== undefined,
    );
    if (!ready) {
      return;
    }
    const sig = memberFiles.join("\n");
    if (flagAutomationSig.current === sig) {
      return;
    }
    flagAutomationSig.current = sig;
    applyUnitFlagAutomation(
      memberFiles,
      fileTracksMap,
      fileSelectedIdsMap,
      {
        resetDefault: activeProfile.automation?.reset_default_track.enabled ?? false,
        resetForced: activeProfile.automation?.reset_forced_display.enabled ?? false,
      },
      setTrackFlag,
    );
  }, [memberFiles, activeProfile, fileTracksMap, fileSelectedIdsMap, setTrackFlag]);

  // Per-member selected tracks (in each file's own order) for the merge command.
  const mergeInputs = (): MergeInput[] =>
    memberFiles.map((file) => {
      const sel = new Set(fileSelectedIdsMap[file] ?? []);
      const tracks = (fileTracksMap[file] ?? []).filter((tk) =>
        sel.has(trackKey(tk)),
      );
      return { file, tracks };
    });

  const buildCurrentCommand = async (): Promise<string | null> => {
    if (!hasSelection || !activeProfile) {
      return null;
    }
    const outputPath =
      outputPathOverride && outputPathOverride.length > 0
        ? await resolveOverriddenOutputPath(outputPathOverride, root)
        : await resolveMergeOutputPath(
            await resolveOutputDir(root, undefined, globalOutputDir),
            root,
          );
    if (isMulti) {
      return buildCommandStringMulti(
        mergeInputs().filter((i) => i.tracks.length > 0),
        outputPath,
        mkvToolNixPath,
        activeProfile,
      );
    }
    return buildCommandString(
      root,
      outputPath,
      mkvToolNixPath,
      mergeInputs()[0]?.tracks ?? [],
      activeProfile,
    );
  };

  const handleCopyCommand = async () => {
    try {
      const command = await buildCurrentCommand();
      if (!command) {
        return;
      }
      await writeText(command);
      setSnackbar({ message: t("merge.commandCopied"), severity: "success" });
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleMerge = async () => {
    if (!hasSelection || isActive || !activeProfile) {
      return;
    }
    try {
      if (isMulti) {
        await enqueueSelectedTracksForUnit({
          root,
          inputs: mergeInputs(),
          profile: activeProfile,
        });
      } else {
        await enqueueSelectedTracksForFile({
          file: root,
          selectedTracks: mergeInputs()[0]?.tracks ?? [],
          profile: activeProfile,
        });
      }
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleCancel = async () => {
    await cancelMerge(root, (err) =>
      setSnackbar({ message: String(err), severity: "error" }),
    );
  };

  const handleOpenOutputDialog = async () => {
    let initial = outputPathOverride ?? "";
    if (initial.length === 0) {
      // Default to the full output file path that the merge would produce.
      try {
        const dir = await resolveOutputDir(root, undefined, globalOutputDir);
        initial = await resolveMergeOutputPath(dir, root);
      } catch {
        initial = "";
      }
    }
    try {
      setOutputDialogDefaultName(await basename(root));
    } catch {
      setOutputDialogDefaultName("");
    }
    setOutputDialogInitial(initial);
    setOutputDialogOpen(true);
  };

  const handleOpenInBetterMediaInfo = async () => {
    try {
      await launchBetterMediaInfo([root]);
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleOutputConfirm = (value: string) => {
    if (value.length === 0) {
      clearFileOutputPath(root);
    } else {
      setFileOutputPath(root, value);
    }
  };

  // Start a pointer-based card drag (HTML5 DnD is unavailable under Tauri's
  // file-drop). The controller in `cardDrag.ts` handles hover + drop/merge.
  const handleDragHandlePointerDown = (e: React.PointerEvent) => {
    if (isActive) {
      return;
    }
    e.preventDefault();
    beginCardDrag(root);
  };

  const openChildMenu = (e: React.MouseEvent<HTMLElement>) => {
    e.stopPropagation();
    setChildMenuAnchor(e.currentTarget);
  };
  const closeChildMenu = () => setChildMenuAnchor(null);

  // Detach a child from this card. The unit's flag automation re-runs on the
  // remaining members (member-set signature changes). Keep the dropdown open so
  // several children can be removed in a row; close it once the last one goes.
  const handleRemoveChild = (file: string) => {
    if (childFiles.length <= 1) {
      setChildMenuAnchor(null);
    }
    detachFileFromRoot(file);
  };

  const handleDelete = async () => {
    const current = useMkvStore.getState().queueItems[root];
    if (current?.status === QueueItemStatus.Merging) {
      return;
    }
    if (current?.status === QueueItemStatus.Waiting) {
      await cancelMerge(root);
      const later = useMkvStore.getState().queueItems[root];
      if (later?.status === QueueItemStatus.Merging) {
        return;
      }
      useMkvStore.getState().removeFromQueue(root);
    }
    for (const file of memberFiles) {
      removeFile(file);
    }
  };

  const titleContent = (
    <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
      <FileStatusIcon status={entry?.status} />
      {isMulti ? (
        <Badge
          color="primary"
          badgeContent={childCount}
          max={999}
          slotProps={{
            badge: {
              onClick: openChildMenu,
              role: "button",
              "aria-label": t("merge.childCount", { count: childCount }),
            },
          }}
          sx={{
            "& .MuiBadge-badge": {
              position: "static",
              transform: "none",
              fontSize: "0.6rem",
              height: 15,
              minWidth: 15,
              px: 0.5,
              cursor: "pointer",
              "&:hover": { filter: "brightness(1.15)" },
            },
          }}
        >
          <Tooltip title={t("merge.childCount", { count: childCount })}>
            <Typography variant="body2" sx={{ wordBreak: "break-all", mr: 1 }}>
              {root}
            </Typography>
          </Tooltip>
        </Badge>
      ) : (
        <Typography variant="body2" sx={{ wordBreak: "break-all" }}>
          {root}
        </Typography>
      )}
    </Box>
  );

  const actionContent = (
    <Box sx={{ display: "flex", gap: 0.5 }}>
      <Tooltip title={t("merge.setOutputPath")}>
        <span>
          <IconButton
            size="small"
            disabled={isActive}
            onClick={handleOpenOutputDialog}
            sx={{ color: outputPathOverride ? "primary.main" : "text.secondary" }}
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
      <Tooltip title={t("merge.copyCommand")}>
        <span>
          <IconButton
            size="small"
            disabled={!hasSelection || isActive}
            onClick={handleCopyCommand}
          >
            <ContentCopyIcon fontSize="small" />
          </IconButton>
        </span>
      </Tooltip>
      <Button
        variant="outlined"
        size="small"
        startIcon={<HubIcon />}
        disabled={!hasSelection || isActive}
        onClick={handleMerge}
        sx={{ textTransform: "none", whiteSpace: "nowrap" }}
      >
        {t("merge.merge")}
      </Button>
      <Tooltip title={t("list.delete")}>
        <span>
          <IconButton
            size="small"
            color="error"
            disabled={isMerging}
            onClick={handleDelete}
          >
            <DeleteIcon fontSize="small" />
          </IconButton>
        </span>
      </Tooltip>
      <Tooltip title={t("merge.dragToMerge")}>
        <Box component="span" sx={{ display: "inline-flex" }}>
          <IconButton
            size="small"
            disabled={isActive}
            disableRipple
            onPointerDown={handleDragHandlePointerDown}
            sx={{ cursor: isActive ? "default" : "grab", touchAction: "none" }}
          >
            <DragIndicatorIcon fontSize="small" />
          </IconButton>
        </Box>
      </Tooltip>
    </Box>
  );

  const progress = entry?.progress ?? 0;
  const startedAt = entry?.mergeStartedAt ?? null;
  const elapsedMs =
    isMerging && startedAt !== null ? Date.now() - startedAt : 0;
  const elapsedStr = isMerging ? formatHMS(elapsedMs) : "--:--:--";
  const etaStr =
    isMerging && progress > 0 && progress < 100
      ? formatHMS((elapsedMs * (100 - progress)) / progress)
      : "--:--:--";

  return (
    <Card
      variant="outlined"
      onClickCapture={activate}
      {...{ [CARD_ROOT_ATTR]: root }}
      sx={{
        mt: 1,
        bgcolor: dropActive
          ? "action.selected"
          : isQueued
            ? "action.hover"
            : undefined,
        borderColor:
          dropActive || cardActive ? "primary.main" : undefined,
        ...(dropActive
          ? {
              outline: "2px dashed",
              outlineColor: "primary.main",
              outlineOffset: -2,
            }
          : {}),
      }}
    >
      <CardHeader
        title={titleContent}
        subheader={
          <CardSummary counts={trackCounts} outputPath={outputPathOverride} />
        }
        action={actionContent}
        sx={{
          pb: isActive ? 0 : 1,
          "& .MuiCardHeader-content": { minWidth: 0, flex: 1 },
        }}
      />
      {isActive && (
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            gap: 1,
            px: 2,
            pb: 1,
            mt: 1,
          }}
        >
          {isMerging ? (
            <>
              <LinearProgress
                variant="determinate"
                value={progress}
                sx={{
                  flex: 1,
                  height: 6,
                  borderRadius: 1,
                  bgcolor: "action.hover",
                  "& .MuiLinearProgress-bar": { bgcolor: "success.main" },
                }}
              />
              <Typography
                variant="caption"
                sx={{
                  fontFamily:
                    "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                  color: "text.secondary",
                  whiteSpace: "nowrap",
                }}
              >
                {elapsedStr} / {etaStr}
              </Typography>
            </>
          ) : (
            <Box sx={{ flex: 1 }} />
          )}
          <Tooltip title={t("merge.cancel")}>
            <IconButton size="small" color="error" onClick={handleCancel}>
              <CancelIcon fontSize="small" />
            </IconButton>
          </Tooltip>
        </Box>
      )}
      <CardContent sx={{ pt: 0, "&.MuiCardContent-root:last-child": { pb: 2 } }}>
        <TrackSelectionTable
          tracks={combined}
          selectedIds={selectedIds}
          selectedRowKeys={selectedRowKeys}
          cursorKey={cursorKey}
          formatting={formatting}
          disabled={isActive}
          loading={loading}
          errorText={error}
          rowKey={(tk) => rowKeyOf(tk as CombinedTrack)}
          idLabel={
            isMulti
              ? (tk) => `${(tk as CombinedTrack).memberIndex}:${tk.id}`
              : undefined
          }
          reorderDisabled={isMulti}
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
          languageOptionsFor={(type) => buildLanguageOptions(activeProfile, type)}
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
      </CardContent>
      <Snackbar
        open={snackbar !== null}
        autoHideDuration={5000}
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
        mode="file"
        defaultFileName={outputDialogDefaultName}
        onConfirm={handleOutputConfirm}
        onClose={() => setOutputDialogOpen(false)}
      />
      <Popover
        anchorEl={childMenuAnchor}
        open={Boolean(childMenuAnchor)}
        onClose={closeChildMenu}
        anchorOrigin={{ vertical: "bottom", horizontal: "left" }}
        transformOrigin={{ vertical: "top", horizontal: "left" }}
      >
        <TableContainer
          sx={{
            maxWidth: CHILD_MENU_MAX_WIDTH,
            // Header row + CHILD_MENU_VISIBLE_ROWS body rows, then scroll.
            maxHeight: (CHILD_MENU_VISIBLE_ROWS + 1) * CHILD_MENU_ROW_HEIGHT,
          }}
        >
          <Table size="small" stickyHeader>
            <TableHead>
              <TableRow>
                <TableCell align="right" sx={{ width: 40 }}>
                  {t("merge.childTable.id")}
                </TableCell>
                <TableCell>{t("merge.childTable.filePath")}</TableCell>
                <TableCell align="center" sx={{ width: 48 }}>
                  {t("merge.childTable.action")}
                </TableCell>
              </TableRow>
            </TableHead>
            <TableBody>
              {childFiles.map((file, index) => (
                <TableRow key={file} hover>
                  <TableCell align="right">{index + 1}</TableCell>
                  <TableCell sx={{ maxWidth: CHILD_PATH_CELL_MAX_WIDTH }}>
                    <Tooltip title={file}>
                      <MiddleEllipsisPath path={file} />
                    </Tooltip>
                  </TableCell>
                  <TableCell align="center" sx={{ py: 0 }}>
                    <Tooltip title={t("merge.removeFromRoot")}>
                      <IconButton
                        size="small"
                        color="error"
                        onClick={() => handleRemoveChild(file)}
                      >
                        <RemoveCircleOutlineIcon fontSize="small" />
                      </IconButton>
                    </Tooltip>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      </Popover>
    </Card>
  );
}
