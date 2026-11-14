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

//! Ctrl+Arrow (Windows, Linux) / Option+Arrow (macOS) stops inside numbers:
//! `123.456` and `1,234.567` are one word to the OS, but users expect each
//! numeric part to be a word. Port of the React app's
//! `input-word-navigation.ts`; Phase 11 wires it to the text inputs.
//!
//! Offsets are UTF-8 byte offsets into the text (GPUI's input offsets); the
//! React app used UTF-16 offsets. Words are UAX #29 word segments, as with
//! `Intl.Segmenter`.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// The arrow key pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
  Left,
  Right,
}

/// Whitespace other than CR and LF.
fn is_horizontal_space(c: char) -> bool {
  c.is_whitespace() && c != '\r' && c != '\n'
}

/// The char at byte offset `index`, if `index` is a char boundary inside the
/// text.
fn char_at(value: &str, index: usize) -> Option<char> {
  value.get(index..).and_then(|rest| rest.chars().next())
}

/// The word segment containing byte offset `index`, with its start.
fn segment_containing(value: &str, index: usize) -> Option<(usize, &str)> {
  value
    .split_word_bound_indices()
    .find(|(start, segment)| (*start..start + segment.len()).contains(&index))
}

/// The extra stop inside a number for a word jump from `caret`, or `None` to
/// let the input's own word navigation run.
///
/// Backward navigation skips spaces first (macOS forward does too), line
/// breaks are left alone, and only a stop at a `.` or `,` between two digits
/// counts: backward it lands on the next part's start; forward it lands
/// after the separator on Windows and before it on macOS. Other punctuation in
/// the same word must not be skipped to reach such a stop.
pub fn numeric_word_boundary(value: &str, caret: usize, direction: Direction, is_mac: bool) -> Option<usize> {
  let left = direction == Direction::Left;
  // `index` walks chars: the one before the caret going left, at it going right.
  let mut index = if left {
    value.get(..caret)?.char_indices().next_back()?.0
  } else {
    caret
  };
  if left || is_mac {
    while let Some(c) = char_at(value, index).filter(|c| is_horizontal_space(*c)) {
      if left {
        index = value[..index].char_indices().next_back()?.0;
      } else {
        index += c.len_utf8();
      }
    }
  }
  let (word_start, word) = segment_containing(value, index)?;
  if !word.chars().any(char::is_alphanumeric) {
    return None;
  }

  let mut nearest: Option<usize> = None;
  let mut numeric = false;
  for (offset, _) in word.match_indices(['.', ',']) {
    let splits_number = word[..offset].chars().next_back().is_some_and(char::is_numeric)
      && word[offset + 1..].chars().next().is_some_and(char::is_numeric);
    let before = word_start + offset;
    let boundaries: &[usize] = if splits_number {
      &[before + usize::from(left || !is_mac)]
    } else {
      &[before, before + 1]
    };
    for &boundary in boundaries {
      let closer = if left {
        boundary < caret && nearest.is_none_or(|nearest| boundary > nearest)
      } else {
        boundary > caret && nearest.is_none_or(|nearest| boundary < nearest)
      };
      if closer {
        nearest = Some(boundary);
        numeric = splits_number;
      }
    }
  }
  if numeric { nearest } else { None }
}

/// A text selection: `range` in byte offsets, `reversed` when the caret is at
/// its start (a backward selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
  pub range: Range<usize>,
  pub reversed: bool,
}

