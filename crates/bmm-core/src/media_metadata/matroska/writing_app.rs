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

//! WritingApp string parser — port of
//! `r_matroska.cpp::read_headers_info_writing_app` (lines 1206-1255).
//!
//! Examples from the C++ comments:
//!
//! ```text
//! mkvmerge v0.6.6
//! mkvmerge v0.9.6 ('Every Little Kiss') built on Oct  7 2004 18:37:49
//! VirtualDubMod 1.5.4.1 (build 2178/release)
//! AVI-Mux GUI 1.16.8 MPEG test build 1, Aug 24 2004  12:42:57
//! HandBrake 0.10.2 2015060900
//! ```
//!
//! The C++ implementation:
//! 1. Strips whitespace.
//! 2. Special-cases "avi-mux gui" → "avimuxgui" so the first part has no space.
//! 3. Splits on spaces into 3 parts.
//! 4. Lowercases the first part as the application name.
//! 5. Parses up to four `.`-separated numeric tokens from the second part as
//!    the version, packed into one `i64` (8 bits per component).
//! 6. If parsing fails or fewer than 2 parts are present, the whole string
//!    becomes the lower-case app name and version becomes `-1`.
//!
//! We return both pieces plus a re-rendered display string (`app vX.Y.Z`)
//! used as the `writingApp` JSON value.  Reconstructing the display string
//! matches what `mkvmerge -J` shows.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWritingApp {
  /// Lower-case application name (e.g. `mkvmerge`).
  pub app: String,
  /// Packed version: bytes 0..3 hold components 1..4, big-endian.  Value
  /// `-1` if no version could be parsed.
  pub version_packed: i64,
}

impl ParsedWritingApp {
  /// Render a display form that mirrors `mkvmerge -J`'s output: lower-case
  /// app name + space + each component joined by `.`. Trailing `.0`
  /// components are stripped so `1.20.0.0` becomes `1.20`.  When parsing
  /// failed, returns the originally-supplied string.
  pub fn into_display(self, original: &str) -> String {
    if self.version_packed < 0 {
      return original.to_string();
    }
    let v = self.version_packed as u64;
    let arr = [
      ((v >> 24) & 0xFF) as u8,
      ((v >> 16) & 0xFF) as u8,
      ((v >> 8) & 0xFF) as u8,
      (v & 0xFF) as u8,
    ];
    // Strip trailing zeros but keep at least two components.
    let mut len = arr.len();
    while len > 2 && arr[len - 1] == 0 {
      len -= 1;
    }
    let version_str: String = arr[..len].iter().map(|c| c.to_string()).collect::<Vec<_>>().join(".");
    format!("{} {}", self.app, version_str)
  }
}

/// Parse the raw `WritingApp` string per mkvtoolnix's algorithm.
pub fn parse(input: &str) -> ParsedWritingApp {
  let mut s = input.trim().to_string();
  if s.is_empty() {
    return ParsedWritingApp {
      app: String::new(),
      version_packed: -1,
    };
  }
  // Mirror the special-case: collapse "avi-mux gui" into "avimuxgui" so
  // the space inside isn't a delimiter.
  let lower = s.to_ascii_lowercase();
  if lower.starts_with("avi-mux gui") {
    let rest = s["avi-mux gui".len()..].to_string();
    s = format!("avimuxgui{rest}");
  }

  let parts: Vec<&str> = s.splitn(3, ' ').filter(|p| !p.is_empty()).collect();
  if parts.len() < 2 {
    // Whole string lower-cased as app name, no version.
    return ParsedWritingApp {
      app: s.to_ascii_lowercase(),
      version_packed: -1,
    };
  }
  let app_name = parts[0].to_ascii_lowercase();
  let ver_token = parts[1];

  // Strip non-digit-and-dot characters (e.g. leading 'v') and split.
  let cleaned: String = ver_token.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
  if cleaned.is_empty() {
    return ParsedWritingApp {
      app: app_name,
      version_packed: -1,
    };
  }

  let mut comps: Vec<i64> = Vec::with_capacity(4);
  for tok in cleaned.split('.') {
    if tok.is_empty() {
      comps.push(0);
      continue;
    }
    match tok.parse::<i64>() {
      Ok(n) if (0..=255).contains(&n) => comps.push(n),
      _ => {
        return ParsedWritingApp {
          app: app_name,
          version_packed: -1,
        };
      }
    }
  }
  while comps.len() < 4 {
    comps.push(0);
  }
  let packed = (comps[0] << 24) | (comps[1] << 16) | (comps[2] << 8) | comps[3];
  ParsedWritingApp {
    app: app_name,
    version_packed: packed,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn mkvmerge_v_prefix() {
    let p = parse("mkvmerge v89.0");
    assert_eq!(p.app, "mkvmerge");
    // 89.0.0.0 packed
    assert_eq!(p.version_packed, (89i64 << 24));
    assert_eq!(p.clone().into_display("mkvmerge v89.0"), "mkvmerge 89.0");
  }

  #[test]
  fn mkvmerge_dot_components() {
    let p = parse("mkvmerge v9.6.0");
    assert_eq!(p.app, "mkvmerge");
    assert_eq!(p.version_packed, (9i64 << 24) | (6 << 16));
    assert_eq!(p.clone().into_display("mkvmerge v9.6.0"), "mkvmerge 9.6");
  }

  #[test]
  fn whitespace_trim() {
    let p = parse("  HandBrake 0.10.2  ");
    assert_eq!(p.app, "handbrake");
    assert_eq!(p.version_packed, (0i64 << 24) | (10 << 16) | (2 << 8));
  }

  #[test]
  fn avi_mux_gui_normalised() {
    let p = parse("AVI-Mux GUI 1.16.8 MPEG test build 1, Aug 24 2004  12:42:57");
    assert_eq!(p.app, "avimuxgui");
    assert_eq!(p.version_packed, (1i64 << 24) | (16 << 16) | (8 << 8));
  }

  #[test]
  fn single_part_input_keeps_whole_string_lowercased() {
    let p = parse("opaque");
    assert_eq!(p.app, "opaque");
    assert_eq!(p.version_packed, -1);
    // Display falls back to original
    assert_eq!(p.into_display("opaque"), "opaque");
  }

  #[test]
  fn empty_input_returns_empty_app_and_no_version() {
    let p = parse("");
    assert_eq!(p.app, "");
    assert_eq!(p.version_packed, -1);
  }

  #[test]
  fn version_token_with_no_digits_falls_back_to_no_version() {
    let p = parse("HandBrake VERY-NEW");
    assert_eq!(p.app, "handbrake");
    assert_eq!(p.version_packed, -1);
  }

  #[test]
  fn version_component_too_large_falls_back() {
    let p = parse("foo 1.300");
    // 300 > 255 ⇒ fallback
    assert_eq!(p.app, "foo");
    assert_eq!(p.version_packed, -1);
  }

  #[test]
  fn fewer_than_4_components_pad_with_zero() {
    let p = parse("foo 1");
    assert_eq!(p.app, "foo");
    assert_eq!(p.version_packed, 1 << 24);
  }

  #[test]
  fn display_strips_trailing_zero_components_but_keeps_two() {
    let p = ParsedWritingApp {
      app: "tool".to_owned(),
      version_packed: (1i64 << 24) | (20 << 16),
    };
    assert_eq!(p.into_display("anything"), "tool 1.20");

    let p = ParsedWritingApp {
      app: "tool".to_owned(),
      version_packed: 1 << 24,
    };
    assert_eq!(p.into_display("anything"), "tool 1.0");
  }
}
