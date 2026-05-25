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

//! PES (Packetized Elementary Stream) header decoder.
//!
//! Layout (ISO/IEC 13818-1 §2.4.3.6):
//!
//! ```text
//! 24 bits: 0x000001 packet_start_code_prefix
//! u8       stream_id
//! u16      pes_packet_length (BE; 0 = "unbounded")
//! ```
//!
//! For PES packets that *aren't* private streams / padding, the next 3 bytes
//! carry flags + a header data length:
//!
//! ```text
//! '10' | scrambling(2) | priority | alignment_indicator | copyright | original_or_copy
//! flags(8): PTS_DTS_flags + ESCR + ES_rate + DSM + ADDITIONAL_COPY_INFO + CRC + EXTENSION
//! u8       pes_header_data_length
//! ```
//!
//! Identification-time we only need the stream_id (to know if the packet is
//! a private stream, system stream, ...).

use crate::media_metadata::error::ParseError;

#[derive(Debug, Clone, Copy)]
pub struct PesHeader {
  pub stream_id: u8,
  pub packet_length: u16,
}

pub const PES_START_CODE: [u8; 3] = [0x00, 0x00, 0x01];

pub fn parse(bytes: &[u8]) -> Result<PesHeader, ParseError> {
  if bytes.len() < 6 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("PES header {} bytes too small", bytes.len()),
    });
  }
  if bytes[..3] != PES_START_CODE {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: "missing PES start code 0x000001".to_string(),
    });
  }
  Ok(PesHeader {
    stream_id: bytes[3],
    packet_length: u16::from_be_bytes([bytes[4], bytes[5]]),
  })
}

/// Classify a stream_id byte into a coarse track type.
pub fn classify_stream_id(stream_id: u8) -> StreamIdClass {
  match stream_id {
    0xBC => StreamIdClass::ProgramStreamMap,
    0xBD => StreamIdClass::PrivateStream1,
    0xBE => StreamIdClass::Padding,
    0xBF => StreamIdClass::PrivateStream2,
    0xF0 | 0xF1 | 0xFF => StreamIdClass::Reserved,
    0xC0..=0xDF => StreamIdClass::Audio,
    0xE0..=0xEF => StreamIdClass::Video,
    _ => StreamIdClass::Other,
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamIdClass {
  Audio,
  Video,
  ProgramStreamMap,
  PrivateStream1,
  PrivateStream2,
  Padding,
  Reserved,
  Other,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn decodes_video_pes_header() {
    let bytes = [0x00, 0x00, 0x01, 0xE0, 0x12, 0x34];
    let h = parse(&bytes).unwrap();
    assert_eq!(h.stream_id, 0xE0);
    assert_eq!(h.packet_length, 0x1234);
    assert_eq!(classify_stream_id(h.stream_id), StreamIdClass::Video);
  }

  #[test]
  fn decodes_audio_pes_header() {
    let bytes = [0x00, 0x00, 0x01, 0xC1, 0xAB, 0xCD];
    let h = parse(&bytes).unwrap();
    assert_eq!(h.stream_id, 0xC1);
    assert_eq!(classify_stream_id(h.stream_id), StreamIdClass::Audio);
  }

  #[test]
  fn rejects_missing_start_code() {
    let bytes = [0x12, 0x34, 0x56, 0xE0, 0, 0];
    let err = parse(&bytes).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_truncated() {
    let err = parse(&[0x00, 0x00, 0x01]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn classify_handles_all_documented_stream_ids() {
    assert_eq!(classify_stream_id(0xBC), StreamIdClass::ProgramStreamMap);
    assert_eq!(classify_stream_id(0xBD), StreamIdClass::PrivateStream1);
    assert_eq!(classify_stream_id(0xBE), StreamIdClass::Padding);
    assert_eq!(classify_stream_id(0xBF), StreamIdClass::PrivateStream2);
    assert_eq!(classify_stream_id(0xC0), StreamIdClass::Audio);
    assert_eq!(classify_stream_id(0xDF), StreamIdClass::Audio);
    assert_eq!(classify_stream_id(0xE0), StreamIdClass::Video);
    assert_eq!(classify_stream_id(0xEF), StreamIdClass::Video);
    assert_eq!(classify_stream_id(0xF0), StreamIdClass::Reserved);
    assert_eq!(classify_stream_id(0xF1), StreamIdClass::Reserved);
    assert_eq!(classify_stream_id(0xFF), StreamIdClass::Reserved);
    assert_eq!(classify_stream_id(0x00), StreamIdClass::Other);
  }
}
