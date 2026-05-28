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

import { useEffect, useMemo, useState } from "react";
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  CardHeader,
  IconButton,
  LinearProgress,
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
import { dirname } from "@tauri-apps/api/path";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useTranslation } from "react-i18next";
import {
  cancelMerge,
  enqueueSelectedTracksForFile,
} from "../actions/mergeActions";
import {
  buildCommandString,
  formatHMS,
  makeTrackSelector,
  resolveOutputDir,
  trackKey,
} from "../merge";
import type { MediaMetadataError } from "../protocol";
import { QueueItemStatus } from "../protocol";
import {
  getMediaMetadata,
  launchBetterMediaInfo,
  resolveMergeOutputPath,
} from "../service";
import { useMkvStore } from "../store";
import { CardSummary } from "./CardSummary";
import { FileStatusIcon } from "./FileStatusIcon";
import { OutputPathDialog } from "./OutputPathDialog";
import { TrackSelectionTable } from "./TrackSelectionTable";
import {
  buildLanguageOptions,
  buildTrackNameOptions,
} from "./TrackCellAutocomplete";
import { useCardRowSelection } from "./useCardRowSelection";

interface MkvFileCardProps {
  path: string;
}

type TranslateFn = (
  key: string,
  options?: Record<string, string | number>,
) => string;

function isMediaMetadataError(value: unknown): value is MediaMetadataError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    typeof (value as { kind: unknown }).kind === "string"
  );
}

/**
 * Map a `get_media_metadata` rejection to a human-readable string. Backend
 * categorises every failure into one of the [`MediaMetadataError`] tagged
 * variants; the i18n keys live under `merge.error.parser.*`. Unrecognised
 * values fall back to `String(err)` so debug output is never silently lost.
 */
function formatMetadataError(err: unknown, t: TranslateFn): string {
  if (isMediaMetadataError(err)) {
    switch (err.kind) {
      case "io":
        return t("merge.error.parser.io", { detail: err.detail });
      case "unexpectedEof":
        return t("merge.error.parser.unexpectedEof", { detail: err.detail });
      case "unrecognised":
        return t("merge.error.parser.unrecognised", { detail: err.detail });
      case "timeout":
        return t("merge.error.parser.timeout", {
          budgetMs: err.budgetMs,
          stage: err.stage,
          detail: err.detail,
        });
      case "malformed":
        return t("merge.error.parser.malformed", { detail: err.detail });
      case "oversizedElement":
        return t("merge.error.parser.oversizedElement", {
          detail: err.detail,
        });
      case "internal":
        return t("merge.error.parser.internal", { detail: err.detail });
    }
  }
  return String(err);
}

