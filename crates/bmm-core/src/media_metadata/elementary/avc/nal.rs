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

//! AVC/H.264 NAL unit parser.
//!
//! Annex B byte stream framing (ITU-T H.264 §B.1):
//!
//! ```text
//! NAL_unit_byte_stream ::= (zero_byte? start_code_prefix nal_unit)+
//! start_code_prefix    ::= 0x00 0x00 0x01
//! nal_unit             ::= forbidden_zero_bit(1) | nal_ref_idc(2) | nal_unit_type(5) | RBSP
//! ```
//!
//! Inside the RBSP the byte sequence `0x00 0x00 0x03` is the
//! emulation-prevention triplet — the `0x03` byte must be stripped to
//! recover the raw byte stream payload (RBSP without escape bytes).

pub const NAL_UNIT_TYPE_SLICE: u8 = 1;
pub const NAL_UNIT_TYPE_DP_A_SLICE: u8 = 2;
pub const NAL_UNIT_TYPE_DP_B_SLICE: u8 = 3;
pub const NAL_UNIT_TYPE_DP_C_SLICE: u8 = 4;
pub const NAL_UNIT_TYPE_IDR_SLICE: u8 = 5;
pub const NAL_UNIT_TYPE_SPS: u8 = 7;
pub const NAL_UNIT_TYPE_PPS: u8 = 8;
pub const NAL_UNIT_TYPE_AUD: u8 = 9;
pub const NAL_UNIT_TYPE_END_OF_SEQ: u8 = 10;
pub const NAL_UNIT_TYPE_END_OF_STREAM: u8 = 11;
pub const NAL_UNIT_TYPE_FILLER: u8 = 12;

#[derive(Debug, Clone, Copy)]
pub struct NalUnit<'a> {
  /// Absolute offset within the source buffer where the NAL payload begins
  /// (i.e. *after* the 3- or 4-byte start code).
  pub start: usize,
  pub nal_unit_type: u8,
  pub nal_ref_idc: u8,
  pub payload: &'a [u8],
}

/// Find every NAL unit in an Annex B byte stream.
pub fn split_nal_units(bytes: &[u8]) -> Vec<NalUnit<'_>> {
  let mut units = Vec::new();
  let starts = find_start_codes(bytes);
  for window in starts.windows(2) {
    let (start, _) = window[0];
    let (next_start, _) = window[1];
    if let Some(unit) = build_unit(bytes, start, next_start) {
      units.push(unit);
    }
  }
  if let Some(&(start, sc_len)) = starts.last() {
    let next_start = bytes.len();
    let _ = sc_len;
    if let Some(unit) = build_unit(bytes, start, next_start) {
      units.push(unit);
    }
  }
  units
}

/// Find every Annex B start-code position.  Returns `(offset, prefix_length)`
/// for each — prefix_length is 3 (`00 00 01`) or 4 (`00 00 00 01`).
pub fn find_start_codes(bytes: &[u8]) -> Vec<(usize, usize)> {
  let mut positions = Vec::new();
  let mut i = 0usize;
  while i + 3 <= bytes.len() {
    if bytes[i] == 0x00 && bytes[i + 1] == 0x00 {
      if bytes.get(i + 2) == Some(&0x01) {
        positions.push((i, 3));
        i += 3;
        continue;
      }
      if bytes.get(i + 2) == Some(&0x00) && bytes.get(i + 3) == Some(&0x01) {
        positions.push((i, 4));
        i += 4;
        continue;
      }
    }
    i += 1;
  }
  positions
}

fn build_unit(bytes: &[u8], start_code_pos: usize, next_start: usize) -> Option<NalUnit<'_>> {
  let sc_len = if bytes.get(start_code_pos + 2) == Some(&0x01) {
    3
  } else {
    4
  };
  let nal_byte_pos = start_code_pos + sc_len;
  if nal_byte_pos >= bytes.len() || nal_byte_pos >= next_start {
    return None;
  }
  let header_byte = bytes[nal_byte_pos];
  let nal_ref_idc = (header_byte >> 5) & 0x03;
  let nal_unit_type = header_byte & 0x1F;
  let payload = &bytes[nal_byte_pos + 1..next_start];
  Some(NalUnit {
    start: nal_byte_pos + 1,
    nal_unit_type,
    nal_ref_idc,
    payload,
  })
}

