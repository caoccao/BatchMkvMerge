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

//! ISO 639 language descriptor (tag 0x0A).
//!
//! Body: one or more 4-byte entries `(3 ASCII bytes ISO 639-2 code + 1 byte
//! audio_type)`.  We keep only the first language (mkvmerge identification
//! also drops the rest).

use crate::media_metadata::language::iso_639;

pub fn decode(body: &[u8]) -> Option<String> {
  if body.len() < 3 {
    return None;
  }
  let lang_bytes = &body[..3];
  if !lang_bytes
    .iter()
    .all(|b| (b'a'..=b'z').contains(b) || (b'A'..=b'Z').contains(b))
  {
    return None;
  }
  let code: String = lang_bytes.iter().map(|b| b.to_ascii_lowercase() as char).collect();
  iso_639::is_valid(&code).then_some(code)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn decodes_eng() {
    assert_eq!(decode(b"eng\x00").as_deref(), Some("eng"));
  }

  #[test]
  fn upper_case_normalised_to_lower() {
    assert_eq!(decode(b"ENG\x00").as_deref(), Some("eng"));
  }

  #[test]
  fn rejects_non_letter_payload() {
    assert!(decode(b"12\xAA\x00").is_none());
  }

  #[test]
  fn rejects_unknown_iso_code() {
    assert!(decode(b"zzz\x00").is_none());
  }

  #[test]
  fn rejects_too_short() {
    assert!(decode(b"en").is_none());
  }
}