/// The selection after a word jump that [`numeric_word_boundary`] redirects:
/// collapsed at the stop, or with Shift extended from the anchor to it.
/// `None` when the native jump should run.
pub fn navigate(
  value: &str,
  selection: &Selection,
  direction: Direction,
  extend: bool,
  is_mac: bool,
) -> Option<Selection> {
  let Selection { range, reversed } = selection;
  let caret = match (extend, direction) {
    (true, _) if *reversed => range.start,
    (true, _) => range.end,
    (false, Direction::Left) => range.start,
    (false, Direction::Right) => range.end,
  };
  let boundary = numeric_word_boundary(value, caret, direction, is_mac)?;
  if !extend {
    return Some(Selection {
      range: boundary..boundary,
      reversed: false,
    });
  }
  let anchor = if *reversed { range.end } else { range.start };
  Some(Selection {
    range: anchor.min(boundary)..anchor.max(boundary),
    reversed: boundary < anchor,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  // The cases of the React app's `tests/input-word-navigation.test.mjs`, one
  // for one, in byte offsets.

  fn nav(value: &str, caret: usize, direction: &str, is_mac: bool) -> Option<usize> {
    let direction = if direction == "left" {
      Direction::Left
    } else {
      Direction::Right
    };
    numeric_word_boundary(value, caret, direction, is_mac)
  }

  #[test]
  fn windows_splits_a_number_into_two_words() {
    for separator in [".", ","] {
      let value = format!("123{separator}456");
      assert_eq!(nav(&value, 0, "right", false), Some(4));
      assert_eq!(nav(&value, 7, "left", false), Some(4));
      // Once in the last/first part, native navigation finishes the movement.
      assert_eq!(nav(&value, 4, "right", false), None);
      assert_eq!(nav(&value, 4, "left", false), None);
    }
  }

  #[test]
  fn macos_splits_a_number_into_two_words() {
    for separator in [".", ","] {
      let value = format!("123{separator}456");
      assert_eq!(nav(&value, 0, "right", true), Some(3));
      assert_eq!(nav(&value, 7, "left", true), Some(4));
      assert_eq!(nav(&value, 3, "right", true), None);
      assert_eq!(nav(&value, 4, "left", true), None);
    }
  }

  #[test]
  fn navigation_works_inside_and_alongside_the_separator() {
    for separator in [".", ","] {
      let value = format!("123{separator}456");
      assert_eq!(nav(&value, 1, "right", false), Some(4));
      assert_eq!(nav(&value, 3, "right", false), Some(4));
      assert_eq!(nav(&value, 6, "left", false), Some(4));
      assert_eq!(nav(&value, 3, "left", false), None);
      assert_eq!(nav(&value, 2, "right", true), Some(3));
    }
  }

  #[test]
  fn repeated_and_mixed_numeric_separators_each_add_a_stop() {
    let value = "1,234.567";
    assert_eq!(nav(value, 0, "right", false), Some(2));
    assert_eq!(nav(value, 2, "right", false), Some(6));
    assert_eq!(nav(value, 0, "right", true), Some(1));
    assert_eq!(nav(value, 1, "right", true), Some(5));
    assert_eq!(nav(value, 9, "left", false), Some(6));
    assert_eq!(nav(value, 6, "left", false), Some(2));
  }

  #[test]
  fn spaces_retain_native_platform_behavior_and_newlines_remain_native() {
    assert_eq!(nav(" 123.456", 0, "right", false), None);
    assert_eq!(nav(" 123.456", 0, "right", true), Some(4));
    assert_eq!(nav("123.456  ", 9, "left", false), Some(4));
    assert_eq!(nav("123.456\n", 8, "left", false), None);
    assert_eq!(nav("\n123.456", 0, "right", true), None);
  }

  #[test]
  fn ordinary_text_and_punctuation_continue_to_use_native_navigation() {
    for value in ["", "123456", "abc.def", "can't", "你好世界", "123..456"] {
      let carets = value.char_indices().map(|(byte, _)| byte).chain([value.len()]);
      for caret in carets {
        for direction in ["left", "right"] {
          for is_mac in [false, true] {
            assert_eq!(
              nav(value, caret, direction, is_mac),
              None,
              "{value:?} {caret} {direction} {is_mac}"
            );
          }
        }
      }
    }
    // Do not skip another word or punctuation to reach a numeric separator.
    assert_eq!(nav("hello 123.456", 0, "right", false), None);
    assert_eq!(nav("abc.def123.456", 0, "right", false), None);
    assert_eq!(nav("abc.def123.456", 4, "right", false), Some(11));
    assert_eq!(nav("123.456.abc", 11, "left", false), None);
  }

  #[test]
  fn unicode_numbers_are_supported() {
    // Full-width digits and CJK take 3 bytes, the emoji and the mathematical
    // digits 4.
    assert_eq!(nav("字幕 １２３.４５６", 7, "right", false), Some(17));
    assert_eq!(nav("字幕 １２３.４５６", 26, "left", true), Some(17));
    assert_eq!(nav("🎬 123.456", 5, "right", false), Some(9));
    assert_eq!(nav("𝟙𝟚.𝟛𝟜", 17, "left", false), Some(9));
  }

  // Beyond the React tests: caret offsets at the edges, and the selection
  // update of `handleInputWordNavigation`.

  #[test]
  fn works_on_byte_offsets() {
    let value = "字幕 １２３.４５６";
    let dot = value.find('.').unwrap();
    assert_eq!(numeric_word_boundary(value, 7, Direction::Right, false), Some(dot + 1));
    assert_eq!(
      numeric_word_boundary(value, value.len(), Direction::Left, false),
      Some(dot + 1)
    );
    assert_eq!(
      numeric_word_boundary(value, value.len() + 1, Direction::Left, false),
      None
    );
    assert_eq!(numeric_word_boundary("123.456", 0, Direction::Left, false), None);
  }

  fn selection(range: Range<usize>, reversed: bool) -> Selection {
    Selection { range, reversed }
  }

  #[test]
  fn collapses_or_extends_the_selection() {
    let value = "1,234.567";
    // A plain jump collapses at the stop, from the side it moves away from.
    assert_eq!(
      navigate(value, &selection(0..2, false), Direction::Right, false, false),
      Some(selection(6..6, false))
    );
    assert_eq!(
      navigate(value, &selection(6..9, false), Direction::Left, false, false),
      Some(selection(2..2, false))
    );
    // Shift extends from the anchor; crossing it reverses the selection.
    assert_eq!(
      navigate(value, &selection(0..2, false), Direction::Right, true, false),
      Some(selection(0..6, false))
    );
    // No stop after the caret: the native jump runs.
    assert_eq!(
      navigate(value, &selection(6..9, true), Direction::Right, true, false),
      None
    );
    assert_eq!(
      navigate(value, &selection(2..6, true), Direction::Right, true, false),
      Some(selection(6..6, false))
    );
    assert_eq!(
      navigate(value, &selection(2..6, false), Direction::Left, true, false),
      Some(selection(2..2, false))
    );
    assert_eq!(
      navigate(value, &selection(6..9, false), Direction::Left, true, false),
      Some(selection(6..6, false))
    );
    assert_eq!(
      navigate(value, &selection(6..6, false), Direction::Left, true, false),
      Some(selection(2..6, true))
    );
    assert_eq!(
      navigate("abc", &selection(0..0, false), Direction::Right, false, false),
      None
    );
  }
}
