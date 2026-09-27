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

const wordSegmenter = new Intl.Segmenter(undefined, { granularity: "word" });

/** Return an extra stop inside a number, or let the webview navigate normally. */
export function numericWordBoundary(
  value: string,
  caret: number,
  direction: "left" | "right",
  isMac: boolean,
): number | null {
  const left = direction === "left";
  let index = left ? caret - 1 : caret;
  // Native backward navigation skips spaces; macOS also skips them forward.
  // Leave line breaks to the webview's multiline navigation.
  if (left || isMac) {
    while (
      index >= 0 &&
      index < value.length &&
      /[^\S\r\n]/u.test(value[index])
    ) {
      index += left ? -1 : 1;
    }
  }
  const word = wordSegmenter.segment(value).containing(index);
  if (!word?.isWordLike) {
    return null;
  }

  let nearest: number | null = null;
  let numeric = false;
  for (const separator of word.segment.matchAll(/[.,]/gu)) {
    const offset = separator.index;
    const splitsNumber =
      /\p{N}$/u.test(word.segment.slice(0, offset)) &&
      /^\p{N}/u.test(word.segment.slice(offset + 1));
    const before = word.index + offset;
    // Each numeric part is a word: backward stops at its start, while macOS
    // forward navigation stops at its end. Windows moves to the next start.
    // Other punctuation must still stop native navigation before our target.
    const boundaries = splitsNumber
      ? [before + (left || !isMac ? 1 : 0)]
      : [before, before + 1];
    for (const boundary of boundaries) {
      if (
        left
          ? boundary < caret && (nearest === null || boundary > nearest)
          : boundary > caret && (nearest === null || boundary < nearest)
      ) {
        nearest = boundary;
        numeric = splitsNumber;
      }
    }
  }
  return numeric ? nearest : null;
}

/** Shared by every input and textarea, including fields rendered in dialogs. */
export function handleInputWordNavigation(event: KeyboardEvent): void {
  if (
    event.defaultPrevented ||
    event.isComposing ||
    event.metaKey ||
    (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
  ) {
    return;
  }
  const isMac = /Mac/i.test(navigator.platform);
  if (isMac ? !event.altKey || event.ctrlKey : !event.ctrlKey || event.altKey) {
    return;
  }
  const input = event.target;
  if (
    !(input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement)
  ) {
    return;
  }
  const { selectionStart: start, selectionEnd: end, selectionDirection } = input;
  // Number/date/etc. inputs do not expose a text selection.
  if (input.disabled || start === null || end === null) {
    return;
  }
  const left = event.key === "ArrowLeft";
  const caret = event.shiftKey
    ? selectionDirection === "backward"
      ? start
      : end
    : left
      ? start
      : end;
  const boundary = numericWordBoundary(
    input.value,
    caret,
    left ? "left" : "right",
    isMac,
  );
  if (boundary === null) {
    return;
  }
  event.preventDefault();
  if (event.shiftKey) {
    const anchor = selectionDirection === "backward" ? end : start;
    input.setSelectionRange(
      Math.min(anchor, boundary),
      Math.max(anchor, boundary),
      boundary < anchor ? "backward" : "forward",
    );
  } else {
    input.setSelectionRange(boundary, boundary);
  }
}