/// Remove the emulation-prevention bytes from a NAL payload — gives the
/// RBSP (Raw Byte Sequence Payload) the bit-stream parser actually consumes.
pub fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
  let mut out = Vec::with_capacity(payload.len());
  let mut i = 0;
  while i < payload.len() {
    if i + 2 < payload.len() && payload[i] == 0x00 && payload[i + 1] == 0x00 && payload[i + 2] == 0x03 {
      out.push(0x00);
      out.push(0x00);
      i += 3;
      continue;
    }
    out.push(payload[i]);
    i += 1;
  }
  out
}

#[cfg(test)]
pub(crate) fn build_annex_b(payloads: &[(u8, &[u8])]) -> Vec<u8> {
  let mut bytes = Vec::new();
  for (nal_unit_type, body) in payloads {
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    bytes.push(*nal_unit_type & 0x1F);
    bytes.extend_from_slice(body);
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finds_start_codes_with_3_and_4_byte_prefixes() {
    let bytes = [
      0x00, 0x00, 0x01, 0x67, // 3-byte SPS
      0x00, 0x00, 0x00, 0x01, 0x68, // 4-byte PPS
    ];
    let positions = find_start_codes(&bytes);
    assert_eq!(positions, vec![(0, 3), (4, 4)]);
  }

  #[test]
  fn split_nal_units_returns_each_payload() {
    let bytes = build_annex_b(&[
      (NAL_UNIT_TYPE_SPS, &[0x42, 0x00, 0x1F]),
      (NAL_UNIT_TYPE_PPS, &[0x68, 0xEB, 0xE3]),
    ]);
    let units = split_nal_units(&bytes);
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].nal_unit_type, NAL_UNIT_TYPE_SPS);
    assert_eq!(units[1].nal_unit_type, NAL_UNIT_TYPE_PPS);
    assert_eq!(units[0].payload, &[0x42, 0x00, 0x1F]);
  }

  #[test]
  fn strip_emulation_removes_03_after_double_zero() {
    let payload = [0x00, 0x00, 0x03, 0xFF, 0xAB, 0x00, 0x00, 0x03, 0x80];
    let stripped = strip_emulation_prevention(&payload);
    assert_eq!(stripped, vec![0x00, 0x00, 0xFF, 0xAB, 0x00, 0x00, 0x80]);
  }

  #[test]
  fn strip_emulation_keeps_unrelated_bytes() {
    let payload = [0xAA, 0xBB, 0xCC];
    let stripped = strip_emulation_prevention(&payload);
    assert_eq!(stripped, vec![0xAA, 0xBB, 0xCC]);
  }

  #[test]
  fn strip_emulation_empty_passes_through() {
    assert_eq!(strip_emulation_prevention(&[]), Vec::<u8>::new());
  }

  #[test]
  fn split_returns_empty_for_no_start_code() {
    assert!(split_nal_units(&[0xAA; 16]).is_empty());
  }

  #[test]
  fn nal_ref_idc_decoded_from_high_bits() {
    let bytes = [0x00, 0x00, 0x01, 0x67]; // 0x67 = 0110_0111 → nal_ref_idc = 11
    let units = split_nal_units(&bytes);
    assert_eq!(units[0].nal_ref_idc, 3);
    assert_eq!(units[0].nal_unit_type, 7);
  }

  #[test]
  fn multiple_consecutive_nal_units_split_cleanly() {
    let bytes = build_annex_b(&[
      (NAL_UNIT_TYPE_AUD, &[0xF0]),
      (NAL_UNIT_TYPE_SPS, &[0x42, 0x00, 0x1F]),
      (NAL_UNIT_TYPE_PPS, &[0xCE]),
      (NAL_UNIT_TYPE_IDR_SLICE, &[0x88, 0x84]),
    ]);
    let units = split_nal_units(&bytes);
    assert_eq!(units.len(), 4);
  }

  #[test]
  fn empty_payload_after_start_code_drops_unit() {
    // Trailing start code with no NAL byte after — should be dropped.
    let bytes = [0x00, 0x00, 0x00, 0x01];
    let units = split_nal_units(&bytes);
    assert!(units.is_empty());
  }
}
