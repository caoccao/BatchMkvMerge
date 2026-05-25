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

//! MPEG program-stream packet markers and helpers.
//!
//! Every PS packet starts with a 4-byte start code: `0x00 0x00 0x01 <byte>`.
//! Common start-code values:
//!
//! - `0xBA` — pack header (groups PES packets into a "pack")
//! - `0xBB` — system header
//! - `0xBC` — program stream map
//! - `0xBD` — private stream 1 (carries AC-3, DTS, LPCM in DVD-VOB)
//! - `0xBE` — padding
//! - `0xBF` — private stream 2
//! - `0xC0..=0xDF` — audio PES
//! - `0xE0..=0xEF` — video PES
//! - `0xB9` — MPEG-PS end code (program_end_code)

pub const START_CODE_PREFIX: [u8; 3] = [0x00, 0x00, 0x01];
pub const PACK_HEADER: u8 = 0xBA;
pub const SYSTEM_HEADER: u8 = 0xBB;
pub const PROGRAM_STREAM_MAP: u8 = 0xBC;
pub const PRIVATE_STREAM_1: u8 = 0xBD;
pub const PADDING: u8 = 0xBE;
pub const PRIVATE_STREAM_2: u8 = 0xBF;
pub const PROGRAM_END_CODE: u8 = 0xB9;
/// VC-1 video PES stream id (mkvtoolnix `r_mpeg_ps.cpp:925`).  PARSER-094.
pub const VC1_VIDEO: u8 = 0xFD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartCode {
  PackHeader,
  SystemHeader,
  ProgramStreamMap,
  PrivateStream1,
  PrivateStream2,
  Padding,
  Audio(u8),
  Video(u8),
  ProgramEnd,
  Other(u8),
}

impl StartCode {
  pub fn from_byte(b: u8) -> Self {
    match b {
      PACK_HEADER => Self::PackHeader,
      SYSTEM_HEADER => Self::SystemHeader,
      PROGRAM_STREAM_MAP => Self::ProgramStreamMap,
      PRIVATE_STREAM_1 => Self::PrivateStream1,
      PADDING => Self::Padding,
      PRIVATE_STREAM_2 => Self::PrivateStream2,
      PROGRAM_END_CODE => Self::ProgramEnd,
      0xC0..=0xDF => Self::Audio(b),
      0xE0..=0xEF => Self::Video(b),
      VC1_VIDEO => Self::Video(b),
      other => Self::Other(other),
    }
  }
}

/// Find the next MPEG-PS start code in `bytes`, starting at `from`.  Returns
/// `(absolute_offset, stream_id)` on success.
pub fn find_start_code(bytes: &[u8], from: usize) -> Option<(usize, u8)> {
  let mut i = from;
  while i + 4 <= bytes.len() {
    if bytes[i] == 0x00 && bytes[i + 1] == 0x00 && bytes[i + 2] == 0x01 {
      return Some((i, bytes[i + 3]));
    }
    i += 1;
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classify_pack_header() {
    assert_eq!(StartCode::from_byte(PACK_HEADER), StartCode::PackHeader);
  }
  #[test]
  fn classify_system_header() {
    assert_eq!(StartCode::from_byte(SYSTEM_HEADER), StartCode::SystemHeader);
  }
  #[test]
  fn classify_program_stream_map() {
    assert_eq!(StartCode::from_byte(PROGRAM_STREAM_MAP), StartCode::ProgramStreamMap);
  }
  #[test]
  fn classify_private_streams() {
    assert_eq!(StartCode::from_byte(PRIVATE_STREAM_1), StartCode::PrivateStream1);
    assert_eq!(StartCode::from_byte(PRIVATE_STREAM_2), StartCode::PrivateStream2);
  }
  #[test]
  fn classify_padding() {
    assert_eq!(StartCode::from_byte(PADDING), StartCode::Padding);
  }
  #[test]
  fn classify_audio_range() {
    assert_eq!(StartCode::from_byte(0xC0), StartCode::Audio(0xC0));
    assert_eq!(StartCode::from_byte(0xDF), StartCode::Audio(0xDF));
  }
  #[test]
  fn classify_video_range() {
    assert_eq!(StartCode::from_byte(0xE0), StartCode::Video(0xE0));
    assert_eq!(StartCode::from_byte(0xEF), StartCode::Video(0xEF));
  }
  #[test]
  fn classify_program_end() {
    assert_eq!(StartCode::from_byte(PROGRAM_END_CODE), StartCode::ProgramEnd);
  }
  #[test]
  fn classify_other_falls_through() {
    assert_eq!(StartCode::from_byte(0x42), StartCode::Other(0x42));
  }

  #[test]
  fn find_start_code_locates_first_occurrence() {
    let bytes = [0xFFu8, 0xFF, 0x00, 0x00, 0x01, 0xBA, 0xAA];
    assert_eq!(find_start_code(&bytes, 0), Some((2, 0xBA)));
  }
  #[test]
  fn find_start_code_returns_none_when_absent() {
    let bytes = [0xFFu8; 16];
    assert!(find_start_code(&bytes, 0).is_none());
  }
  #[test]
  fn find_start_code_respects_from_offset() {
    let bytes = [0x00u8, 0x00, 0x01, 0xBA, 0x00, 0x00, 0x01, 0xE0];
    let (pos, id) = find_start_code(&bytes, 4).unwrap();
    assert_eq!(pos, 4);
    assert_eq!(id, 0xE0);
  }
}
