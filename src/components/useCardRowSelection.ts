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

import { useCallback, useEffect, useRef, useState } from "react";
import { useMkvStore } from "../store";

/**
 * Per-card UI row selection plus active-card wiring. Row selection is purely a
 * UI highlight — independent of the merge checkboxes. While the card is active,
 * pressing Space flips the merge checkbox of every selected row through the
 * card-supplied `flipMergeSelection` callback.
 */
export function useCardRowSelection(
  cardId: string,
  disabled: boolean,
  flipMergeSelection: (keys: string[]) => void,
  allRowKeys: string[],
) {
  const activeCard = useMkvStore((s) => s.activeCard);
  const setActiveCard = useMkvStore((s) => s.setActiveCard);
  const cardActive = activeCard === cardId;

  const [selectedRowKeys, setSelectedRowKeys] = useState<Set<string>>(
    () => new Set(),
  );
  // The keyboard cursor — the "current row" drawn with a dotted outline. Arrow
  // keys move it; a click moves it to the clicked row.
  const [cursorKey, setCursorKey] = useState<string | null>(null);
  const cursorRef = useRef<string | null>(cursorKey);
  cursorRef.current = cursorKey;

  const invertRowSelection = (key: string) =>
    setSelectedRowKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });

  const toggleRowSelection = useCallback((key: string) => {
    setCursorKey(key);
    invertRowSelection(key);
  }, []);

  // Keep the latest selection / flip callback in refs so the Space listener
  // doesn't need to re-register on every change.
  const flipRef = useRef(flipMergeSelection);
  flipRef.current = flipMergeSelection;
  const selectedRef = useRef(selectedRowKeys);
  selectedRef.current = selectedRowKeys;
  const allKeysRef = useRef(allRowKeys);
  allKeysRef.current = allRowKeys;

  useEffect(() => {
    if (!cardActive || disabled) {
      return;
    }
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const inEditable = !!target?.closest(
        'input, textarea, [contenteditable="true"]',
      );

      // Ctrl/Cmd+A selects all rows; Ctrl/Cmd+Shift+A deselects all. Skipped
      // inside text fields so they keep their own select-all behaviour.
      if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.key === "A")) {
        if (inEditable) {
          return;
        }
        e.preventDefault();
        setSelectedRowKeys(
          e.shiftKey ? new Set() : new Set(allKeysRef.current),
        );
        return;
      }

      // Arrow up/down move the cursor and invert the row selection of the row
      // it lands on. The merge checkbox is untouched (Space toggles that).
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        if (inEditable) {
          return;
        }
        const keys = allKeysRef.current;
        if (keys.length === 0) {
          return;
        }
        e.preventDefault();
        const current = cursorRef.current;
        let idx = current ? keys.indexOf(current) : -1;
        if (e.key === "ArrowDown") {
          idx = idx < 0 ? 0 : Math.min(keys.length - 1, idx + 1);
        } else {
          idx = idx < 0 ? keys.length - 1 : Math.max(0, idx - 1);
        }
        const nextKey = keys[idx];
        setCursorKey(nextKey);
        invertRowSelection(nextKey);
        return;
      }

      // Space toggles the checkboxes of the selected rows.
      if (e.key === " " || e.code === "Space") {
        if (
          target &&
          target.closest(
            'input, textarea, button, select, [role="checkbox"], [contenteditable="true"]',
          )
        ) {
          return;
        }
        if (selectedRef.current.size === 0) {
          return;
        }
        e.preventDefault();
        flipRef.current([...selectedRef.current]);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [cardActive, disabled]);

  const activate = useCallback(
    () => setActiveCard(cardId),
    [setActiveCard, cardId],
  );

  return {
    cardActive,
    activate,
    selectedRowKeys,
    toggleRowSelection,
    cursorKey,
  };
}
