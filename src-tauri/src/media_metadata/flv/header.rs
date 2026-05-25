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

//! FLV file header (9 bytes) — Adobe SWF/Flash Video specification.
//!
//! ```text
//! offset  size   field
//! 0       3      "FLV" magic
//! 3       1      version (typically 1)
//! 4       1      type_flags (bit 0 = video, bit 2 = audio)
//! 5       4      data_offset (big-endian) — always 9 for v1
//! ```

pub const MAGIC: [u8; 3] = *b"FLV";
pub const HEADER_LEN: usize = 9;
pub const TYPE_FLAG_VIDEO: u8 = 0x01;
pub const TYPE_FLAG_AUDIO: u8 = 0x04;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvHeader {
  pub version: u8,
  pub type_flags: u8,
  pub data_offset: u32,
}

impl FlvHeader {
  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < HEADER_LEN || bytes[..3] != MAGIC {
      return None;
    }
    Some(Self {
      version: bytes[3],
      type_flags: bytes[4],
      data_offset: u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
    })
  }

  pub fn has_video(&self) -> bool {
    self.type_flags & TYPE_FLAG_VIDEO == TYPE_FLAG_VIDEO
  }

  pub fn has_audio(&self) -> bool {
    self.type_flags & TYPE_FLAG_AUDIO == TYPE_FLAG_AUDIO
  }
}

#[cfg(test)]
pub(crate) fn build_header(version: u8, type_flags: u8) -> Vec<u8> {
  let mut buf = Vec::with_capacity(HEADER_LEN);
  buf.extend_from_slice(&MAGIC);
  buf.push(version);
  buf.push(type_flags);
  buf.extend_from_slice(&(HEADER_LEN as u32).to_be_bytes());
  buf
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parse_accepts_canonical_v1_header() {
    let h = FlvHeader::parse(&build_header(1, TYPE_FLAG_VIDEO | TYPE_FLAG_AUDIO)).unwrap();
    assert_eq!(h.version, 1);
    assert_eq!(h.data_offset, HEADER_LEN as u32);
    assert!(h.has_audio());
    assert!(h.has_video());
  }

  #[test]
  fn parse_rejects_wrong_magic() {
    let mut bytes = build_header(1, 0x05);
    bytes[0] = b'X';
    assert!(FlvHeader::parse(&bytes).is_none());
  }

  #[test]
  fn parse_rejects_short_input() {
    assert!(FlvHeader::parse(&[0u8; 5]).is_none());
  }

  #[test]
  fn type_flag_audio_only_disables_video() {
    let h = FlvHeader::parse(&build_header(1, TYPE_FLAG_AUDIO)).unwrap();
    assert!(h.has_audio());
    assert!(!h.has_video());
  }

  #[test]
  fn type_flag_video_only_disables_audio() {
    let h = FlvHeader::parse(&build_header(1, TYPE_FLAG_VIDEO)).unwrap();
    assert!(!h.has_audio());
    assert!(h.has_video());
  }
}
