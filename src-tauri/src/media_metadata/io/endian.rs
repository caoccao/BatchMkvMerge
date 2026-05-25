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

//! Endian-aware byte-slice readers. Mirrors `mkvtoolnix/src/common/endian.h`
//! but uses Rust's primitive `from_*e_bytes` rather than hand-rolled shifts.
//! All functions panic on an over-short slice — callers must validate length
//! before calling (typically via `BitReader` / `FileSource` which check first).

// --- Big-endian unsigned --------------------------------------------------

pub fn get_u16_be(buf: &[u8]) -> u16 {
  u16::from_be_bytes([buf[0], buf[1]])
}

pub fn get_u24_be(buf: &[u8]) -> u32 {
  ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32)
}

pub fn get_u32_be(buf: &[u8]) -> u32 {
  u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]])
}

pub fn get_u40_be(buf: &[u8]) -> u64 {
  ((buf[0] as u64) << 32) | ((buf[1] as u64) << 24) | ((buf[2] as u64) << 16) | ((buf[3] as u64) << 8) | (buf[4] as u64)
}

pub fn get_u48_be(buf: &[u8]) -> u64 {
  ((buf[0] as u64) << 40)
    | ((buf[1] as u64) << 32)
    | ((buf[2] as u64) << 24)
    | ((buf[3] as u64) << 16)
    | ((buf[4] as u64) << 8)
    | (buf[5] as u64)
}

pub fn get_u56_be(buf: &[u8]) -> u64 {
  ((buf[0] as u64) << 48)
    | ((buf[1] as u64) << 40)
    | ((buf[2] as u64) << 32)
    | ((buf[3] as u64) << 24)
    | ((buf[4] as u64) << 16)
    | ((buf[5] as u64) << 8)
    | (buf[6] as u64)
}

pub fn get_u64_be(buf: &[u8]) -> u64 {
  u64::from_be_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]])
}

// --- Little-endian unsigned ----------------------------------------------

pub fn get_u16_le(buf: &[u8]) -> u16 {
  u16::from_le_bytes([buf[0], buf[1]])
}

pub fn get_u24_le(buf: &[u8]) -> u32 {
  (buf[0] as u32) | ((buf[1] as u32) << 8) | ((buf[2] as u32) << 16)
}

pub fn get_u32_le(buf: &[u8]) -> u32 {
  u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
}

pub fn get_u64_le(buf: &[u8]) -> u64 {
  u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]])
}

// --- Signed BE/LE -- two's complement reinterpretation of the unsigned read

pub fn get_i16_be(buf: &[u8]) -> i16 {
  get_u16_be(buf) as i16
}
pub fn get_i32_be(buf: &[u8]) -> i32 {
  get_u32_be(buf) as i32
}
pub fn get_i64_be(buf: &[u8]) -> i64 {
  get_u64_be(buf) as i64
}
pub fn get_i16_le(buf: &[u8]) -> i16 {
  get_u16_le(buf) as i16
}
pub fn get_i32_le(buf: &[u8]) -> i32 {
  get_u32_le(buf) as i32
}
pub fn get_i64_le(buf: &[u8]) -> i64 {
  get_u64_le(buf) as i64
}

// --- Floats — IEEE-754 reinterpret from the matching uint read -----------

pub fn get_f32_be(buf: &[u8]) -> f32 {
  f32::from_bits(get_u32_be(buf))
}
pub fn get_f64_be(buf: &[u8]) -> f64 {
  f64::from_bits(get_u64_be(buf))
}
pub fn get_f32_le(buf: &[u8]) -> f32 {
  f32::from_bits(get_u32_le(buf))
}
pub fn get_f64_le(buf: &[u8]) -> f64 {
  f64::from_bits(get_u64_le(buf))
}

/// EBML-style variable-width unsigned big-endian decode of 1-8 bytes.
/// Used for `KaxBlockGroup` reference timestamps and Tags `TargetValue`.
/// Panics if `buf.len() < width` or `width == 0 || width > 8`.
pub fn get_uint_be(buf: &[u8], width: usize) -> u64 {
  assert!((1..=8).contains(&width), "get_uint_be width out of range: {width}");
  let mut acc: u64 = 0;
  for i in 0..width {
    acc = (acc << 8) | (buf[i] as u64);
  }
  acc
}

