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

//! DVB subtitling descriptor (tag 0x59) — ETSI EN 300 468 §6.2.41.
//!
//! Each 8-byte entry: ISO-639 (3) + subtitling_type (1) + composition_page_id
//! (2 BE) + ancillary_page_id (2 BE).  mkvtoolnix derives S_DVBSUB tracks
//! and stores a 5-byte codec_private of `[comp_page (2BE), anc_page (2BE),
//! subtitling_type]`.  See `parse_subtitling_pmt_descriptor` at
//! `r_mpeg_ts.cpp:942-968`.  PARSER-091.

/// Decoded DVB subtitling descriptor entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitlingEntry {
  pub language_iso_639_2: String,
  pub subtitling_type: u8,
  pub composition_page_id: u16,
  pub ancillary_page_id: u16,
}

impl SubtitlingEntry {
  /// 5-byte codec_private as written by mkvtoolnix for DVB subtitle
  /// packetizers: composition_page (BE u16) ‖ ancillary_page (BE u16) ‖
  /// subtitling_type.
  pub fn codec_private(&self) -> [u8; 5] {
    let cp = self.composition_page_id.to_be_bytes();
    let an = self.ancillary_page_id.to_be_bytes();
    [cp[0], cp[1], an[0], an[1], self.subtitling_type]
  }
}

pub fn decode_all(body: &[u8]) -> Vec<SubtitlingEntry> {
  let mut out = Vec::new();
  let mut pos = 0usize;
  while pos + 8 <= body.len() {
    let lang_bytes = &body[pos..pos + 3];
    let subtitling_type = body[pos + 3];
    let composition_page_id = u16::from_be_bytes([body[pos + 4], body[pos + 5]]);
    let ancillary_page_id = u16::from_be_bytes([body[pos + 6], body[pos + 7]]);
    let language_iso_639_2 = String::from_utf8_lossy(lang_bytes)
      .trim_end_matches('\0')
      .trim()
      .to_string();
    out.push(SubtitlingEntry {
      language_iso_639_2,
      subtitling_type,
      composition_page_id,
      ancillary_page_id,
    });
    pos += 8;
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn decodes_single_entry() {
    let body = [b'e', b'n', b'g', 0x10, 0x00, 0x01, 0x00, 0x02];
    let v = decode_all(&body);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].language_iso_639_2, "eng");
    assert_eq!(v[0].subtitling_type, 0x10);
    assert_eq!(v[0].composition_page_id, 0x0001);
    assert_eq!(v[0].ancillary_page_id, 0x0002);
    assert_eq!(v[0].codec_private(), [0x00, 0x01, 0x00, 0x02, 0x10]);
  }

  #[test]
  fn decodes_multiple_entries() {
    let mut body = Vec::new();
    body.extend_from_slice(&[b'e', b'n', b'g', 0x10, 0x00, 0x01, 0x00, 0x02]);
    body.extend_from_slice(&[b'd', b'e', b'u', 0x20, 0x00, 0x03, 0x00, 0x04]);
    let v = decode_all(&body);
    assert_eq!(v.len(), 2);
    assert_eq!(v[1].language_iso_639_2, "deu");
    assert_eq!(v[1].composition_page_id, 0x0003);
  }

  #[test]
  fn truncated_body_yields_no_entry() {
    assert!(decode_all(&[1, 2, 3, 4]).is_empty());
  }
}
