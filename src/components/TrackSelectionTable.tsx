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

import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
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
import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  PointerSensor,
  type PointerSensorOptions,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { restrictToVerticalAxis } from "@dnd-kit/modifiers";
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { trackKey } from "../merge";
import { formatTrackBitRate, formatTrackSize } from "../format";
import type { MediaTrack } from "../media-metadata";
import type { ConfigFormatting, TrackFlag } from "../protocol";
import {
  LanguageAutocomplete,
  TitleAutocomplete,
} from "./TrackCellAutocomplete";
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

/** `true` when the pointer landed on an interactive control inside a row, so a
 *  drag must NOT start (the click belongs to the checkbox / flag button). */
function isInteractiveDragTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    target.closest(
      [
        "button",
        "input",
        "select",
        "textarea",
        '[contenteditable="true"]',
        '[role="checkbox"]',
        ".MuiButtonBase-root",
      ].join(","),
    ) !== null
  );
}

/** PointerSensor that ignores pointer-downs on interactive controls, so the
 *  row's checkboxes / flag buttons keep working while the row stays draggable.
 *  Mirrors BetterMediaInfo's reorderable tables. */
class InteractiveSafePointerSensor extends PointerSensor {
  static activators = [
    {
      eventName: "onPointerDown" as const,
      handler: (
        { nativeEvent: event }: ReactPointerEvent,
        { onActivation }: PointerSensorOptions,
      ): boolean => {
        if (
          !event.isPrimary ||
          event.button !== 0 ||
          isInteractiveDragTarget(event.target)
        ) {
          return false;
        }
        onActivation?.({ event });
        return true;
      },
    },
  ];
}

function SortableTableRow({
  id,
  disabled,
  selected,
  onClick,
  children,
}: {
  id: string;
  disabled: boolean;
  selected: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id, disabled });
  return (
    <TableRow
      ref={setNodeRef}
      hover
      selected={selected}
      {...attributes}
      {...listeners}
      onClick={onClick}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
        opacity: isDragging ? 0.4 : undefined,
      }}
      sx={{ cursor: disabled ? "default" : "grab" }}
    >
      {children}
    </TableRow>
  );
}

interface TrackSelectionTableProps {
  tracks: MediaTrack[];
  selectedIds: Set<string>;
  /** UI row-selection (highlight) — independent of the merge checkboxes. */
  selectedRowKeys: Set<string>;
  /** Size / bit-rate display formatting (per stream kind). */
  formatting: ConfigFormatting | null;
  disabled: boolean;
  emptyText: string;
  headers: {
    id: string;
    type: string;
    codec: string;
    size: string;
    bitRate: string;
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
  /** Toggle a row's UI selection (click on the row, away from its controls). */
  onToggleRowSelection: (key: string) => void;
  /** Language dropdown options for a track type (preferred first + the rest). */
  languageOptionsFor: (trackType: string) => {
    options: string[];
    preferredCount: number;
  };
  /** Track-name preset options for a track type + current language. */
  trackNameOptionsFor: (trackType: string, language: string) => string[];
  /** Commit an edited language code for a track row. */
  onTrackLanguageChange: (key: string, value: string) => void;
  /** Commit an edited track name for a track row. */
  onTrackNameChange: (key: string, value: string) => void;
  /** Cycle a track's default/forced flag (true → false → unspecified → true). */
  onCycleFlag: (key: string, kind: "default" | "forced") => void;
  /** Default Track header: make the first video/audio/subtitle track default. */
  onDefaultHeaderClick: () => void;
  /** Forced Display header: reset every track's forced flag. */
  onForcedHeaderClick: () => void;
  /** Drag-reorder: move the dragged row (`fromKey`) to the drop row (`toKey`). */
  onReorder: (fromKey: string, toKey: string) => void;
}

export function TrackSelectionTable({
  tracks,
  selectedIds,
  selectedRowKeys,
  formatting,
  disabled,
  emptyText,
  headers,
  loading = false,
  errorText = null,
  onToggleAll,
  onToggleOne,
  onToggleRowSelection,
  languageOptionsFor,
  trackNameOptionsFor,
  onTrackLanguageChange,
  onTrackNameChange,
  onCycleFlag,
  onDefaultHeaderClick,
  onForcedHeaderClick,
  onReorder,
}: TrackSelectionTableProps) {
  const sensors = useSensors(
    useSensor(InteractiveSafePointerSensor, {
      activationConstraint: { distance: 5 },
    }),
  );
  const sortableIds = tracks.map((track) => trackKey(track));
  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) {
      return;
    }
    onReorder(String(active.id), String(over.id));
  };

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
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      modifiers={[restrictToVerticalAxis]}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={sortableIds} strategy={verticalListSortingStrategy}>
        <TableContainer>
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell padding="checkbox">
                  <Checkbox
                    size="small"
                    disabled={disabled}
                    checked={
                      tracks.length > 0 && selectedIds.size === tracks.length
                    }
                    indeterminate={
                      selectedIds.size > 0 && selectedIds.size < tracks.length
                    }
                    onChange={(event) => onToggleAll(event.target.checked)}
                  />
                </TableCell>
                <TableCell>{headers.id}</TableCell>
                <TableCell>{headers.type}</TableCell>
                <TableCell>{headers.codec}</TableCell>
                <TableCell>{headers.size}</TableCell>
                <TableCell>{headers.bitRate}</TableCell>
                <TableCell>{headers.language}</TableCell>
                <TableCell>{headers.trackName}</TableCell>
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
                  <SortableTableRow
                    key={key}
                    id={key}
                    disabled={disabled}
                    selected={selectedRowKeys.has(key)}
                    onClick={() => onToggleRowSelection(key)}
                  >
                    <TableCell
                      padding="checkbox"
                      onClick={(e) => e.stopPropagation()}
                    >
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
                    <TableCell>
                      {track.size != null
                        ? formatTrackSize(track.size, track.type, formatting)
                        : ""}
                    </TableCell>
                    <TableCell>
                      {track.bitRate != null
                        ? formatTrackBitRate(
                            track.bitRate,
                            track.type,
                            formatting,
                          )
                        : ""}
                    </TableCell>
                    <TableCell
                      onClick={(e) => e.stopPropagation()}
                      sx={{ minWidth: 120 }}
                    >
                      {track.kind === "track" ? (
                        (() => {
                          const { options, preferredCount } =
                            languageOptionsFor(track.type);
                          return (
                            <LanguageAutocomplete
                              value={track.language}
                              options={options}
                              preferredOptionCount={preferredCount}
                              disabled={disabled}
                              onChange={(value) =>
                                onTrackLanguageChange(key, value)
                              }
                            />
                          );
                        })()
                      ) : (
                        track.language
                      )}
                    </TableCell>
                    <TableCell
                      onClick={(e) => e.stopPropagation()}
                      sx={{ minWidth: 140 }}
                    >
                      {track.kind === "track" ? (
                        <TitleAutocomplete
                          value={track.trackName}
                          options={trackNameOptionsFor(
                            track.type,
                            track.language,
                          )}
                          disabled={disabled}
                          onChange={(value) => onTrackNameChange(key, value)}
                        />
                      ) : (
                        track.trackName
                      )}
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
                  </SortableTableRow>
                );
              })}
            </TableBody>
          </Table>
        </TableContainer>
      </SortableContext>
    </DndContext>
  );
}
