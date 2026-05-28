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

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Box,
  Button,
  Checkbox,
  IconButton,
  InputAdornment,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import ClearIcon from "@mui/icons-material/Clear";
import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  pointerWithin,
  rectIntersection,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DragEndEvent,
  type DragOverEvent,
  type DragStartEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useTranslation } from "react-i18next";
import type { MkvLanguage } from "../../mkvLanguages";

// `L:` rows / PREFERRED_CONTAINER_ID belong to the ordered Preferred pane;
// `R:` rows / AVAILABLE_CONTAINER_ID belong to the filterable Available pane.
const PREFERRED_CONTAINER_ID = "language-preferred-container";
const AVAILABLE_CONTAINER_ID = "language-available-container";

// Disable sortable's auto-shifting so the drop indicator is the sole visual cue.
const noShiftStrategy = () => null;

// Pointer-first detection: reflects what the user is actually pointing at, so
// dragging from the Available pane into the Preferred pane resolves to the
// container droppable instead of being pulled back by closestCenter's bias.
const collisionDetection: CollisionDetection = (args) => {
  const pointerHits = pointerWithin(args);
  if (pointerHits.length > 0) {
    return pointerHits;
  }
  const rectHits = rectIntersection(args);
  if (rectHits.length > 0) {
    return rectHits;
  }
  return closestCenter(args);
};

function rowSx(isDragging: boolean) {
  return {
    display: "flex",
    alignItems: "stretch",
    borderBottom: 1,
    borderColor: "divider",
    bgcolor: "background.paper",
    cursor: "grab",
    userSelect: "none",
    opacity: isDragging ? 0.4 : 1,
    "&:hover": { bgcolor: "action.hover" },
  } as const;
}

const checkCellSx = {
  width: 40,
  flex: "0 0 auto",
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
} as const;

const propCellSx = {
  flex: "1 1 0",
  minWidth: 0,
  px: 1,
  py: 0.5,
  display: "flex",
  alignItems: "center",
  fontSize: "0.8125rem",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
} as const;

function DropIndicator({ position }: { position: "top" | "bottom" }) {
  return (
    <Box
      sx={{
        position: "absolute",
        left: 0,
        right: 0,
        [position]: -1,
        height: 2,
        bgcolor: "primary.main",
        zIndex: 5,
        pointerEvents: "none",
      }}
    />
  );
}

function SortablePreferredRow({
  code,
  label,
  checked,
  onCheck,
  isActive,
  showIndicator,
}: {
  code: string;
  label: string;
  checked: boolean;
  onCheck: (next: boolean) => void;
  isActive: boolean;
  showIndicator: boolean;
}) {
  const { attributes, listeners, setNodeRef, transform, transition } =
    useSortable({ id: `L:${code}` });
  return (
    <Box
      ref={setNodeRef}
      {...attributes}
      {...listeners}
      style={{
        transform: CSS.Transform.toString(transform),
        transition,
      }}
      sx={{ ...rowSx(isActive), position: "relative" }}
    >
      {showIndicator && <DropIndicator position="top" />}
      <Box onPointerDown={(e) => e.stopPropagation()} sx={checkCellSx}>
        <Checkbox
          size="small"
          checked={checked}
          onChange={(e) => onCheck(e.target.checked)}
        />
      </Box>
      <Box sx={propCellSx}>{label}</Box>
    </Box>
  );
}

function DraggableAvailableRow({
  code,
  label,
  checked,
  onCheck,
  isActive,
}: {
  code: string;
  label: string;
  checked: boolean;
  onCheck: (next: boolean) => void;
  isActive: boolean;
}) {
  const { attributes, listeners, setNodeRef } = useDraggable({
    id: `R:${code}`,
  });
  return (
    <Box ref={setNodeRef} {...attributes} {...listeners} sx={rowSx(isActive)}>
      <Box onPointerDown={(e) => e.stopPropagation()} sx={checkCellSx}>
        <Checkbox
          size="small"
          checked={checked}
          onChange={(e) => onCheck(e.target.checked)}
        />
      </Box>
      <Box sx={propCellSx}>{label}</Box>
    </Box>
  );
}