export function MkvFileCard({ path }: MkvFileCardProps) {
  const { t } = useTranslation();
  const removeFile = useMkvStore((s) => s.removeFile);
  const mkvToolNixPath = useMkvStore(
    (s) => s.config?.externalTools?.mkvToolNixPath ?? "",
  );
  const entry = useMkvStore((s) => s.queueItems[path]);
  const setFileMetadata = useMkvStore((s) => s.setFileMetadata);
  const setFileSelectedIds = useMkvStore((s) => s.setFileSelectedIds);
  const cycleTrackFlag = useMkvStore((s) => s.cycleTrackFlag);
  const setDefaultTrackByType = useMkvStore((s) => s.setDefaultTrackByType);
  const clearForcedFlags = useMkvStore((s) => s.clearForcedFlags);
  const reorderTracks = useMkvStore((s) => s.reorderTracks);
  const setTrackLanguage = useMkvStore((s) => s.setTrackLanguage);
  const setTrackName = useMkvStore((s) => s.setTrackName);
  const applyAutomationToFile = useMkvStore((s) => s.applyAutomationToFile);
  const setFileOutputDir = useMkvStore((s) => s.setFileOutputDir);
  const clearFileOutputDir = useMkvStore((s) => s.clearFileOutputDir);
  const cachedTracks = useMkvStore((s) => s.fileTracks[path]);
  const storedSelectedIds = useMkvStore((s) => s.fileSelectedIds[path]);
  const outputDirOverride = useMkvStore((s) => s.fileOutputDirs[path]);
  const trackCounts = useMkvStore((s) => s.fileTrackCounts[path]);
  const betterMediaInfoAvailable = useMkvStore(
    (s) => s.betterMediaInfoAvailable,
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

  const isMerging = entry?.status === QueueItemStatus.Merging;
  const isQueued = entry?.status === QueueItemStatus.Waiting;
  const isActive = isMerging || isQueued;

  const [loading, setLoading] = useState<boolean>(
    () => cachedTracks === undefined,
  );
  const [error, setError] = useState<string | null>(null);
  const tracks = cachedTracks ?? [];
  const selectedIds = useMemo(
    () => new Set<string>(storedSelectedIds ?? []),
    [storedSelectedIds],
  );
  const [snackbar, setSnackbar] = useState<{
    message: string;
    severity: "success" | "error";
  } | null>(null);
  const [outputDialogOpen, setOutputDialogOpen] = useState(false);
  const [outputDialogInitial, setOutputDialogInitial] = useState("");

  useEffect(() => {
    if (storedSelectedIds !== undefined) {
      return;
    }
    if (tracks.length === 0 || !activeProfile) {
      return;
    }
    const auto: string[] = [];
    const selectTrack = makeTrackSelector(activeProfile);
    for (const track of tracks) {
      if (selectTrack(track)) {
        auto.push(trackKey(track));
      }
    }
    setFileSelectedIds(path, auto);
  }, [path, tracks, activeProfile, storedSelectedIds, setFileSelectedIds]);

  useEffect(() => {
    if (useMkvStore.getState().fileTracks[path] !== undefined) {
      setLoading(false);
      setError(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    getMediaMetadata(path)
      .then((metadata) => {
        if (cancelled) {
          return;
        }
        setFileMetadata(path, metadata);
        // Apply per-profile automation once, to this newly added file.
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
            automation.set_track_name.enabled)
        ) {
          applyAutomationToFile(
            path,
            automation.reset_und_language,
            automation.set_track_name.enabled,
            (type, language) =>
              buildTrackNameOptions(profile, type, language)[0],
          );
        }
        setLoading(false);
      })
      .catch((err: unknown) => {
        if (cancelled) {
          return;
        }
        setError(formatMetadataError(err, t));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [path, t, setFileMetadata, applyAutomationToFile]);

  const selectedTracks = tracks.filter((track) =>
    selectedIds.has(trackKey(track)),
  );
  const hasSelection = selectedTracks.length > 0;

  const toggleAll = (checked: boolean) => {
    setFileSelectedIds(
      path,
      checked ? tracks.map((t) => trackKey(t)) : [],
    );
  };

  const flipMergeSelection = (keys: string[]) => {
    const current = new Set(storedSelectedIds ?? []);
    for (const key of keys) {
      if (current.has(key)) {
        current.delete(key);
      } else {
        current.add(key);
      }
    }
    setFileSelectedIds(path, [...current]);
  };

  const { cardActive, activate, selectedRowKeys, toggleRowSelection } =
    useCardRowSelection(
      path,
      isActive,
      flipMergeSelection,
      tracks.map((track) => trackKey(track)),
    );

  // A per-row edit (checkbox, default/forced flags, language, name) applies to
  // every selected row when the edited row is part of the selection; otherwise
  // just to itself. The clicked key stays first so flag-cycling reads its value.
  const resolveTargetRowKeys = (key: string): string[] =>
    selectedRowKeys.has(key)
      ? [key, ...[...selectedRowKeys].filter((k) => k !== key)]
      : [key];

  const toggleOne = (key: string, checked: boolean) => {
    const targets = resolveTargetRowKeys(key);
    const current = storedSelectedIds ?? [];
    if (checked) {
      const existing = new Set(current);
      setFileSelectedIds(path, [
        ...current,
        ...targets.filter((k) => !existing.has(k)),
      ]);
    } else {
      const remove = new Set(targets);
      setFileSelectedIds(
        path,
        current.filter((k) => !remove.has(k)),
      );
    }
  };

  const buildCurrentCommand = async (): Promise<string | null> => {
    if (!hasSelection || !activeProfile) {
      return null;
    }
    const outputDir = await resolveOutputDir(path, outputDirOverride);
    const outputPath = await resolveMergeOutputPath(outputDir, path);
    return buildCommandString(
      path,
      outputPath,
      mkvToolNixPath,
      selectedTracks,
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
      setSnackbar({
        message: t("merge.commandCopied"),
        severity: "success",
      });
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleMerge = async () => {
    if (!hasSelection || isActive || !activeProfile) {
      return;
    }
    try {
      await enqueueSelectedTracksForFile({
        file: path,
        selectedTracks,
        profile: activeProfile,
        t,
      });
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleCancel = async () => {
    await cancelMerge(path, (err) =>
      setSnackbar({ message: String(err), severity: "error" }),
    );
  };

  const handleOpenOutputDialog = async () => {
    let initial = "";
    if (outputDirOverride && outputDirOverride.length > 0) {
      initial = outputDirOverride;
    } else {
      try {
        initial = await dirname(path);
      } catch {
        initial = "";
      }
    }
    setOutputDialogInitial(initial);
    setOutputDialogOpen(true);
  };

  const handleOpenInBetterMediaInfo = async () => {
    try {
      await launchBetterMediaInfo([path]);
    } catch (err) {
      setSnackbar({ message: String(err), severity: "error" });
    }
  };

  const handleOutputConfirm = (value: string) => {
    if (value.length === 0) {
      clearFileOutputDir(path);
    } else {
      setFileOutputDir(path, value);
    }
  };

  const handleDelete = async () => {
    const current = useMkvStore.getState().queueItems[path];
    if (current?.status === QueueItemStatus.Merging) {
      return;
    }
    if (current?.status === QueueItemStatus.Waiting) {
      await cancelMerge(path);
      const later = useMkvStore.getState().queueItems[path];
      if (later?.status === QueueItemStatus.Merging) {
        return;
      }
      useMkvStore.getState().removeFromQueue(path);
    }
    removeFile(path);
  };


  const titleContent = (
    <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
      <FileStatusIcon status={entry?.status} />
      <Typography variant="body2" sx={{ wordBreak: "break-all" }}>
        {path}
      </Typography>
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
      sx={{
        mt: 1,
        bgcolor: isQueued ? "action.hover" : undefined,
        borderColor: cardActive ? "primary.main" : undefined,
      }}
    >
      <CardHeader
        title={titleContent}
        subheader={
          <CardSummary
            counts={trackCounts}
            outputPath={outputDirOverride}
          />
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
          tracks={tracks}
          selectedIds={selectedIds}
          selectedRowKeys={selectedRowKeys}
          disabled={isActive}
          loading={loading}
          errorText={error}
          emptyText={t("merge.noTracks")}
          headers={{
            id: t("merge.header.id"),
            type: t("merge.header.type"),
            codec: t("merge.header.codec"),
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
          onTrackLanguageChange={(key, value) =>
            setTrackLanguage([path], resolveTargetRowKeys(key), value)
          }
          onTrackNameChange={(key, value) =>
            setTrackName([path], resolveTargetRowKeys(key), value)
          }
          onCycleFlag={(key, kind) =>
            cycleTrackFlag([path], resolveTargetRowKeys(key), kind)
          }
          onDefaultHeaderClick={() => setDefaultTrackByType([path])}
          onForcedHeaderClick={() => clearForcedFlags([path])}
          onReorder={(from, to) => reorderTracks([path], from, to)}
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
        onConfirm={handleOutputConfirm}
        onClose={() => setOutputDialogOpen(false)}
      />
    </Card>
  );
}
