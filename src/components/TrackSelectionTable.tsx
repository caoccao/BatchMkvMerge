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

import {
  Box,
  Checkbox,
  CircularProgress,
  IconButton,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Tooltip,
  Typography,
} from "@mui/material";
import CheckIcon from "@mui/icons-material/Check";
import CheckBoxOutlineBlankIcon from "@mui/icons-material/CheckBoxOutlineBlank";
import CloseIcon from "@mui/icons-material/Close";
import CameraRollIcon from "@mui/icons-material/CameraRoll";
import CycloneIcon from "@mui/icons-material/Cyclone";
import { trackKey } from "../merge";
import type { MediaTrack } from "../media-metadata";
import type { TrackFlag } from "../protocol";
import { TrackTypeIcon } from "./TrackTypeIcon";

/** Render a tri-state flag as an icon: green check / red cross / blank square. */
function flagIcon(flag: TrackFlag) {
  if (flag === "true") {
    return <CheckIcon fontSize="small" color="success" />;
  }
  if (flag === "false") {
    return <CloseIcon fontSize="small" color="error" />;
  }
  return <CheckBoxOutlineBlankIcon fontSize="small" color="disabled" />;
}

interface TrackSelectionTableProps {
  tracks: MediaTrack[];
  selectedIds: Set<string>;
  disabled: boolean;
  emptyText: string;
  headers: {
    id: string;
    number: string;
    type: string;
    codec: string;
    trackName: string;
    language: string;
    /** Tooltip text for the icon-only "default track" column. */
    defaultTrack: string;
    /** Tooltip text for the icon-only "forced display" column. */
    forcedDisplay: string;
  };
  loading?: boolean;
  errorText?: string | null;
  onToggleAll: (checked: boolean) => void;
  onToggleOne: (key: string, checked: boolean) => void;
  /** Cycle a track's default/forced flag (true → false → unspecified → true). */
  onCycleFlag: (key: string, kind: "default" | "forced") => void;
  /** Default Track header: make the first video/audio/subtitle track default. */
  onDefaultHeaderClick: () => void;
  /** Forced Display header: reset every track's forced flag. */
  onForcedHeaderClick: () => void;
}

export function TrackSelectionTable({
  tracks,
  selectedIds,
  disabled,
  emptyText,
  headers,
  loading = false,
  errorText = null,
  onToggleAll,
  onToggleOne,
  onCycleFlag,
  onDefaultHeaderClick,
  onForcedHeaderClick,
}: TrackSelectionTableProps) {
  if (loading) {
    return (
      <Box sx={{ display: "flex", justifyContent: "center", py: 1 }}>
        <CircularProgress size={20} />
      </Box>
    );
  }
  if (errorText) {
    return (
      <Typography variant="body2" color="error">
        {errorText}
      </Typography>
    );
  }
  if (tracks.length === 0) {
    return (
      <Typography variant="body2" color="text.secondary" sx={{ p: 1 }}>
        {emptyText}
      </Typography>
    );
  }
  return (
    <TableContainer>
      <Table size="small">
        <TableHead>
          <TableRow>
            <TableCell padding="checkbox">
              <Checkbox
                size="small"
                disabled={disabled}
                checked={tracks.length > 0 && selectedIds.size === tracks.length}
                indeterminate={
                  selectedIds.size > 0 && selectedIds.size < tracks.length
                }
                onChange={(event) => onToggleAll(event.target.checked)}
              />
            </TableCell>
            <TableCell>{headers.id}</TableCell>
            <TableCell>{headers.type}</TableCell>
            <TableCell>{headers.codec}</TableCell>
            <TableCell>{headers.trackName}</TableCell>
            <TableCell>{headers.language}</TableCell>
            <TableCell>{headers.number}</TableCell>
            <TableCell padding="checkbox" align="center">
              <Tooltip title={headers.defaultTrack}>
                <span>
                  <IconButton
                    size="small"
                    disabled={disabled}
                    onClick={onDefaultHeaderClick}
                    sx={{ p: 0.25 }}
                  >
                    <CameraRollIcon fontSize="small" />
                  </IconButton>
                </span>
              </Tooltip>
            </TableCell>
            <TableCell padding="checkbox" align="center">
              <Tooltip title={headers.forcedDisplay}>
                <span>
                  <IconButton
                    size="small"
                    disabled={disabled}
                    onClick={onForcedHeaderClick}
                    sx={{ p: 0.25 }}
                  >
                    <CycloneIcon fontSize="small" />
                  </IconButton>
                </span>
              </Tooltip>
            </TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {tracks.map((track) => {
            const key = trackKey(track);
            return (
              <TableRow
                key={key}
                hover
                sx={{ cursor: disabled ? "default" : "pointer" }}
                onClick={() => {
                  if (disabled) {
                    return;
                  }
                  onToggleOne(key, !selectedIds.has(key));
                }}
              >
                <TableCell padding="checkbox" onClick={(e) => e.stopPropagation()}>
                  <Checkbox
                    size="small"
                    disabled={disabled}
                    checked={selectedIds.has(key)}
                    onChange={(e) => onToggleOne(key, e.target.checked)}
                  />
                </TableCell>
                <TableCell>{track.id}</TableCell>
                <TableCell>
                  <TrackTypeIcon type={track.type} />
                </TableCell>
                <TableCell>{track.codec}</TableCell>
                <TableCell>{track.trackName}</TableCell>
                <TableCell>{track.language}</TableCell>
                <TableCell>{track.number}</TableCell>
                <TableCell
                  padding="checkbox"
                  align="center"
                  onClick={(e) => e.stopPropagation()}
                >
                  {track.kind === "track" ? (
                    <IconButton
                      size="small"
                      disabled={disabled}
                      onClick={() => onCycleFlag(key, "default")}
                      sx={{ p: 0.25 }}
                    >
                      {flagIcon(track.defaultTrack)}
                    </IconButton>
                  ) : null}
                </TableCell>
                <TableCell
                  padding="checkbox"
                  align="center"
                  onClick={(e) => e.stopPropagation()}
                >
                  {track.kind === "track" ? (
                    <IconButton
                      size="small"
                      disabled={disabled}
                      onClick={() => onCycleFlag(key, "forced")}
                      sx={{ p: 0.25 }}
                    >
                      {flagIcon(track.forced)}
                    </IconButton>
                  ) : null}
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </TableContainer>
  );
}
