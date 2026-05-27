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

//! HEVC NAL unit framing.  Same Annex B byte-stream as AVC but the NAL
//! header is two bytes:
//!
//! ```text
//! 1 bit  forbidden_zero_bit
//! 6 bits nal_unit_type (0..63)
//! 6 bits nuh_layer_id
//! 3 bits nuh_temporal_id_plus1
//! ```

use super::super::avc::nal as avc_nal;

pub const NAL_UNIT_TYPE_VPS: u8 = 32;
pub const NAL_UNIT_TYPE_SPS: u8 = 33;
pub const NAL_UNIT_TYPE_PPS: u8 = 34;
pub const NAL_UNIT_TYPE_AUD: u8 = 35;
pub const NAL_UNIT_TYPE_END_OF_SEQ: u8 = 36;
pub const NAL_UNIT_TYPE_END_OF_STREAM: u8 = 37;
pub const NAL_UNIT_TYPE_FILLER: u8 = 38;
pub const NAL_UNIT_TYPE_PREFIX_SEI: u8 = 39;
pub const NAL_UNIT_TYPE_SUFFIX_SEI: u8 = 40;

#[derive(Debug, Clone, Copy)]
pub struct HevcNalUnit<'a> {
  pub nal_unit_type: u8,
  pub layer_id: u8,
  pub temporal_id_plus1: u8,
  pub payload: &'a [u8],
}

/// Walk an HEVC Annex B byte stream.
pub fn split_nal_units(bytes: &[u8]) -> Vec<HevcNalUnit<'_>> {
  let mut units = Vec::new();
  let starts = avc_nal::find_start_codes(bytes);
  for window in starts.windows(2) {
    let (start, _) = window[0];
    let (next_start, _) = window[1];
    if let Some(unit) = build_unit(bytes, start, next_start) {
      units.push(unit);
    }
  }
  if let Some(&(start, _)) = starts.last() {
    if let Some(unit) = build_unit(bytes, start, bytes.len()) {
      units.push(unit);
    }
  }
  units
}

fn build_unit(bytes: &[u8], start_code_pos: usize, next_start: usize) -> Option<HevcNalUnit<'_>> {
  let sc_len = if bytes.get(start_code_pos + 2) == Some(&0x01) {
    3
  } else {
    4
  };
  let header_pos = start_code_pos + sc_len;
  if header_pos + 2 > bytes.len() || header_pos + 2 > next_start {
    return None;
  }
  let b0 = bytes[header_pos];
  let b1 = bytes[header_pos + 1];
  let nal_unit_type = (b0 >> 1) & 0x3F;
  let layer_id = ((b0 & 0x01) << 5) | (b1 >> 3);
  let temporal_id_plus1 = b1 & 0x07;
  let payload_start = header_pos + 2;
  if payload_start > next_start {
    return None;
  }
  Some(HevcNalUnit {
    nal_unit_type,
    layer_id,
    temporal_id_plus1,
    payload: &bytes[payload_start..next_start],
  })
}

/// Reuse the AVC emulation-prevention stripper (same algorithm).
pub fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
  avc_nal::strip_emulation_prevention(payload)
}

#[cfg(test)]
pub(crate) fn build_annex_b(payloads: &[(u8, &[u8])]) -> Vec<u8> {
  let mut bytes = Vec::new();
  for (nal_unit_type, body) in payloads {
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    // HEVC NAL header byte 0 = (type << 1)  (layer_id=0, forbidden=0)
    bytes.push((nal_unit_type & 0x3F) << 1);
    bytes.push(0x01); // layer_id_low=0 + temporal_id_plus1=1
    bytes.extend_from_slice(body);
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn split_recognises_vps_sps_pps() {
    let bytes = build_annex_b(&[
      (NAL_UNIT_TYPE_VPS, &[0x01]),
      (NAL_UNIT_TYPE_SPS, &[0x02, 0x03]),
      (NAL_UNIT_TYPE_PPS, &[0x04]),
    ]);
    let units = split_nal_units(&bytes);
    assert_eq!(units.len(), 3);
    assert_eq!(units[0].nal_unit_type, NAL_UNIT_TYPE_VPS);
    assert_eq!(units[1].nal_unit_type, NAL_UNIT_TYPE_SPS);
    assert_eq!(units[2].nal_unit_type, NAL_UNIT_TYPE_PPS);
  }

  #[test]
  fn payloads_round_trip() {
    let bytes = build_annex_b(&[(NAL_UNIT_TYPE_SPS, &[0xAB, 0xCD, 0xEF])]);
    let units = split_nal_units(&bytes);
    assert_eq!(units[0].payload, &[0xAB, 0xCD, 0xEF]);
  }

  #[test]
  fn returns_empty_for_unprefixed_bytes() {
    assert!(split_nal_units(&[0xAA; 16]).is_empty());
  }

  #[test]
  fn temporal_id_decoded_from_second_header_byte() {
    // Hand-craft: type 33, layer_id 0, temporal_id_plus1 = 3
    let bytes = [0x00, 0x00, 0x00, 0x01, 0x42, 0x03, 0xAA];
    let units = split_nal_units(&bytes);
    assert_eq!(units[0].nal_unit_type, 33);
    assert_eq!(units[0].temporal_id_plus1, 3);
  }

  #[test]
  fn strip_emulation_delegates_to_shared_impl() {
    let payload = [0x00, 0x00, 0x03, 0xFF];
    assert_eq!(strip_emulation_prevention(&payload), vec![0x00, 0x00, 0xFF]);
  }
}
