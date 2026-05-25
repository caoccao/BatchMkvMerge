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

//! Codec-specific configuration boxes that hang off sample entries.
//!
//! Each sub-module owns one FOURCC and writes into the parent `TrackBuilder`:
//! - [`avcc`] — `avc1` configuration record (profile / level / chroma).
//! - [`hvcc`] — `hev1`/`hvc1` configuration record (profile / tier / level).
//! - [`esds`] — MPEG-4 elementary stream descriptor (AAC AudioSpecificConfig).
//! - [`colr`] — colour information atom (nclx / nclc).
//! - [`pasp`] — pixel aspect ratio.
//! - [`dvcc`] — Dolby Vision configuration (dvcC / dvvC).

pub mod av1c;
pub mod avcc;
pub mod colr;
pub mod dvcc;
pub mod esds;
pub mod hvcc;
pub mod pasp;

/// Hex-encode a byte slice using lower-case digits.
pub fn hex_encode(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{:02x}", b));
  }
  s
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hex_encode_is_lower_case_and_padded() {
    assert_eq!(hex_encode(&[0xAB, 0x01, 0xFF]), "ab01ff");
    assert_eq!(hex_encode(&[]), "");
  }
}
