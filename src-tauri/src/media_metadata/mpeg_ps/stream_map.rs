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

//! Program Stream Map (0x000001BC) decoder — ISO/IEC 13818-1 §2.5.4.
//!
//! Layout after the 4-byte start code:
//!
//! ```text
//! u16 program_stream_map_length
//! u8  current_next | reserved(2) | program_stream_map_version(5)
//! u8  reserved(7) | marker
//! u16 program_stream_info_length
//! [program_stream_info_length bytes of descriptors]
//! u16 elementary_stream_map_length
//! repeat elementary_stream_map_length / 4 times:
//!   u8  stream_type
//!   u8  elementary_stream_id
//!   u16 elementary_stream_info_length
//!   [elementary_stream_info_length bytes of descriptors]
//! u32 CRC32
//! ```

use crate::media_metadata::error::ParseError;

#[derive(Debug, Clone)]
pub struct PsmEntry {
  pub stream_type: u8,
  pub elementary_stream_id: u8,
  pub descriptors: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct ProgramStreamMap {
  pub entries: Vec<PsmEntry>,
}

pub fn parse(payload: &[u8]) -> Result<ProgramStreamMap, ParseError> {
  if payload.len() < 2 {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: format!("PSM payload {} bytes too small", payload.len()),
    });
  }
  let psm_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
  if psm_len == 0 || psm_len > 1018 {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: format!("invalid PSM length {psm_len}"),
    });
  }
  let declared_end = 2 + psm_len;
  if payload.len() < declared_end || psm_len < 8 {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: "PSM declared length overruns payload".to_string(),
    });
  }
  let payload = &payload[..declared_end];
  // bytes[0..2] = program_stream_map_length, [2..4] = ver/marker bytes
  let psi_len = u16::from_be_bytes([payload[4], payload[5]]) as usize;
  let mut pos = 6 + psi_len;
  if pos + 2 > declared_end.saturating_sub(4) {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: "PSM program_stream_info_length overruns payload".to_string(),
    });
  }
  let esi_len = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
  pos += 2;
  let map_end = pos + esi_len;
  if map_end + 4 > declared_end {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: "PSM elementary_stream_map_length overruns payload".to_string(),
    });
  }
  let mut entries = Vec::new();
  while pos + 4 <= map_end {
    let stream_type = payload[pos];
    let stream_id = payload[pos + 1];
    let info_len = u16::from_be_bytes([payload[pos + 2], payload[pos + 3]]) as usize;
    let desc_start = pos + 4;
    let desc_end = desc_start + info_len;
    let clamped_desc_end = desc_end.min(map_end);
    entries.push(PsmEntry {
      stream_type,
      elementary_stream_id: stream_id,
      descriptors: payload[desc_start..clamped_desc_end].to_vec(),
    });
    if desc_end > map_end {
      break;
    }
    pos = desc_end;
  }
  Ok(ProgramStreamMap { entries })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn build_psm(entries: &[(u8, u8, &[u8])]) -> Vec<u8> {
    let mut esi = Vec::new();
    for (st, sid, descs) in entries {
      esi.push(*st);
      esi.push(*sid);
      esi.extend_from_slice(&(descs.len() as u16).to_be_bytes());
      esi.extend_from_slice(descs);
    }
    let esi_len = esi.len() as u16;
    let psi_len = 0u16;
    let mut body = Vec::new();
    body.extend_from_slice(&0u16.to_be_bytes());
    body.push(0xC1); // current_next + version
    body.push(0x01); // marker
    body.extend_from_slice(&psi_len.to_be_bytes());
    body.extend_from_slice(&esi_len.to_be_bytes());
    body.extend(esi);
    body.extend_from_slice(&0u32.to_be_bytes()); // CRC
    let psm_len = (body.len() - 2) as u16;
    body[..2].copy_from_slice(&psm_len.to_be_bytes());
    body
  }

  #[test]
  fn parses_psm_with_two_streams() {
    let payload = build_psm(&[(0x1B, 0xE0, &[]), (0x0F, 0xC0, &[])]);
    let psm = parse(&payload).unwrap();
    assert_eq!(psm.entries.len(), 2);
    assert_eq!(psm.entries[0].stream_type, 0x1B);
    assert_eq!(psm.entries[1].elementary_stream_id, 0xC0);
  }

  #[test]
  fn rejects_truncated_payload() {
    let err = parse(&[0u8; 4]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_zero_or_overlarge_declared_length() {
    let err = parse(&[0u8, 0, 0xC1, 0x01]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
    let mut payload = build_psm(&[]);
    payload[..2].copy_from_slice(&1019u16.to_be_bytes());
    let err = parse(&payload).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_es_map_length_overrun() {
    let mut payload = build_psm(&[]);
    // Inflate esi_len to overrun
    payload[6] = 0xFF;
    payload[7] = 0xFF;
    let err = parse(&payload).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn records_entry_before_overlong_descriptor_skip() {
    let mut payload = build_psm(&[(0x1B, 0xE0, &[1, 2, 3, 4])]);
    // Force the entry's info_len to overrun the map
    let pos = 8; // after program_stream_map_length + version/marker + psi_len + esi_len
    payload[pos + 2] = 0xFF;
    payload[pos + 3] = 0xFF;
    let psm = parse(&payload).unwrap();
    assert_eq!(psm.entries.len(), 1);
    assert_eq!(psm.entries[0].stream_type, 0x1B);
    assert_eq!(psm.entries[0].elementary_stream_id, 0xE0);
    assert_eq!(psm.entries[0].descriptors, vec![1, 2, 3, 4]);
  }

  #[test]
  fn entry_descriptors_round_trip() {
    let descs = vec![0x0A, 0x04, b'e', b'n', b'g', 0x00];
    let payload = build_psm(&[(0x06, 0xBD, &descs)]);
    let psm = parse(&payload).unwrap();
    assert_eq!(psm.entries[0].descriptors, descs);
  }

  #[test]
  fn ignores_bytes_after_declared_map() {
    let mut payload = build_psm(&[(0x0F, 0xC0, &[])]);
    payload.extend_from_slice(&[0x1B, 0xE0, 0x00, 0x00]);
    let psm = parse(&payload).unwrap();
    assert_eq!(psm.entries.len(), 1);
    assert_eq!(psm.entries[0].elementary_stream_id, 0xC0);
  }
}
