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

import assert from "node:assert/strict";
import test from "node:test";
import { numericWordBoundary } from "../src/input-word-navigation.ts";

for (const separator of [".", ","]) {
  const value = `123${separator}456`;

  test(`Windows splits ${value} into two words`, () => {
    assert.equal(numericWordBoundary(value, 0, "right", false), 4);
    assert.equal(numericWordBoundary(value, 7, "left", false), 4);
    // Once in the last/first part, native navigation finishes the movement.
    assert.equal(numericWordBoundary(value, 4, "right", false), null);
    assert.equal(numericWordBoundary(value, 4, "left", false), null);
  });

  test(`macOS splits ${value} into two words`, () => {
    assert.equal(numericWordBoundary(value, 0, "right", true), 3);
    assert.equal(numericWordBoundary(value, 7, "left", true), 4);
    assert.equal(numericWordBoundary(value, 3, "right", true), null);
    assert.equal(numericWordBoundary(value, 4, "left", true), null);
  });

  test(`navigation works inside and alongside ${value}'s separator`, () => {
    assert.equal(numericWordBoundary(value, 1, "right", false), 4);
    assert.equal(numericWordBoundary(value, 3, "right", false), 4);
    assert.equal(numericWordBoundary(value, 6, "left", false), 4);
    assert.equal(numericWordBoundary(value, 3, "left", false), null);
    assert.equal(numericWordBoundary(value, 2, "right", true), 3);
  });
}

test("repeated and mixed numeric separators each add a stop", () => {
  const value = "1,234.567";
  assert.equal(numericWordBoundary(value, 0, "right", false), 2);
  assert.equal(numericWordBoundary(value, 2, "right", false), 6);
  assert.equal(numericWordBoundary(value, 0, "right", true), 1);
  assert.equal(numericWordBoundary(value, 1, "right", true), 5);
  assert.equal(numericWordBoundary(value, 9, "left", false), 6);
  assert.equal(numericWordBoundary(value, 6, "left", false), 2);
});

test("spaces retain native platform behavior and newlines remain native", () => {
  assert.equal(numericWordBoundary(" 123.456", 0, "right", false), null);
  assert.equal(numericWordBoundary(" 123.456", 0, "right", true), 4);
  assert.equal(numericWordBoundary("123.456  ", 9, "left", false), 4);
  assert.equal(numericWordBoundary("123.456\n", 8, "left", false), null);
  assert.equal(numericWordBoundary("\n123.456", 0, "right", true), null);
});

test("ordinary text and punctuation continue to use native navigation", () => {
  for (const value of ["", "123456", "abc.def", "can't", "你好世界", "123..456"]) {
    for (let caret = 0; caret <= value.length; caret++) {
      for (const direction of ["left", "right"]) {
        for (const isMac of [false, true]) {
          assert.equal(numericWordBoundary(value, caret, direction, isMac), null);
        }
      }
    }
  }
  // Do not skip another word or punctuation to reach a numeric separator.
  assert.equal(numericWordBoundary("hello 123.456", 0, "right", false), null);
  assert.equal(numericWordBoundary("abc.def123.456", 0, "right", false), null);
  assert.equal(numericWordBoundary("abc.def123.456", 4, "right", false), 11);
  assert.equal(numericWordBoundary("123.456.abc", 11, "left", false), null);
});

test("Unicode numbers and UTF-16 caret offsets are supported", () => {
  assert.equal(numericWordBoundary("字幕 １２３.４５６", 3, "right", false), 7);
  assert.equal(numericWordBoundary("字幕 １２３.４５６", 10, "left", true), 7);
  assert.equal(numericWordBoundary("🎬 123.456", 3, "right", false), 7);
  assert.equal(numericWordBoundary("𝟙𝟚.𝟛𝟜", 9, "left", false), 5);
});