function PaneHeader({
  label,
  checked,
  indeterminate,
  onToggle,
  disabled,
}: {
  label: string;
  checked: boolean;
  indeterminate: boolean;
  onToggle: (next: boolean) => void;
  disabled: boolean;
}) {
  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "center",
        bgcolor: "background.paper",
        borderBottom: 1,
        borderColor: "divider",
        position: "sticky",
        top: 0,
        zIndex: 1,
      }}
    >
      <Box sx={checkCellSx}>
        <Checkbox
          size="small"
          disabled={disabled}
          checked={checked}
          indeterminate={indeterminate}
          onChange={(e) => onToggle(e.target.checked)}
        />
      </Box>
      <Box sx={{ ...propCellSx, py: 0.75, fontWeight: 600 }}>{label}</Box>
    </Box>
  );
}

function PreferredDropArea({
  preferredCodes,
  getLabel,
  selection,
  onToggle,
  activeId,
  overId,
  dragSet,
}: {
  preferredCodes: string[];
  getLabel: (code: string) => string;
  selection: Set<string>;
  onToggle: (code: string, next: boolean) => void;
  activeId: string | null;
  overId: string | null;
  dragSet: string[];
}) {
  const { setNodeRef } = useDroppable({ id: PREFERRED_CONTAINER_ID });
  const ids = useMemo(
    () => preferredCodes.map((code) => `L:${code}`),
    [preferredCodes],
  );
  const dragSetSet = useMemo(() => new Set(dragSet), [dragSet]);
  const showContainerIndicator =
    activeId !== null && overId === PREFERRED_CONTAINER_ID;
  return (
    <Box
      ref={setNodeRef}
      sx={{ flex: 1, minHeight: 80, overflow: "auto", position: "relative" }}
    >
      <SortableContext items={ids} strategy={noShiftStrategy}>
        {preferredCodes.map((code) => (
          <SortablePreferredRow
            key={code}
            code={code}
            label={getLabel(code)}
            checked={selection.has(code)}
            onCheck={(next) => onToggle(code, next)}
            isActive={activeId === `L:${code}`}
            showIndicator={
              activeId !== null &&
              overId === `L:${code}` &&
              !dragSetSet.has(code)
            }
          />
        ))}
        {showContainerIndicator && (
          <Box sx={{ position: "relative", height: 2 }}>
            <DropIndicator position="top" />
          </Box>
        )}
      </SortableContext>
    </Box>
  );
}

/**
 * Dual-pane drag-and-drop language picker ported from BetterMediaInfo's
 * MkvLanguagesPanel. Left pane = full Available list (filterable); right pane =
 * ordered Preferred subset. Drag between panes to add/remove, drag within the
 * Preferred pane to reorder.
 */
