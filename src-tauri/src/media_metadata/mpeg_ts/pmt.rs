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

//! Program Map Table (PMT) decoder.
//!
//! Layout (ISO/IEC 13818-1 §2.4.4.8):
//!
//! ```text
//! u8  table_id (== 2)
//! u16 section_syntax | '0' | reserved(2) | section_length(12)
//! u16 program_number
//! u8  reserved(2) | version_number(5) | current_next(1)
//! u8  section_number
//! u8  last_section_number
//! u16 reserved(3) | PCR_PID(13)
//! u16 reserved(4) | program_info_length(12)
//! [program_info_length bytes of program descriptors]
//! repeat:
//!   u8  stream_type
//!   u16 reserved(3) | elementary_PID(13)
//!   u16 reserved(4) | ES_info_length(12)
//!   [ES_info_length bytes of per-stream descriptors]
//! u32 CRC32
//! ```

use crate::media_metadata::error::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmtStreamEntry {
  pub stream_type: u8,
  pub elementary_pid: u16,
  /// Concatenated descriptor bytes — left for the descriptor walker to decode.
  pub descriptors: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Pmt {
  pub program_number: u16,
  pub pcr_pid: u16,
  /// Program-level descriptors (apply to every stream in the program).
  pub program_descriptors: Vec<u8>,
  pub streams: Vec<PmtStreamEntry>,
}

pub fn parse(section: &[u8]) -> Result<Pmt, ParseError> {
  if section.len() < 16 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("PMT section {} bytes too small", section.len()),
    });
  }
  if section[0] != 0x02 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("PMT table_id 0x{:02X} != 0x02", section[0]),
    });
  }
  // PARSER-270: mandatory PSI header flags mkvtoolnix enforces
  // (`r_mpeg_ts.cpp:1928-1940`).  An inactive next-version section or any
  // multi-section PMT is rejected.  CRC32 is intentionally not enforced — see
  // the PAT module doc for the upstream retry-fallback rationale.
  if section[1] & 0x80 == 0 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 1,
      reason: "PMT section_syntax_indicator != 1".to_string(),
    });
  }
  if section[5] & 0x01 == 0 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 5,
      reason: "PMT current_next_indicator == 0 (inactive section)".to_string(),
    });
  }
  if section[6] != 0 || section[7] != 0 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 6,
      reason: "unsupported multi-section PMT".to_string(),
    });
  }
  let section_length = (((section[1] as usize) & 0x0F) << 8) | section[2] as usize;
  // PARSER-270: section_length bounds (`r_mpeg_ts.cpp:1961-1964`).
  if !(13..=1021).contains(&section_length) {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 1,
      reason: format!("PMT section_length {} out of range (13..=1021)", section_length),
    });
  }
  if section.len() < 3 + section_length {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!(
        "PMT declares {} bytes but buffer has {}",
        3 + section_length,
        section.len()
      ),
    });
  }
  let program_number = u16::from_be_bytes([section[3], section[4]]);
  let pcr_pid = (((section[8] as u16) & 0x1F) << 8) | section[9] as u16;
  let program_info_length = (((section[10] as usize) & 0x0F) << 8) | section[11] as usize;
  let mut pos = 12usize;
  let table_end = 3 + section_length - 4; // strip CRC32

  if pos + program_info_length > table_end {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: "PMT program_info_length overruns section".to_string(),
    });
  }
  let program_descriptors = section[pos..pos + program_info_length].to_vec();
  pos += program_info_length;

  let mut streams = Vec::new();
  while pos + 5 <= table_end {
    let stream_type = section[pos];
    let elementary_pid = (((section[pos + 1] as u16) & 0x1F) << 8) | section[pos + 2] as u16;
    let es_info_length = (((section[pos + 3] as usize) & 0x0F) << 8) | section[pos + 4] as usize;
    let desc_start = pos + 5;
    let desc_end = desc_start + es_info_length;
    if desc_end > table_end {
      // Mkvtoolnix's behaviour is to bail on malformed PMT; matching that
      // by stopping at the last good entry.
      break;
    }
    streams.push(PmtStreamEntry {
      stream_type,
      elementary_pid,
      descriptors: section[desc_start..desc_end].to_vec(),
    });
    pos = desc_end;
  }
  Ok(Pmt {
    program_number,
    pcr_pid,
    program_descriptors,
    streams,
  })
}

