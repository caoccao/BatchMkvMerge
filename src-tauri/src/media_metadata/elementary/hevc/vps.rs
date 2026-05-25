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

//! HEVC VPS (Video Parameter Set) — we only need the IDs that downstream
//! consumers (Dolby Vision composition, mainly) cross-reference.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;

#[derive(Debug, Clone, Copy)]
pub struct VpsSummary {
  pub vps_id: u8,
  pub max_layers_minus1: u8,
  pub max_sub_layers_minus1: u8,
}

pub fn parse(rbsp: &[u8]) -> Result<VpsSummary, ParseError> {
  if rbsp.is_empty() {
    return Err(ParseError::Malformed {
      format: "hevc",
      offset: 0,
      reason: "empty VPS RBSP".to_string(),
    });
  }
  let mut reader = BitReader::from_rbsp(rbsp);
  let vps_id = reader.read_bits(4)? as u8;
  let _base_layer_internal = reader.read_bit()?;
  let _base_layer_available = reader.read_bit()?;
  let max_layers_minus1 = reader.read_bits(6)? as u8;
  let max_sub_layers_minus1 = reader.read_bits(3)? as u8;
  Ok(VpsSummary {
    vps_id,
    max_layers_minus1,
    max_sub_layers_minus1,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_vps_with_id_zero() {
    // Hand-pack bits per MSB-first BitReader:
    //   byte 0: 0000 1 1 00 = 0b0000_1100
    //     - vps_id = 0000 (4 bits)
    //     - base_layer_internal = 1
    //     - base_layer_available = 1
    //     - max_layers_minus1 high nibble = 00 (2 bits)
    //   byte 1: 0000 010_ = 0b0000_0100
    //     - max_layers_minus1 low nibble = 0000 (4 bits → total = 0)
    //     - max_sub_layers_minus1 = 010 (3 bits = 2)
    let rbsp = [0b0000_1100, 0b0000_0100, 0x80];
    let vps = parse(&rbsp).unwrap();
    assert_eq!(vps.vps_id, 0);
    assert_eq!(vps.max_layers_minus1, 0);
    assert_eq!(vps.max_sub_layers_minus1, 2);
  }

  #[test]
  fn rejects_empty_rbsp() {
    assert!(matches!(parse(&[]), Err(ParseError::Malformed { .. })));
  }
}
