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

//! VP8-in-Ogg identification header.  Port of `ogm_v_vp8_demuxer_c`
//! (`r_ogm.cpp:1536-1652`) + `mtx::ogm::vp8_header_t`
//! (`common/ogmstreams.h:103-115`):
//!
//! ```text
//! u8  header_id        (== 0x4f)
//! u32 id               (== 0x56503830, "VP80", BE)
//! u8  header_type      (== 0x01)
//! u8  version_major
//! u8  version_minor
//! u16 pixel_width      (BE)
//! u16 pixel_height     (BE)
//! 3   par_num          (BE, 24-bit pixel aspect ratio numerator)
//! 3   par_den          (BE, 24-bit pixel aspect ratio denominator)
//! u32 frame_rate_num   (BE)
//! u32 frame_rate_den   (BE)
//! ```
//!
//! `sizeof(vp8_header_t)` with the upstream packing is 26 bytes (1 + 4 + 1 + 1
//! + 1 + 2 + 2 + 3 + 3 + 4 + 4).  Mirrors the upstream sniffer at
//! `r_ogm.cpp:474`: first byte `0x4f`, next four bytes `0x56503830`.

use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use super::BitstreamMetadata;

/// VP8-in-Ogg mapping ID `0x4f` followed by "VP80" (`0x56503830`).
const HEADER_ID: u8 = 0x4f;
const MAPPING_ID: u32 = 0x5650_3830;
/// `sizeof(mtx::ogm::vp8_header_t)` — see the module doc-comment.
const HEADER_LEN: usize = 26;

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  if packet.len() < HEADER_LEN || packet[0] != HEADER_ID {
    return None;
  }
  if u32::from_be_bytes([packet[1], packet[2], packet[3], packet[4]]) != MAPPING_ID {
    return None;
  }

  // Field offsets follow `vp8_header_t` (header_id @0, id @1, header_type @5,
  // version_major @6, version_minor @7, pixel_width @8, pixel_height @10,
  // par_num[3] @12, par_den[3] @15, frame_rate_num @18, frame_rate_den @22).
  // r_ogm.cpp:1553-1573.
  let pixel_width = u16::from_be_bytes([packet[8], packet[9]]) as u32;
  let pixel_height = u16::from_be_bytes([packet[10], packet[11]]) as u32;
  // par_num / par_den are 24-bit big-endian fields (the struct stores them as
  // `uint8_t [3]`); mkvtoolnix reads them through `get_uint16_be`, i.e. only
  // the top 16 bits matter for the rational comparison.  r_ogm.cpp:1555-1556.
  let par_num = u16::from_be_bytes([packet[12], packet[13]]) as u64;
  let par_den = u16::from_be_bytes([packet[15], packet[16]]) as u64;
  let frame_rate_num = u32::from_be_bytes([packet[18], packet[19], packet[20], packet[21]]) as u64;
  let frame_rate_den = u32::from_be_bytes([packet[22], packet[23], packet[24], packet[25]]) as u64;

  // Display-dimension derivation mirrors r_ogm.cpp:1558-1570.
  let (display_width, display_height) = compute_display_dimensions(pixel_width, pixel_height, par_num, par_den);

  // default_duration = frame_rate_den / frame_rate_num * 1e9 (r_ogm.cpp:1574).
  let frame_duration_ns = if frame_rate_num != 0 && frame_rate_den != 0 {
    Some((frame_rate_den as u128 * 1_000_000_000 / frame_rate_num as u128) as u64)
  } else {
    None
  };

  let mut metadata = BitstreamMetadata::video_only("V_VP8", "VP8");
  metadata.frame_duration_ns = frame_duration_ns;
  metadata.video = Some(VideoTrackProperties {
    pixel_dimensions: if pixel_width > 0 && pixel_height > 0 {
      Some(Dimensions2D {
        width: pixel_width,
        height: pixel_height,
      })
    } else {
      None
    },
    display_dimensions: if display_width > 0 && display_height > 0 {
      Some(Dimensions2D {
        width: display_width,
        height: display_height,
      })
    } else {
      None
    },
    default_duration_ns: frame_duration_ns,
    ..VideoTrackProperties::default()
  });
  Some(metadata)
}

