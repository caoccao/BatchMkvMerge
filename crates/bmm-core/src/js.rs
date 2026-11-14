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

//! JavaScript semantics the React app relied on, reproduced so the ported
//! logic gives the same results: `Number(string)`, the `\s` class,
//! `Number.prototype.toFixed` rounding and the UTF-16 code-unit order of
//! `<` / `Array.prototype.sort` on strings.

use std::cmp::Ordering;

/// JavaScript's `\s`: Unicode `White_Space` without U+0085, plus U+FEFF.
pub fn is_whitespace(c: char) -> bool {
  (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// JavaScript's `Number(string)`: surrounding whitespace is ignored, an empty
/// string is 0, unsigned `0x` / `0o` / `0b` literals are integers, and
/// anything else must be a decimal literal or `Infinity`. `None` stands for
/// `NaN`.
pub fn number(text: &str) -> Option<f64> {
  let text = text.trim_matches(is_whitespace);
  if text.is_empty() {
    return Some(0.0);
  }
  for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
    if let Some(digits) = text.strip_prefix(prefix) {
      if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
      }
      return Some(digits.chars().fold(0.0, |value, c| {
        value * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
      }));
    }
  }
  let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
  if unsigned == "Infinity" {
    return Some(if text.starts_with('-') {
      f64::NEG_INFINITY
    } else {
      f64::INFINITY
    });
  }
  // Rust also parses `inf`, `nan` and `infinity` in any case; JavaScript
  // does not, so only digits, dots and exponents go on to the parser.
  let decimal_literal = unsigned
    .chars()
    .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    && unsigned.chars().any(|c| c.is_ascii_digit());
  if !decimal_literal {
    return None;
  }
  text.parse::<f64>().ok()
}

/// JavaScript's `value.toFixed(digits)` for finite values below 1e21.
///
/// Both languages round the exact binary value, so they agree except on exact
/// ties, where Rust rounds half to even and JavaScript picks the larger
/// candidate (`(1.125).toFixed(2)` is `1.13`, `format!("{:.2}", 1.125)` is
/// `1.12`). A tie at `digits` means `value * 2^(digits + 1)` is an odd
/// integer, so that case is computed exactly in integers.
pub fn to_fixed(value: f64, digits: usize) -> String {
  let scale = 2f64.powi(digits as i32 + 1);
  let scaled = value.abs() * scale;
  if scaled.fract() == 0.0 && scaled < 2f64.powi(64) && (scaled as u64) % 2 == 1 {
    // value = odd / 2^(digits+1) is exactly half-way between two candidates
    // n - 1 and n at `digits` places: n = (odd * 5^digits + 1) / 2.
    let odd = u128::from(scaled as u64);
    let n = (odd * 5u128.pow(digits as u32)).div_ceil(2);
    let mut text = n.to_string();
    if digits > 0 {
      if text.len() <= digits {
        text = format!("{}{text}", "0".repeat(digits + 1 - text.len()));
      }
      text.insert(text.len() - digits, '.');
    }
    // JavaScript rounds a negative tie away from zero as well, and keeps the
    // sign of a value that rounds to zero.
    return if value.is_sign_negative() {
      format!("-{text}")
    } else {
      text
    };
  }
  format!("{value:.digits$}")
}

/// The UTF-16 length JavaScript's `String.prototype.length` reports.
pub fn utf16_len(text: &str) -> usize {
  text.encode_utf16().count()
}

/// JavaScript's string order (`<`, default `sort`): by UTF-16 code units. It
/// differs from Rust's `str` order only for characters above U+FFFF against
/// characters in U+E000..=U+FFFF.
pub fn cmp_utf16(a: &str, b: &str) -> Ordering {
  a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_numbers_like_javascript() {
    assert_eq!(number(" 42 "), Some(42.0));
    assert_eq!(number("\u{feff}7\u{3000}"), Some(7.0));
    assert_eq!(number(""), Some(0.0));
    assert_eq!(number("1e3"), Some(1000.0));
    assert_eq!(number("-2.5"), Some(-2.5));
    assert_eq!(number(".5"), Some(0.5));
    assert_eq!(number("5."), Some(5.0));
    assert_eq!(number("0x1F"), Some(31.0));
    assert_eq!(number("0b101"), Some(5.0));
    assert_eq!(number("0O17"), Some(15.0));
    assert_eq!(number("Infinity"), Some(f64::INFINITY));
    assert_eq!(number("-Infinity"), Some(f64::NEG_INFINITY));
    for not_a_number in [
      "0x", "0xG", "-0x10", "inf", "NaN", "12 kbps", "1_000", "e", "+-1", "1.2.3", "\u{85}1",
    ] {
      assert_eq!(number(not_a_number), None, "{not_a_number:?}");
    }
  }

  #[test]
  fn matches_the_javascript_whitespace_class() {
    for c in [
      ' ', '\t', '\n', '\r', '\u{b}', '\u{c}', '\u{a0}', '\u{2028}', '\u{3000}', '\u{feff}',
    ] {
      assert!(is_whitespace(c), "{c:?}");
    }
    for c in ['\u{85}', 'a', '0', '\u{200b}'] {
      assert!(!is_whitespace(c), "{c:?}");
    }
  }

  #[test]
  fn rounds_ties_like_to_fixed() {
    assert_eq!(to_fixed(1.125, 2), "1.13");
    assert_eq!(to_fixed(1.375, 2), "1.38");
    assert_eq!(to_fixed(0.125, 2), "0.13");
    assert_eq!(to_fixed(0.25, 1), "0.3");
    assert_eq!(to_fixed(2.5, 0), "3");
    assert_eq!(to_fixed(0.5, 0), "1");
    assert_eq!(to_fixed(-2.5, 0), "-3");
    assert_eq!(to_fixed(0.005, 2), "0.01");
    // Not ties: the binary value is just below or above the half.
    assert_eq!(to_fixed(1.005, 2), "1.00");
    assert_eq!(to_fixed(0.35, 1), "0.3");
    assert_eq!(to_fixed(1.45, 1), "1.4");
    assert_eq!(to_fixed(10.0, 2), "10.00");
    assert_eq!(to_fixed(1023.999, 2), "1024.00");
    assert_eq!(to_fixed(3.0, 0), "3");
    assert_eq!(to_fixed(0.0, 2), "0.00");
  }

  #[test]
  fn compares_by_utf16_code_units() {
    assert_eq!(cmp_utf16("a", "b"), Ordering::Less);
    assert_eq!(cmp_utf16("ab", "a"), Ordering::Greater);
    assert_eq!(cmp_utf16("same", "same"), Ordering::Equal);
    // U+1F3AC is a surrogate pair (0xD83C …), which sorts before U+FF21.
    assert_eq!(cmp_utf16("\u{1f3ac}", "\u{ff21}"), Ordering::Less);
    assert_eq!("\u{1f3ac}".cmp("\u{ff21}"), Ordering::Greater);
    assert_eq!(utf16_len("a\u{1f3ac}é"), 4);
  }
}
