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

//! MPEG-4 Part 2 (DivX / Xvid) Visual-Object-Layer pixel-aspect-ratio
//! extraction — port of `mtx::mpeg4_p2::extract_par` and `find_vol_header`
//! (`../mkvtoolnix/src/common/mpeg4_p2.cpp`).
//!
//! AVI files store DivX/Xvid PAR only in the elementary bit-stream, so
//! `avi_reader_c::extended_identify_mpeg4_l2` reads the first video frame and
//! decodes the VOL header's `aspect_ratio_info` field to report the anamorphic
//! display dimensions (PARSER-241).

use crate::media_metadata::io::bit_reader::BitReader;

/// `ar_nums` / `ar_dens` from `extract_par_internal` (`mpeg4_p2.cpp:134-135`).
const AR_NUMS: [u32; 16] = [0, 1, 12, 10, 16, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const AR_DENS: [u32; 16] = [1, 1, 11, 11, 11, 33, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];

/// `true` for the FOURCCs mkvtoolnix maps to `V_MPEG4_P2`
/// (`codec.cpp:143`: `3iv2|xvi[dx]|divx|dx50|fmp4|mp4v`, case-insensitive).
pub fn is_mpeg4_p2(fourcc: &str) -> bool {
  let upper = fourcc.to_ascii_uppercase();
  matches!(
    upper.as_str(),
    "3IV2" | "XVID" | "XVIX" | "DIVX" | "DX50" | "FMP4" | "MP4V"
  )
}

/// Find the byte offset of a Visual-Object-Layer start code (`00 00 01` plus a
/// `0x20..=0x2F` VOL id byte), mirroring `find_vol_header` (`mpeg4_p2.cpp:39-60`).
fn find_vol_header(buffer: &[u8]) -> Option<usize> {
  let mut i = 0usize;
  while i + 4 <= buffer.len() {
    if buffer[i] == 0 && buffer[i + 1] == 0 && buffer[i + 2] == 1 && (0x20..=0x2f).contains(&buffer[i + 3]) {
      return Some(i);
    }
    i += 1;
  }
  None
}

/// Extract the pixel aspect ratio `(num, den)` from an MPEG-4 Part 2 video
/// frame, or `None` when the VOL header carries no non-trivial PAR.  Port of
/// `extract_par_internal` (`mpeg4_p2.cpp:129-168`).
pub fn extract_par(buffer: &[u8]) -> Option<(u32, u32)> {
  let vol = find_vol_header(buffer)?;
  // Skip the 32-bit start code (`00 00 01` + VOL id byte).
  let body = buffer.get(vol + 4..)?;
  let mut bits = BitReader::new(body);
  bits.skip_bits(1).ok()?; // random access
  bits.skip_bits(8).ok()?; // vo_type
  if bits.read_bit().ok()? {
    // is_old_id
    bits.skip_bits(4).ok()?; // vo_ver_id
    bits.skip_bits(3).ok()?; // vo_priority
  }
  let aspect_ratio_info = bits.read_bits(4).ok()? as usize;
  let (num, den) = if aspect_ratio_info == 15 {
    // ASPECT_EXTENDED
    (bits.read_bits(8).ok()? as u32, bits.read_bits(8).ok()? as u32)
  } else {
    (AR_NUMS[aspect_ratio_info], AR_DENS[aspect_ratio_info])
  };
  // mkvtoolnix keeps only a non-zero, non-1:1 ratio (`mpeg4_p2.cpp:160-166`).
  if num != 0 && den != 0 && num != den {
    Some((num, den))
  } else {
    None
  }
}

/// Apply a pixel aspect ratio to the coded dimensions to obtain display
/// dimensions, mirroring `avi_reader_c::extended_identify_mpeg4_l2`
/// (`r_avi.cpp:851-862`).  When `par_num > par_den` the width is stretched,
/// otherwise the height is.
pub fn display_dimensions(width: u32, height: u32, par_num: u32, par_den: u32) -> (u32, u32) {
  if par_num == 0 || par_den == 0 {
    return (width, height);
  }
  if par_num > par_den {
    let disp_width = ((width as u64 * par_num as u64 + par_den as u64 / 2) / par_den as u64) as u32;
    (disp_width, height)
  } else {
    let disp_height = ((height as u64 * par_den as u64 + par_num as u64 / 2) / par_num as u64) as u32;
    (width, disp_height)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Build a minimal MPEG-4 Part 2 VOL header carrying `aspect_ratio_info`.
  /// Layout: `00 00 01 20` start code, then `random_access(1)`, `vo_type(8)`,
  /// `is_old_id(1)=0`, `aspect_ratio_info(4)`, `[extended num(8)/den(8)]`.
  fn build_vol(aspect_ratio_info: u8, extended: Option<(u8, u8)>) -> Vec<u8> {
    let mut out = vec![0x00, 0x00, 0x01, 0x20];
    // Hand-pack the bit-stream after the start code.
    let mut bit_buf: Vec<bool> = Vec::new();
    bit_buf.push(false); // random access
    for _ in 0..8 {
      bit_buf.push(false); // vo_type
    }
    bit_buf.push(false); // is_old_id = 0
    for i in (0..4).rev() {
      bit_buf.push((aspect_ratio_info >> i) & 1 != 0);
    }
    if let Some((num, den)) = extended {
      for i in (0..8).rev() {
        bit_buf.push((num >> i) & 1 != 0);
      }
      for i in (0..8).rev() {
        bit_buf.push((den >> i) & 1 != 0);
      }
    }
    // Pack bits MSB-first into bytes.
    let mut byte = 0u8;
    let mut count = 0u8;
    for b in bit_buf {
      byte = (byte << 1) | (b as u8);
      count += 1;
      if count == 8 {
        out.push(byte);
        byte = 0;
        count = 0;
      }
    }
    if count > 0 {
      out.push(byte << (8 - count));
    }
    out
  }

  #[test]
  fn detects_mpeg4_p2_fourccs() {
    assert!(is_mpeg4_p2("XVID"));
    assert!(is_mpeg4_p2("xvid"));
    assert!(is_mpeg4_p2("DIVX"));
    assert!(is_mpeg4_p2("DX50"));
    assert!(is_mpeg4_p2("MP4V"));
    assert!(!is_mpeg4_p2("H264"));
    assert!(!is_mpeg4_p2("MJPG"));
  }

  #[test]
  fn extract_par_reads_predefined_ratio() {
    // aspect_ratio_info 2 → 12:11.
    let vol = build_vol(2, None);
    assert_eq!(extract_par(&vol), Some((12, 11)));
  }

  #[test]
  fn extract_par_reads_extended_ratio() {
    // aspect_ratio_info 15 (ASPECT_EXTENDED) → explicit 40:33.
    let vol = build_vol(15, Some((40, 33)));
    assert_eq!(extract_par(&vol), Some((40, 33)));
  }

  #[test]
  fn extract_par_rejects_square_pixels() {
    // aspect_ratio_info 1 → 1:1 (square) → no PAR.
    let vol = build_vol(1, None);
    assert_eq!(extract_par(&vol), None);
  }

  #[test]
  fn extract_par_returns_none_without_vol_header() {
    assert_eq!(extract_par(&[0xAA; 32]), None);
  }

  #[test]
  fn display_dimensions_stretch_width_when_par_gt_one() {
    // 720x576 with 16:11 PAR → width stretched to round(720*16/11) = 1047.
    assert_eq!(display_dimensions(720, 576, 16, 11), (1047, 576));
  }

  #[test]
  fn display_dimensions_stretch_height_when_par_lt_one() {
    // 720x480 with 10:11 PAR → height stretched to round(480*11/10) = 528.
    assert_eq!(display_dimensions(720, 480, 10, 11), (720, 528));
  }
}
