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

//! Thin BCP-47 wrapper.  We do not own a full IANA subtag registry — the
//! `language-tags` crate handles validation and we expose just the surface
//! the parser needs (validate + extract primary subtag).

use language_tags::LanguageTag;

/// Validate a BCP-47 tag.  Returns the canonical form on success.
pub fn validate(tag: &str) -> Option<String> {
  let parsed: LanguageTag = tag.parse().ok()?;
  Some(parsed.to_string())
}

/// Return the lowercased primary language subtag of a BCP-47 tag (e.g.
/// `"en"` for `"en-US"`).  Returns `None` if the tag is malformed.
pub fn primary_subtag(tag: &str) -> Option<String> {
  let parsed: LanguageTag = tag.parse().ok()?;
  Some(parsed.primary_language().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn validates_common_tags() {
    assert!(validate("en-US").is_some());
    assert!(validate("zh-Hant-HK").is_some());
    assert!(validate("ja").is_some());
    assert!(validate("de-DE").is_some());
  }

  #[test]
  fn rejects_garbage() {
    assert!(validate("not a tag at all").is_none());
    assert!(validate("").is_none());
    assert!(validate("--").is_none());
    // empty single-letter primary
    assert!(validate("z").is_none());
  }

  #[test]
  fn primary_subtag_strips_region() {
    assert_eq!(primary_subtag("en-US").as_deref(), Some("en"));
    assert_eq!(primary_subtag("zh-Hant-HK").as_deref(), Some("zh"));
    assert_eq!(primary_subtag("ja").as_deref(), Some("ja"));
  }

  #[test]
  fn primary_subtag_lowercases() {
    assert_eq!(primary_subtag("EN-us").as_deref(), Some("en"));
  }

  #[test]
  fn primary_subtag_on_garbage_is_none() {
    assert!(primary_subtag("bogus tag").is_none());
  }
}