/// Like [`get_uint_be`] but sign-extends from the leading bit of the first
/// byte. Mirrors `endian.h::get_int_be(_, n)` for `KaxSegment` deltas.
pub fn get_int_be(buf: &[u8], width: usize) -> i64 {
  let raw = get_uint_be(buf, width) as i64;
  let bits = (width as u32) * 8;
  if bits == 64 {
    raw
  } else {
    // sign-extend `bits` to 64
    let shift = 64 - bits;
    (raw << shift) >> shift
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn u16_be_le_round_trip() {
    let bytes = [0x12, 0x34];
    assert_eq!(get_u16_be(&bytes), 0x1234);
    assert_eq!(get_u16_le(&bytes), 0x3412);
  }

  #[test]
  fn u24_be_le_round_trip() {
    let bytes = [0xAB, 0xCD, 0xEF];
    assert_eq!(get_u24_be(&bytes), 0x00AB_CDEF);
    assert_eq!(get_u24_le(&bytes), 0x00EF_CDAB);
  }

  #[test]
  fn u32_be_le_round_trip() {
    let bytes = [0xDE, 0xAD, 0xBE, 0xEF];
    assert_eq!(get_u32_be(&bytes), 0xDEAD_BEEF);
    assert_eq!(get_u32_le(&bytes), 0xEFBE_ADDE);
  }

  #[test]
  fn u64_be_le_round_trip() {
    let bytes = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF];
    assert_eq!(get_u64_be(&bytes), 0x0123_4567_89AB_CDEF);
    assert_eq!(get_u64_le(&bytes), 0xEFCD_AB89_6745_2301);
  }

  #[test]
  fn u40_u48_u56_be() {
    let bytes = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
    assert_eq!(get_u40_be(&bytes), 0x0102030405);
    assert_eq!(get_u48_be(&bytes), 0x010203040506);
    assert_eq!(get_u56_be(&bytes), 0x01020304050607);
  }

  #[test]
  fn signed_be_sign_extends() {
    // 0xFFFE = -2 as i16
    assert_eq!(get_i16_be(&[0xFF, 0xFE]), -2);
    // 0xFFFF_FFFE = -2 as i32
    assert_eq!(get_i32_be(&[0xFF, 0xFF, 0xFF, 0xFE]), -2);
  }

  #[test]
  fn signed_le_sign_extends() {
    assert_eq!(get_i16_le(&[0xFE, 0xFF]), -2);
    assert_eq!(get_i32_le(&[0xFE, 0xFF, 0xFF, 0xFF]), -2);
  }

  #[test]
  fn float_be_roundtrips_one_point_zero() {
    let f32_one_be: [u8; 4] = 1.0_f32.to_be_bytes();
    let f64_one_be: [u8; 8] = 1.0_f64.to_be_bytes();
    assert_eq!(get_f32_be(&f32_one_be), 1.0);
    assert_eq!(get_f64_be(&f64_one_be), 1.0);
  }

  #[test]
  fn float_le_roundtrips_minus_one() {
    let f32_le: [u8; 4] = (-1.0_f32).to_le_bytes();
    let f64_le: [u8; 8] = (-1.0_f64).to_le_bytes();
    assert_eq!(get_f32_le(&f32_le), -1.0);
    assert_eq!(get_f64_le(&f64_le), -1.0);
  }

  #[test]
  fn variable_width_uint_be() {
    let bytes = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
    assert_eq!(get_uint_be(&bytes, 1), 0xAA);
    assert_eq!(get_uint_be(&bytes, 2), 0xAABB);
    assert_eq!(get_uint_be(&bytes, 3), 0xAABBCC);
    assert_eq!(get_uint_be(&bytes, 4), 0xAABBCCDD);
    assert_eq!(get_uint_be(&bytes, 5), 0xAABBCCDDEE);
  }

  #[test]
  fn variable_width_int_be_sign_extends_from_msb() {
    // 0x80 with width 1 → -128
    assert_eq!(get_int_be(&[0x80], 1), -128);
    // 0x7F with width 1 → 127
    assert_eq!(get_int_be(&[0x7F], 1), 127);
    // 0xFFFE with width 2 → -2
    assert_eq!(get_int_be(&[0xFF, 0xFE], 2), -2);
    // 3-byte negative: 0xFFFFFE → -2 once sign-extended
    assert_eq!(get_int_be(&[0xFF, 0xFF, 0xFE], 3), -2);
    // 8-byte: just reinterpret
    assert_eq!(get_int_be(&[0xFF; 8], 8), u64::MAX as i64);
  }

  #[test]
  #[should_panic(expected = "get_uint_be width out of range")]
  fn variable_width_uint_panics_on_zero_width() {
    let _ = get_uint_be(&[0x00], 0);
  }

  #[test]
  #[should_panic(expected = "get_uint_be width out of range")]
  fn variable_width_uint_panics_on_overlong_width() {
    let _ = get_uint_be(&[0x00; 16], 9);
  }
}
