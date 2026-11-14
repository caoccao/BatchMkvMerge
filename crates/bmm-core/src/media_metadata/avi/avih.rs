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

//! `avih` (MainAVIHeader).  56 bytes of fixed-layout metadata:
//!
//! ```text
//! u32 microsec_per_frame
//! u32 max_bytes_per_sec
//! u32 padding_granularity
//! u32 flags
//! u32 total_frames
//! u32 initial_frames
//! u32 streams
//! u32 suggested_buffer_size
//! u32 width
//! u32 height
//! u32 reserved[4]
//! ```
//!
//! All fields are 32-bit little-endian.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use super::riff::ChunkHeader;

pub const AVIH_PAYLOAD_BYTES: u32 = 56;

#[derive(Debug, Clone, Copy)]
pub struct MainAviHeader {
  pub microsec_per_frame: u32,
  pub max_bytes_per_sec: u32,
  pub flags: u32,
  pub total_frames: u32,
  pub initial_frames: u32,
  pub streams: u32,
  pub width: u32,
  pub height: u32,
}

impl MainAviHeader {
  /// Bit set when the file uses an OpenDML index (> 2 GB-friendly).
  pub const FLAG_HAS_INDEX: u32 = 0x00000010;
  /// Bit set when the file is interleaved.
  pub const FLAG_IS_INTERLEAVED: u32 = 0x00000100;
  /// Bit set when the muxer recommended trusting the chunk index.
  pub const FLAG_TRUST_CK_TYPE: u32 = 0x00000800;
  /// Bit set when the file is capturing live data.
  pub const FLAG_WAS_CAPTURE_FILE: u32 = 0x00010000;
  /// Bit set when the file uses an OpenDML extended index.
  pub const FLAG_COPYRIGHTED: u32 = 0x00020000;

  /// Frame duration in nanoseconds, derived from `microsec_per_frame`.
  pub fn frame_duration_ns(&self) -> Option<u64> {
    if self.microsec_per_frame == 0 {
      None
    } else {
      Some(self.microsec_per_frame as u64 * 1000)
    }
  }

  /// Average bitrate in bits per second when known.
  pub fn average_bitrate_bps(&self) -> Option<u64> {
    if self.max_bytes_per_sec == 0 {
      None
    } else {
      Some(self.max_bytes_per_sec as u64 * 8)
    }
  }
}

pub fn parse(src: &mut FileSource, header: &ChunkHeader) -> Result<MainAviHeader, ParseError> {
  if header.size < AVIH_PAYLOAD_BYTES {
    return Err(ParseError::Malformed {
      format: "avi",
      offset: header.start,
      reason: format!(
        "avih payload {} bytes is smaller than the {} required",
        header.size, AVIH_PAYLOAD_BYTES
      ),
    });
  }
  let microsec_per_frame = src.read_u32_le()?;
  let max_bytes_per_sec = src.read_u32_le()?;
  let _padding_granularity = src.read_u32_le()?;
  let flags = src.read_u32_le()?;
  let total_frames = src.read_u32_le()?;
  let initial_frames = src.read_u32_le()?;
  let streams = src.read_u32_le()?;
  let _suggested_buffer_size = src.read_u32_le()?;
  let width = src.read_u32_le()?;
  let height = src.read_u32_le()?;
  // 4 reserved DWORDs = 16 bytes
  src.skip(16)?;
  Ok(MainAviHeader {
    microsec_per_frame,
    max_bytes_per_sec,
    flags,
    total_frames,
    initial_frames,
    streams,
    width,
    height,
  })
}

#[cfg(test)]
pub(crate) fn build_avih_payload(
  microsec_per_frame: u32,
  max_bytes_per_sec: u32,
  flags: u32,
  total_frames: u32,
  streams: u32,
  width: u32,
  height: u32,
) -> Vec<u8> {
  let mut p = Vec::with_capacity(AVIH_PAYLOAD_BYTES as usize);
  p.extend_from_slice(&microsec_per_frame.to_le_bytes());
  p.extend_from_slice(&max_bytes_per_sec.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes()); // padding_granularity
  p.extend_from_slice(&flags.to_le_bytes());
  p.extend_from_slice(&total_frames.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes()); // initial_frames
  p.extend_from_slice(&streams.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes()); // suggested_buffer_size
  p.extend_from_slice(&width.to_le_bytes());
  p.extend_from_slice(&height.to_le_bytes());
  p.extend_from_slice(&[0u8; 16]); // reserved[4]
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::avi::riff::{self, encode_chunk};
  use std::io::Cursor;

  fn read(payload: Vec<u8>) -> (ChunkHeader, FileSource) {
    let bytes = encode_chunk(b"avih", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = riff::read_chunk_header(&mut s).unwrap();
    (h, s)
  }

  #[test]
  fn parses_full_avih_payload() {
    let payload = build_avih_payload(41_708, 5_000_000, 0, 240, 2, 1920, 1080);
    let (h, mut s) = read(payload);
    let avih = parse(&mut s, &h).unwrap();
    assert_eq!(avih.microsec_per_frame, 41_708);
    assert_eq!(avih.max_bytes_per_sec, 5_000_000);
    assert_eq!(avih.total_frames, 240);
    assert_eq!(avih.streams, 2);
    assert_eq!(avih.width, 1920);
    assert_eq!(avih.height, 1080);
  }

  #[test]
  fn rejects_truncated_payload() {
    let mut bytes = b"avih".to_vec();
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = riff::read_chunk_header(&mut s).unwrap();
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn frame_duration_ns_handles_zero_microsec() {
    let avih = MainAviHeader {
      microsec_per_frame: 0,
      max_bytes_per_sec: 0,
      flags: 0,
      total_frames: 0,
      initial_frames: 0,
      streams: 0,
      width: 0,
      height: 0,
    };
    assert!(avih.frame_duration_ns().is_none());
    assert!(avih.average_bitrate_bps().is_none());
  }

  #[test]
  fn frame_duration_ns_converts_microseconds_to_nanoseconds() {
    let avih = MainAviHeader {
      microsec_per_frame: 41_708,
      max_bytes_per_sec: 1_000_000,
      flags: 0,
      total_frames: 0,
      initial_frames: 0,
      streams: 0,
      width: 0,
      height: 0,
    };
    assert_eq!(avih.frame_duration_ns(), Some(41_708_000));
    assert_eq!(avih.average_bitrate_bps(), Some(8_000_000));
  }

  #[test]
  fn flag_constants_match_spec_bits() {
    assert_eq!(MainAviHeader::FLAG_HAS_INDEX, 0x10);
    assert_eq!(MainAviHeader::FLAG_IS_INTERLEAVED, 0x100);
    assert_eq!(MainAviHeader::FLAG_TRUST_CK_TYPE, 0x800);
    assert_eq!(MainAviHeader::FLAG_WAS_CAPTURE_FILE, 0x1_0000);
    assert_eq!(MainAviHeader::FLAG_COPYRIGHTED, 0x2_0000);
  }
}
