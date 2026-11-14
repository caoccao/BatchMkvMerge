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

//! Kate identification header.  Layout per the Kate codec spec:
//!
//! ```text
//! u8 0x80
//! 7  "kate\0\0\0"
//! u8 reserved (== 0)
//! u8 VMAJ
//! u8 VMIN
//! u8 ntracks
//! u8 granule_shift
//! u8 numerator (frame rate)
//! u8 denominator
//! u32 granule_rate_n  (LE)
//! u32 granule_rate_d  (LE)
//! u32 base_granule    (LE)
//! u32 message_header_flags (LE)
//! 16 language          (NUL-terminated BCP-47, padded)
//! 16 category          (e.g. "subtitles", "lyrics", ...)
//! ```

use super::BitstreamMetadata;

const SIGNATURE: [u8; 8] = [0x80, b'k', b'a', b't', b'e', 0x00, 0x00, 0x00];

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  if packet.len() < 32 || packet[..8] != SIGNATURE {
    return None;
  }
  let mut metadata = BitstreamMetadata::subtitle("S_KATE", "Kate");
  if packet.len() >= 60 {
    let lang_bytes = &packet[32..48];
    if let Some(end) = lang_bytes.iter().position(|b| *b == 0) {
      let trimmed = &lang_bytes[..end];
      if !trimmed.is_empty() {
        metadata.language = Some(String::from_utf8_lossy(trimmed).into_owned());
      }
    }
  }
  Some(metadata)
}

#[cfg(test)]
pub(crate) fn build_identification_packet(language: &str) -> Vec<u8> {
  let mut p = Vec::with_capacity(64);
  p.extend_from_slice(&SIGNATURE);
  p.extend_from_slice(&[0u8; 24]); // reserved + version + counts + rates
  let mut lang_bytes = [0u8; 16];
  let n = language.len().min(15);
  lang_bytes[..n].copy_from_slice(&language.as_bytes()[..n]);
  p.extend_from_slice(&lang_bytes);
  p.extend_from_slice(&[0u8; 16]); // category
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sniffs_kate_with_language() {
    let pkt = build_identification_packet("fr-FR");
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "S_KATE");
    assert_eq!(m.language.as_deref(), Some("fr-FR"));
  }

  #[test]
  fn sniffs_kate_without_language() {
    let pkt = build_identification_packet("");
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "S_KATE");
    assert!(m.language.is_none());
  }

  #[test]
  fn rejects_non_kate_signature() {
    assert!(sniff(b"\x80theora").is_none());
  }

  #[test]
  fn rejects_short_packet() {
    assert!(sniff(&SIGNATURE).is_none());
  }
}
