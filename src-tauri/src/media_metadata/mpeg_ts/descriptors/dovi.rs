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

//! Dolby Vision video stream descriptor (tag 0xB0).  Port of
//! `r_mpeg_ts.cpp::parse_dovi_pmt_descriptor` (r_mpeg_ts.cpp:692-741).  Body:
//!
//! ```text
//! u8 dv_version_major
//! u8 dv_version_minor
//!  7 dv_profile
//!  6 dv_level
//!  1 rpu_present_flag
//!  1 bl_present_flag
//!  1 el_present_flag
//!  if !bl_present_flag:
//!    13 base_layer_pid
//!     3 (reserved)
//!  4 dv_bl_signal_compatibility_id
//!  4 (reserved)
//! ```
//!
//! Identification needs the profile and, when the base layer lives on another
//! PID, that base-layer PID (PARSER-173).

use crate::media_metadata::io::bit_reader::BitReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DoviDescriptor {
  pub profile: u32,
  /// Present only when `bl_present_flag` is false: the PID carrying the base
  /// layer (r_mpeg_ts.cpp:712-715).
  pub base_layer_pid: Option<u16>,
}

pub fn decode(body: &[u8]) -> Option<DoviDescriptor> {
  if body.len() < 3 {
    return None;
  }
  let mut r = BitReader::new(body);
  // Skip dv_version_major + dv_version_minor.
  r.skip_bits(16).ok()?;
  let profile = r.read_bits(7).ok()? as u32;
  r.skip_bits(6).ok()?; // dv_level
  r.skip_bits(1).ok()?; // rpu_present_flag
  let bl_present = r.read_bit().ok()?;
  r.skip_bits(1).ok()?; // el_present_flag

  let base_layer_pid = if !bl_present {
    let pid = r.read_bits(13).ok()? as u16;
    r.skip_bits(3).ok()?;
    Some(pid)
  } else {
    None
  };

  Some(DoviDescriptor { profile, base_layer_pid })
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Build a DV descriptor body. `bl_present` controls whether a base-layer
  /// PID is encoded after the flags.
  fn build(profile: u32, bl_present: bool, base_layer_pid: u16) -> Vec<u8> {
    // MSB-first bit packer.
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |value: u64, n: u32| {
      for i in (0..n).rev() {
        bits.push(((value >> i) & 1) as u8);
      }
    };
    push(1, 8); // dv_version_major
    push(0, 8); // dv_version_minor
    push(profile as u64, 7);
    push(0, 6); // dv_level
    push(0, 1); // rpu_present_flag
    push(if bl_present { 1 } else { 0 }, 1);
    push(0, 1); // el_present_flag
    if !bl_present {
      push(base_layer_pid as u64, 13);
      push(0, 3);
    }
    push(0, 4); // dv_bl_signal_compatibility_id
    push(0, 4);
    // Pack the bit vector into bytes (pad with zeros).
    let mut out = Vec::new();
    for chunk in bits.chunks(8) {
      let mut byte = 0u8;
      for (i, &b) in chunk.iter().enumerate() {
        byte |= b << (7 - i);
      }
      out.push(byte);
    }
    out
  }

  #[test]
  fn extracts_profile_5_with_base_layer_present() {
    let body = build(5, true, 0);
    let d = decode(&body).unwrap();
    assert_eq!(d.profile, 5);
    assert_eq!(d.base_layer_pid, None);
  }

  #[test]
  fn extracts_profile_7_and_base_layer_pid() {
    let body = build(7, false, 0x1011);
    let d = decode(&body).unwrap();
    assert_eq!(d.profile, 7);
    assert_eq!(d.base_layer_pid, Some(0x1011));
  }

  #[test]
  fn extracts_profile_8() {
    let body = build(8, true, 0);
    assert_eq!(decode(&body).unwrap().profile, 8);
  }

  #[test]
  fn rejects_truncated_body() {
    assert!(decode(&[1u8, 0]).is_none());
  }
}