/// Apply the pixel-aspect-ratio adjustment from r_ogm.cpp:1558-1570.  When PAR
/// is unset the display dimensions equal the pixel dimensions.
fn compute_display_dimensions(pixel_width: u32, pixel_height: u32, par_num: u64, par_den: u64) -> (u32, u32) {
  if par_num == 0 || par_den == 0 {
    return (pixel_width, pixel_height);
  }
  // mtx::rational(pixel_width, pixel_height) < mtx::rational(par_num, par_den)
  // ⇔ pixel_width * par_den < pixel_height * par_num.
  if (pixel_width as u64) * par_den < (pixel_height as u64) * par_num {
    let display_width = round_div((pixel_width as u64) * par_num, par_den);
    (display_width, pixel_height)
  } else {
    let display_height = round_div((pixel_height as u64) * par_den, par_num);
    (pixel_width, display_height)
  }
}

/// `mtx::to_int_rounded(mtx::rational(num, den))` — round-half-to-even is not
/// required here; mkvtoolnix uses plain rounding (round half away from zero for
/// non-negative values).
fn round_div(num: u64, den: u64) -> u32 {
  if den == 0 {
    return 0;
  }
  ((num + den / 2) / den) as u32
}

#[cfg(test)]
pub(crate) fn build_identification_packet(
  pixel_width: u16,
  pixel_height: u16,
  par_num: u16,
  par_den: u16,
  frame_rate_num: u32,
  frame_rate_den: u32,
) -> Vec<u8> {
  let mut p = Vec::with_capacity(HEADER_LEN);
  p.push(HEADER_ID);
  p.extend_from_slice(&MAPPING_ID.to_be_bytes());
  p.push(0x01); // header_type
  p.push(1); // version_major
  p.push(0); // version_minor
  p.extend_from_slice(&pixel_width.to_be_bytes());
  p.extend_from_slice(&pixel_height.to_be_bytes());
  // par_num / par_den as 24-bit BE; mkvtoolnix reads the top 16 bits via
  // `get_uint16_be`, so place the value in the first two bytes.
  p.extend_from_slice(&par_num.to_be_bytes());
  p.push(0);
  p.extend_from_slice(&par_den.to_be_bytes());
  p.push(0);
  p.extend_from_slice(&frame_rate_num.to_be_bytes());
  p.extend_from_slice(&frame_rate_den.to_be_bytes());
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sniffs_640x480_at_30fps_no_par() {
    let pkt = build_identification_packet(640, 480, 0, 0, 30, 1);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "V_VP8");
    assert_eq!(m.codec_name, "VP8");
    let v = m.video.unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 640,
        height: 480
      })
    );
    // No PAR → display dimensions equal pixel dimensions.
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 640,
        height: 480
      })
    );
    // 1/30 s = 33_333_333 ns.
    assert_eq!(v.default_duration_ns, Some(33_333_333));
    assert_eq!(m.frame_duration_ns, Some(33_333_333));
  }

  #[test]
  fn applies_par_widening() {
    // pixel 720x480, PAR 32/27 (wide anamorphic): width should widen.
    let pkt = build_identification_packet(720, 480, 32, 27, 25, 1);
    let v = sniff(&pkt).unwrap().video.unwrap();
    // 720/480 = 1.5 ; 32/27 ≈ 1.185 → pixel ratio is NOT < par ratio, so the
    // height is reduced instead: display_height = round(480 * 27 / 32) = 405.
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 720,
        height: 480
      })
    );
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 720,
        height: 405
      })
    );
  }

  #[test]
  fn applies_par_heightening() {
    // A narrow pixel ratio relative to PAR widens the width.
    let pkt = build_identification_packet(480, 480, 2, 1, 25, 1);
    let v = sniff(&pkt).unwrap().video.unwrap();
    // 480/480 = 1 < 2/1 → display_width = round(480 * 2 / 1) = 960.
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 960,
        height: 480
      })
    );
  }

  #[test]
  fn frame_duration_none_when_frame_rate_zero() {
    let pkt = build_identification_packet(320, 240, 0, 0, 0, 0);
    let m = sniff(&pkt).unwrap();
    assert!(m.frame_duration_ns.is_none());
    assert!(m.video.unwrap().default_duration_ns.is_none());
  }

  #[test]
  fn rejects_wrong_header_id() {
    let mut pkt = build_identification_packet(640, 480, 0, 0, 30, 1);
    pkt[0] = 0x4e;
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn rejects_wrong_mapping_id() {
    let mut pkt = build_identification_packet(640, 480, 0, 0, 30, 1);
    pkt[1] = 0x00;
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn rejects_short_packet() {
    let pkt = build_identification_packet(640, 480, 0, 0, 30, 1);
    assert!(sniff(&pkt[..HEADER_LEN - 1]).is_none());
  }
}
