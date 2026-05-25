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

//! Program Association Table (PAT) decoder.
//!
//! Layout (ISO/IEC 13818-1 §2.4.4.3):
//!
//! ```text
//! u8  table_id (== 0)
//! u16 section_syntax_indicator | '0' | reserved(2) | section_length(12)
//! u16 transport_stream_id
//! u8  reserved(2) | version_number(5) | current_next(1)
//! u8  section_number
//! u8  last_section_number
//! repeat:
//!   u16 program_number
//!   u16 reserved(3) | program_map_PID(13)
//! u32 CRC32
//! ```
//!
//! We accept any sections whose length fits and skip CRC verification (the
//! reader is lenient — real-world streams sometimes have stale CRCs).

use crate::media_metadata::error::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatEntry {
  pub program_number: u16,
  pub pmt_pid: u16,
}

#[derive(Debug, Clone)]
pub struct Pat {
  pub transport_stream_id: u16,
  pub entries: Vec<PatEntry>,
}

/// Decode a PSI section that should be a PAT.  The `section` slice starts
/// at the `table_id` byte (i.e. *after* the 1-byte pointer field that
/// precedes section data inside the packet payload).
pub fn parse(section: &[u8]) -> Result<Pat, ParseError> {
  if section.len() < 12 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("PAT section {} bytes too small", section.len()),
    });
  }
  if section[0] != 0x00 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("PAT table_id 0x{:02X} != 0x00", section[0]),
    });
  }
  let section_length = (((section[1] as usize) & 0x0F) << 8) | section[2] as usize;
  // section_length counts everything after the 3-byte header.  Total section
  // size is 3 + section_length.  We require the buffer to contain at least
  // that many bytes (excluding the CRC32 at the end is fine because we
  // skip it).
  if section.len() < 3 + section_length {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!(
        "PAT declares {} bytes but buffer has {}",
        3 + section_length,
        section.len()
      ),
    });
  }
  let transport_stream_id = u16::from_be_bytes([section[3], section[4]]);
  // Bytes 5..=7 hold version_number / section_number / last_section_number.
  let table_end = 3 + section_length - 4; // last 4 bytes are CRC32
  if table_end < 8 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: "PAT body shorter than its header".to_string(),
    });
  }
  let mut entries = Vec::new();
  let mut pos = 8usize;
  while pos + 4 <= table_end {
    let program_number = u16::from_be_bytes([section[pos], section[pos + 1]]);
    let pid = (((section[pos + 2] as u16) & 0x1F) << 8) | section[pos + 3] as u16;
    if program_number != 0 {
      entries.push(PatEntry {
        program_number,
        pmt_pid: pid,
      });
    }
    pos += 4;
  }
  Ok(Pat {
    transport_stream_id,
    entries,
  })
}

#[cfg(test)]
pub(crate) fn build_section(transport_stream_id: u16, entries: &[(u16, u16)]) -> Vec<u8> {
  let body_len = 5 + entries.len() * 4 + 4; // tsid + ver/sec/last + entries + CRC
  let section_length = (body_len) as u16;
  let mut section = Vec::with_capacity(3 + body_len);
  section.push(0x00); // table_id
  section.push(0xB0 | ((section_length >> 8) as u8 & 0x0F)); // section_syntax + reserved + len hi
  section.push((section_length & 0xFF) as u8);
  section.extend_from_slice(&transport_stream_id.to_be_bytes());
  section.push(0xC1); // reserved + version 0 + current_next 1
  section.push(0x00); // section_number
  section.push(0x00); // last_section_number
  for (program, pid) in entries {
    section.extend_from_slice(&program.to_be_bytes());
    let masked = 0xE000 | (pid & 0x1FFF);
    section.extend_from_slice(&masked.to_be_bytes());
  }
  section.extend_from_slice(&0u32.to_be_bytes()); // placeholder CRC
  section
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_single_program_pat() {
    let section = build_section(1, &[(1, 0x100)]);
    let pat = parse(&section).unwrap();
    assert_eq!(pat.transport_stream_id, 1);
    assert_eq!(
      pat.entries,
      vec![PatEntry {
        program_number: 1,
        pmt_pid: 0x100
      }]
    );
  }

  #[test]
  fn parses_multiple_programs() {
    let section = build_section(7, &[(1, 0x100), (2, 0x200), (3, 0x300)]);
    let pat = parse(&section).unwrap();
    assert_eq!(pat.transport_stream_id, 7);
    assert_eq!(pat.entries.len(), 3);
    assert_eq!(pat.entries[2].pmt_pid, 0x300);
  }

  #[test]
  fn skips_network_information_entry_program_number_zero() {
    // program_number 0 → NIT PID, not a real program.
    let section = build_section(1, &[(0, 0x010), (1, 0x100)]);
    let pat = parse(&section).unwrap();
    assert_eq!(
      pat.entries,
      vec![PatEntry {
        program_number: 1,
        pmt_pid: 0x100
      }]
    );
  }

  #[test]
  fn rejects_wrong_table_id() {
    let mut section = build_section(1, &[(1, 0x100)]);
    section[0] = 0x02;
    let err = parse(&section).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_section_smaller_than_header() {
    let err = parse(&[0u8; 4]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_section_length_overrunning_buffer() {
    let mut section = build_section(1, &[(1, 0x100)]);
    // Bump section_length to a huge value
    section[1] = 0xBF;
    section[2] = 0xFF;
    let err = parse(&section).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn empty_program_list_yields_empty_entries() {
    let section = build_section(42, &[]);
    let pat = parse(&section).unwrap();
    assert!(pat.entries.is_empty());
  }
}
