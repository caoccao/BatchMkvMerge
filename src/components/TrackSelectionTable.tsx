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
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from "@mui/material";
import { trackKey } from "../extract-utils";
import type { MkvTrack } from "../protocol";
import { TrackTypeIcon } from "./TrackTypeIcon";

interface TrackSelectionTableProps {
  tracks: MkvTrack[];
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
  };
  loading?: boolean;
  errorText?: string | null;
  onToggleAll: (checked: boolean) => void;
  onToggleOne: (key: string, checked: boolean) => void;
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
            <TableCell>{headers.number}</TableCell>
            <TableCell>{headers.type}</TableCell>
            <TableCell>{headers.codec}</TableCell>
            <TableCell>{headers.trackName}</TableCell>
            <TableCell>{headers.language}</TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {tracks.map((track) => {
            const key = trackKey(track);
            return (
              <TableRow
                key={track.id}
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
                <TableCell>{track.number}</TableCell>
                <TableCell>
                  <TrackTypeIcon type={track.type} />
                </TableCell>
                <TableCell>{track.codec}</TableCell>
                <TableCell>{track.trackName}</TableCell>
                <TableCell>{track.language}</TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </TableContainer>
  );
}