export function LanguagePicker({
  availableLanguages,
  preferredCodes,
  onPreferredCodesChange,
}: {
  availableLanguages: readonly MkvLanguage[];
  preferredCodes: string[];
  onPreferredCodesChange: (codes: string[]) => void;
}) {
  const { t } = useTranslation();
  const labelByCode = useMemo(
    () => new Map(availableLanguages.map((lang) => [lang.code, lang.label])),
    [availableLanguages],
  );
  const availableLanguageCodes = useMemo(
    () => availableLanguages.map((lang) => lang.code),
    [availableLanguages],
  );
  const getLanguageLabel = useCallback(
    (code: string) => labelByCode.get(code) ?? code,
    [labelByCode],
  );

  const [filter, setFilter] = useState("");
  const [leftSelection, setLeftSelection] = useState<Set<string>>(
    () => new Set(),
  );
  const [rightSelection, setRightSelection] = useState<Set<string>>(
    () => new Set(),
  );
  const [activeId, setActiveId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);
  const dragSetRef = useRef<string[]>([]);

  useEffect(() => {
    setLeftSelection((prev) => {
      if (prev.size === 0) {
        return prev;
      }
      const valid = new Set(preferredCodes);
      const next = new Set<string>();
      let changed = false;
      prev.forEach((code) => {
        if (valid.has(code)) {
          next.add(code);
        } else {
          changed = true;
        }
      });
      return changed ? next : prev;
    });
  }, [preferredCodes]);

  useEffect(() => {
    setRightSelection((prev) => {
      if (prev.size === 0) {
        return prev;
      }
      const valid = new Set(availableLanguageCodes);
      const next = new Set<string>();
      let changed = false;
      prev.forEach((code) => {
        if (valid.has(code)) {
          next.add(code);
        } else {
          changed = true;
        }
      });
      return changed ? next : prev;
    });
  }, [availableLanguageCodes]);

  const filteredAvailableLanguageCodes = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) {
      return availableLanguageCodes;
    }
    return availableLanguages
      .filter(
        (lang) =>
          lang.label.toLowerCase().includes(f) ||
          lang.code.toLowerCase().includes(f),
      )
      .map((lang) => lang.code);
  }, [availableLanguages, availableLanguageCodes, filter]);

  const handleToggleLeft = useCallback((code: string, next: boolean) => {
    setLeftSelection((prev) => {
      const updated = new Set(prev);
      if (next) {
        updated.add(code);
      } else {
        updated.delete(code);
      }
      return updated;
    });
  }, []);

  const handleToggleRight = useCallback((code: string, next: boolean) => {
    setRightSelection((prev) => {
      const updated = new Set(prev);
      if (next) {
        updated.add(code);
      } else {
        updated.delete(code);
      }
      return updated;
    });
  }, []);

  const handleToggleAllLeft = useCallback(
    (next: boolean) => {
      setLeftSelection((prev) => {
        const updated = new Set(prev);
        if (next) {
          preferredCodes.forEach((code) => updated.add(code));
        } else {
          preferredCodes.forEach((code) => updated.delete(code));
        }
        return updated;
      });
    },
    [preferredCodes],
  );

  const handleToggleAllRight = useCallback(
    (next: boolean) => {
      setRightSelection((prev) => {
        const updated = new Set(prev);
        if (next) {
          filteredAvailableLanguageCodes.forEach((code) => updated.add(code));
        } else {
          filteredAvailableLanguageCodes.forEach((code) =>
            updated.delete(code),
          );
        }
        return updated;
      });
    },
    [filteredAvailableLanguageCodes],
  );

  const handleAddAll = useCallback(() => {
    if (filteredAvailableLanguageCodes.length === 0) {
      return;
    }
    const existing = new Set(preferredCodes);
    const toAdd = filteredAvailableLanguageCodes.filter(
      (code) => !existing.has(code),
    );
    if (toAdd.length === 0) {
      return;
    }
    onPreferredCodesChange([...preferredCodes, ...toAdd]);
  }, [filteredAvailableLanguageCodes, preferredCodes, onPreferredCodesChange]);

  const handleRemoveAll = useCallback(() => {
    if (preferredCodes.length === 0) {
      return;
    }
    onPreferredCodesChange([]);
  }, [preferredCodes.length, onPreferredCodesChange]);

  const rightPaneRef = useRef<HTMLDivElement | null>(null);
  const { setNodeRef: setRightDropNode } = useDroppable({
    id: AVAILABLE_CONTAINER_ID,
  });
  const attachRightPaneRef = useCallback(
    (node: HTMLDivElement | null) => {
      rightPaneRef.current = node;
      setRightDropNode(node);
    },
    [setRightDropNode],
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const resetDrag = useCallback(() => {
    setActiveId(null);
    setOverId(null);
    dragSetRef.current = [];
  }, []);

  const handleDragStart = useCallback(
    (event: DragStartEvent) => {
      const id = String(event.active.id);
      const source = id.startsWith("L:") ? "left" : "right";
      const code = id.slice(2);
      if (source === "left") {
        if (leftSelection.size > 0 && leftSelection.has(code)) {
          dragSetRef.current = preferredCodes.filter((c) =>
            leftSelection.has(c),
          );
        } else {
          dragSetRef.current = [code];
        }
      } else {
        if (rightSelection.size > 0 && rightSelection.has(code)) {
          dragSetRef.current = availableLanguageCodes.filter((c) =>
            rightSelection.has(c),
          );
        } else {
          dragSetRef.current = [code];
        }
      }
      setActiveId(id);
    },
    [leftSelection, rightSelection, preferredCodes, availableLanguageCodes],
  );

  const handleDragOver = useCallback((event: DragOverEvent) => {
    setOverId(event.over ? String(event.over.id) : null);
  }, []);

  const handleDragEnd = useCallback(
    (event: DragEndEvent) => {
      const { active, over, activatorEvent, delta } = event;
      const activeIdStr = String(active.id);
      const source = activeIdStr.startsWith("L:") ? "left" : "right";
      const dragSet = dragSetRef.current.slice();
      const droppedOverId = over ? String(over.id) : null;
      const overInLeft =
        droppedOverId === PREFERRED_CONTAINER_ID ||
        (droppedOverId?.startsWith("L:") ?? false);

      let droppedOnRightPane = droppedOverId === AVAILABLE_CONTAINER_ID;
      if (!droppedOnRightPane && rightPaneRef.current) {
        const evt = activatorEvent as
          | { clientX?: number; clientY?: number }
          | undefined;
        if (
          evt &&
          typeof evt.clientX === "number" &&
          typeof evt.clientY === "number"
        ) {
          const dropX = evt.clientX + delta.x;
          const dropY = evt.clientY + delta.y;
          const rect = rightPaneRef.current.getBoundingClientRect();
          droppedOnRightPane =
            dropX >= rect.left &&
            dropX <= rect.right &&
            dropY >= rect.top &&
            dropY <= rect.bottom;
        }
      }

      resetDrag();

      if (dragSet.length === 0) {
        return;
      }

      if (source === "right") {
        if (!overInLeft || droppedOnRightPane) {
          return;
        }
        let targetIndex = preferredCodes.length;
        if (droppedOverId && droppedOverId.startsWith("L:")) {
          const overCode = droppedOverId.slice(2);
          const idx = preferredCodes.indexOf(overCode);
          targetIndex = idx >= 0 ? idx : preferredCodes.length;
        }
        const existing = new Set(preferredCodes);
        const toAdd = dragSet.filter((code) => !existing.has(code));
        if (toAdd.length === 0) {
          return;
        }
        onPreferredCodesChange([
          ...preferredCodes.slice(0, targetIndex),
          ...toAdd,
          ...preferredCodes.slice(targetIndex),
        ]);
        setRightSelection(new Set());
        return;
      }

      if (droppedOnRightPane) {
        const remove = new Set(dragSet);
        const next = preferredCodes.filter((code) => !remove.has(code));
        if (next.length === preferredCodes.length) {
          return;
        }
        onPreferredCodesChange(next);
        setLeftSelection(new Set());
        return;
      }
      if (!overInLeft) {
        return;
      }

      let targetIndex = preferredCodes.length;
      if (droppedOverId && droppedOverId.startsWith("L:")) {
        const overCode = droppedOverId.slice(2);
        if (dragSet.includes(overCode)) {
          return;
        }
        const idx = preferredCodes.indexOf(overCode);
        if (idx < 0) {
          return;
        }
        targetIndex = idx;
      }
      const dragSetMembers = new Set(dragSet);
      const remaining = preferredCodes.filter(
        (code) => !dragSetMembers.has(code),
      );
      const removedBefore = preferredCodes
        .slice(0, targetIndex)
        .reduce((acc, code) => (dragSetMembers.has(code) ? acc + 1 : acc), 0);
      const adjusted = targetIndex - removedBefore;
      const next = [
        ...remaining.slice(0, adjusted),
        ...dragSet,
        ...remaining.slice(adjusted),
      ];
      if (
        next.length === preferredCodes.length &&
        next.every((code, i) => code === preferredCodes[i])
      ) {
        return;
      }
      onPreferredCodesChange(next);
    },
    [preferredCodes, onPreferredCodesChange, resetDrag],
  );

  const leftCheckedCount = useMemo(
    () =>
      preferredCodes.reduce(
        (acc, code) => (leftSelection.has(code) ? acc + 1 : acc),
        0,
      ),
    [preferredCodes, leftSelection],
  );
  const leftAllChecked =
    preferredCodes.length > 0 && leftCheckedCount === preferredCodes.length;
  const leftSomeChecked =
    leftCheckedCount > 0 && leftCheckedCount < preferredCodes.length;

  const filteredAvailableSize = filteredAvailableLanguageCodes.length;
  const rightCheckedInFiltered = useMemo(
    () =>
      filteredAvailableLanguageCodes.reduce(
        (acc, code) => (rightSelection.has(code) ? acc + 1 : acc),
        0,
      ),
    [filteredAvailableLanguageCodes, rightSelection],
  );
  const rightAllChecked =
    filteredAvailableSize > 0 && rightCheckedInFiltered === filteredAvailableSize;
  const rightSomeChecked =
    rightCheckedInFiltered > 0 && rightCheckedInFiltered < filteredAvailableSize;

  const overlayCode = activeId ? activeId.slice(2) : null;
  const overlayCount = activeId ? dragSetRef.current.length : 0;

  return (
    <Box
      sx={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}
    >
      <Box sx={{ display: "flex", gap: 1, mb: 1, alignItems: "center" }}>
        <TextField
          size="small"
          placeholder={t("settings.filter")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          sx={{ flex: 1 }}
          slotProps={{
            input: {
              endAdornment: filter ? (
                <InputAdornment position="end">
                  <Tooltip title={t("settings.clear")}>
                    <IconButton
                      size="small"
                      aria-label={t("settings.clear")}
                      onClick={() => setFilter("")}
                      edge="end"
                    >
                      <ClearIcon fontSize="small" />
                    </IconButton>
                  </Tooltip>
                </InputAdornment>
              ) : null,
            },
          }}
        />
        <Button
          variant="outlined"
          size="small"
          onClick={handleAddAll}
          disabled={filteredAvailableLanguageCodes.length === 0}
          sx={{ textTransform: "none", whiteSpace: "nowrap", height: 36 }}
        >
          {t("settings.addAll")}
        </Button>
        <Button
          variant="outlined"
          size="small"
          onClick={handleRemoveAll}
          disabled={preferredCodes.length === 0}
          sx={{ textTransform: "none", whiteSpace: "nowrap", height: 36 }}
        >
          {t("settings.removeAll")}
        </Button>
      </Box>
      <DndContext
        sensors={sensors}
        collisionDetection={collisionDetection}
        onDragStart={handleDragStart}
        onDragOver={handleDragOver}
        onDragEnd={handleDragEnd}
        onDragCancel={resetDrag}
      >
        <Box sx={{ display: "flex", gap: 1, flex: 1, minHeight: 0 }}>
          <Box
            ref={attachRightPaneRef}
            sx={{
              flex: 1,
              minWidth: 0,
              display: "flex",
              flexDirection: "column",
              border: 1,
              borderColor: "divider",
              borderRadius: 1,
              overflow: "hidden",
              bgcolor:
                activeId?.startsWith("L:") && overId === AVAILABLE_CONTAINER_ID
                  ? "action.selected"
                  : undefined,
              transition: "background-color 120ms",
            }}
          >
            <PaneHeader
              label={t("settings.available")}
              checked={rightAllChecked}
              indeterminate={rightSomeChecked}
              onToggle={handleToggleAllRight}
              disabled={filteredAvailableSize === 0}
            />
            <Box sx={{ flex: 1, overflow: "auto" }}>
              {filteredAvailableLanguageCodes.map((code) => (
                <DraggableAvailableRow
                  key={code}
                  code={code}
                  label={getLanguageLabel(code)}
                  checked={rightSelection.has(code)}
                  onCheck={(next) => handleToggleRight(code, next)}
                  isActive={activeId === `R:${code}`}
                />
              ))}
            </Box>
          </Box>
          <Box
            sx={{
              flex: 1,
              minWidth: 0,
              display: "flex",
              flexDirection: "column",
              border: 1,
              borderColor: "divider",
              borderRadius: 1,
              overflow: "hidden",
            }}
          >
            <PaneHeader
              label={t("settings.preferred")}
              checked={leftAllChecked}
              indeterminate={leftSomeChecked}
              onToggle={handleToggleAllLeft}
              disabled={preferredCodes.length === 0}
            />
            <PreferredDropArea
              preferredCodes={preferredCodes}
              getLabel={getLanguageLabel}
              selection={leftSelection}
              onToggle={handleToggleLeft}
              activeId={activeId}
              overId={overId}
              dragSet={dragSetRef.current}
            />
          </Box>
        </Box>
        <DragOverlay dropAnimation={null}>
          {overlayCode ? (
            <Box
              sx={{
                display: "inline-flex",
                alignItems: "center",
                gap: 1,
                px: 1,
                py: 0.5,
                border: 1,
                borderColor: "divider",
                borderRadius: 1,
                bgcolor: "background.paper",
                boxShadow: 4,
                fontSize: "0.8125rem",
              }}
            >
              <span>{getLanguageLabel(overlayCode)}</span>
              {overlayCount > 1 && (
                <Typography variant="caption" color="text.secondary">
                  +{overlayCount - 1}
                </Typography>
              )}
            </Box>
          ) : null}
        </DragOverlay>
      </DndContext>
      <Typography
        variant="caption"
        color="text.secondary"
        sx={{ mt: 1, display: "block", textAlign: "center" }}
      >
        {t("settings.languagesInstruction")}
      </Typography>
    </Box>
  );
}
