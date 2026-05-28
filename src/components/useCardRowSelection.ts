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
) {
  const activeCard = useMkvStore((s) => s.activeCard);
  const setActiveCard = useMkvStore((s) => s.setActiveCard);
  const cardActive = activeCard === cardId;

  const [selectedRowKeys, setSelectedRowKeys] = useState<Set<string>>(
    () => new Set(),
  );

  const toggleRowSelection = useCallback((key: string) => {
    setSelectedRowKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  }, []);

  // Keep the latest selection / flip callback in refs so the Space listener
  // doesn't need to re-register on every change.
  const flipRef = useRef(flipMergeSelection);
  flipRef.current = flipMergeSelection;
  const selectedRef = useRef(selectedRowKeys);
  selectedRef.current = selectedRowKeys;

  useEffect(() => {
    if (!cardActive || disabled) {
      return;
    }
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== " " && e.code !== "Space") {
        return;
      }
      const target = e.target as HTMLElement | null;
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
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [cardActive, disabled]);

  const activate = useCallback(
    () => setActiveCard(cardId),
    [setActiveCard, cardId],
  );

  return { cardActive, activate, selectedRowKeys, toggleRowSelection };
}