#[cfg(test)]
pub(crate) fn build_section(
  program_number: u16,
  pcr_pid: u16,
  program_descriptors: &[u8],
  streams: &[(u8, u16, Vec<u8>)],
) -> Vec<u8> {
  let mut body_after_header = Vec::new();
  body_after_header.extend_from_slice(&program_number.to_be_bytes());
  body_after_header.push(0xC1);
  body_after_header.push(0x00);
  body_after_header.push(0x00);
  body_after_header.extend_from_slice(&(0xE000 | (pcr_pid & 0x1FFF)).to_be_bytes());
  let pil = program_descriptors.len() as u16;
  body_after_header.extend_from_slice(&(0xF000 | (pil & 0x0FFF)).to_be_bytes());
  body_after_header.extend_from_slice(program_descriptors);
  for (stream_type, pid, descs) in streams {
    body_after_header.push(*stream_type);
    body_after_header.extend_from_slice(&(0xE000 | (pid & 0x1FFF)).to_be_bytes());
    let dl = descs.len() as u16;
    body_after_header.extend_from_slice(&(0xF000 | (dl & 0x0FFF)).to_be_bytes());
    body_after_header.extend_from_slice(descs);
  }
  body_after_header.extend_from_slice(&0u32.to_be_bytes()); // CRC placeholder
  let section_length = body_after_header.len() as u16;
  let mut section = Vec::new();
  section.push(0x02);
  section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F));
  section.push((section_length & 0xFF) as u8);
  section.extend_from_slice(&body_after_header);
  section
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_pmt_with_two_streams() {
    let section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![]), (0x0F, 0x111, vec![])]);
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.program_number, 1);
    assert_eq!(pmt.pcr_pid, 0x100);
    assert_eq!(pmt.streams.len(), 2);
    assert_eq!(pmt.streams[0].stream_type, 0x1B); // H.264
    assert_eq!(pmt.streams[1].elementary_pid, 0x111);
  }

  #[test]
  fn extracts_program_descriptors() {
    let section = build_section(1, 0x100, &[0xCA, 0x06, 1, 2, 3, 4, 5, 6], &[]);
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.program_descriptors, vec![0xCA, 0x06, 1, 2, 3, 4, 5, 6]);
  }

  #[test]
  fn extracts_per_stream_descriptors() {
    let section = build_section(
      1,
      0x100,
      &[],
      &[(0x06, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00])],
    );
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.streams[0].descriptors[0], 0x0A);
    assert_eq!(pmt.streams[0].descriptors.len(), 6);
  }

  #[test]
  fn rejects_wrong_table_id() {
    let mut section = build_section(1, 0x100, &[], &[]);
    section[0] = 0x05;
    let err = parse(&section).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_section_too_small() {
    let err = parse(&[0u8; 8]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_program_info_length_overrun() {
    let mut section = build_section(1, 0x100, &[], &[(0x06, 0x110, vec![])]);
    // Inflate program_info_length
    section[10] = 0xFF;
    section[11] = 0xFF;
    let err = parse(&section).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn stops_at_truncated_stream_entry() {
    // Build a PMT with two streams, then overrun the SECOND entry's
    // ES_info_length so the parser bails on it.  Tail layout (last byte
    // first):
    //   crc[3] crc[2] crc[1] crc[0] desc[3] desc[2] desc[1] desc[0]
    //   len_lo len_hi pid_lo pid_hi stream_type ...
    // ⇒ len_hi/lo live at section.len()-10 / -9.
    let mut section = build_section(
      1,
      0x100,
      &[],
      &[(0x1B, 0x110, vec![]), (0x0F, 0x111, vec![0xAA, 0xBB, 0xCC, 0xDD])],
    );
    let len = section.len();
    section[len - 10] = 0xFF;
    section[len - 9] = 0xFF;
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.streams.len(), 1);
  }

  #[test]
  fn empty_stream_list_returns_empty() {
    let section = build_section(1, 0x100, &[], &[]);
    let pmt = parse(&section).unwrap();
    assert!(pmt.streams.is_empty());
  }

  #[test]
  fn pcr_pid_decoded_correctly() {
    let section = build_section(1, 0x1FF, &[], &[]);
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.pcr_pid, 0x1FF);
  }

  // ---- PARSER-270: mandatory PSI header validation ---------------------

  #[test]
  fn rejects_section_syntax_indicator_zero() {
    let mut section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![])]);
    section[1] &= !0x80;
    assert!(matches!(parse(&section).unwrap_err(), ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_inactive_current_next_indicator() {
    let mut section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![])]);
    section[5] &= !0x01;
    assert!(matches!(parse(&section).unwrap_err(), ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_multi_section_pmt() {
    let mut section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![])]);
    section[6] = 1; // section_number != 0
    assert!(matches!(parse(&section).unwrap_err(), ParseError::Malformed { .. }));
    let mut section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![])]);
    section[7] = 1; // last_section_number != 0
    assert!(matches!(parse(&section).unwrap_err(), ParseError::Malformed { .. }));
  }

  #[test]
  fn accepts_single_active_section() {
    let section = build_section(1, 0x100, &[], &[(0x1B, 0x110, vec![])]);
    let pmt = parse(&section).unwrap();
    assert_eq!(pmt.streams.len(), 1);
  }
}
