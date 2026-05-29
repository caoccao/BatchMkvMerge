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

import { useMkvStore } from "../store";

/** Data attribute marking a single-root card's root element (a valid drop
 *  target) — carries the card's root file path. */
export const CARD_ROOT_ATTR = "data-bmm-card-root";
/** Data attribute marking a multi-root group card's root element — an invalid
 *  drop target (shows the "not allowed" cursor). */
export const GROUP_CARD_ATTR = "data-bmm-group-card";

/**
 * Pointer-event-based drag for merging one single-root card into another.
 *
 * Tauri's OS file-drop (`dragDropEnabled`, on by default and required here for
 * adding files) disables the HTML5 drag-and-drop API inside the webview, so the
 * native `draggable` / `drop` events never fire. We therefore drive the whole
 * gesture with raw pointer events: while the pointer is down we locate the card
 * under the cursor via `elementFromPoint` + data attributes, force a global
 * cursor, mark the hovered card as the drop target (for its highlight), and on
 * release merge the source card's whole tree into the target.
 *
 * Call this from the drag handle's `onPointerDown`.
 */
export function beginCardDrag(sourceRoot: string): void {
  // Force the cursor everywhere — MUI buttons/rows set their own cursor, so a
  // plain `body.style.cursor` would be overridden while hovering them.
  const cursorStyle = document.createElement("style");
  document.head.appendChild(cursorStyle);
  const setCursor = (cursor: string) => {
    cursorStyle.textContent = `* { cursor: ${cursor} !important; }`;
  };
  setCursor("grabbing");

  const prevUserSelect = document.body.style.userSelect;
  document.body.style.userSelect = "none";

  const onMove = (e: PointerEvent) => {
    const el = document.elementFromPoint(e.clientX, e.clientY);
    const single = el?.closest(`[${CARD_ROOT_ATTR}]`) ?? null;
    const group = el?.closest(`[${GROUP_CARD_ATTR}]`) ?? null;
    const targetRoot = single?.getAttribute(CARD_ROOT_ATTR) ?? null;
    if (targetRoot && targetRoot !== sourceRoot) {
      useMkvStore.getState().setDropTarget(targetRoot);
      setCursor("copy");
    } else {
      useMkvStore.getState().setDropTarget(null);
      // A group card (more than one root) or the source card itself is invalid.
      setCursor(group || targetRoot === sourceRoot ? "no-drop" : "grabbing");
    }
  };

  const cleanup = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", cleanup);
    cursorStyle.remove();
    document.body.style.userSelect = prevUserSelect;
    useMkvStore.getState().setDropTarget(null);
  };

  const onUp = () => {
    const target = useMkvStore.getState().dropTargetRoot;
    cleanup();
    if (target && target !== sourceRoot) {
      useMkvStore.getState().mergeCardInto(sourceRoot, target);
    }
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", cleanup);
}
