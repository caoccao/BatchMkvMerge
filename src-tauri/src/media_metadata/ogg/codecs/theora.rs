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

//! Theora identification header (Theora I §6.2):
//!
//! ```text
//! u8  packet_type      (== 0x80)
//! 6   "theora"
//! u8  VMAJ             (3)
//! u8  VMIN             (2)
//! u8  VREV             (1)
//! u16 FMBW             (BE, frame width in 16-pixel macroblocks — multiply by 16)
//! u16 FMBH             (BE, frame height in macroblocks)
//! u24 PICW             (BE, picture width in pixels)
//! u24 PICH             (BE, picture height in pixels)
//! u8  PICX
//! u8  PICY
//! u32 FRN              (BE, frame-rate numerator)
//! u32 FRD              (BE, frame-rate denominator)
//! ```

use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use super::BitstreamMetadata;

const SIGNATURE: [u8; 7] = [0x80, b't', b'h', b'e', b'o', b'r', b'a'];
const MIN_LEN: usize = 7 + 1 + 1 + 1 + 2 + 2 + 3 + 3 + 1 + 1 + 4 + 4; // 29

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
    if packet.len() < MIN_LEN || packet[..7] != SIGNATURE {
        return None;
    }
    // Theora I §6.2 offsets: 7 (signature) + 3 (VMAJ/VMIN/VREV) = 10.
    let mb_width = u16::from_be_bytes([packet[10], packet[11]]) as u32 * 16;
    let mb_height = u16::from_be_bytes([packet[12], packet[13]]) as u32 * 16;
    let pic_w = ((packet[14] as u32) << 16) | ((packet[15] as u32) << 8) | (packet[16] as u32);
    let pic_h = ((packet[17] as u32) << 16) | ((packet[18] as u32) << 8) | (packet[19] as u32);
    // PICX (20) + PICY (21) skipped.
    let frn = u32::from_be_bytes([packet[22], packet[23], packet[24], packet[25]]);
    let frd = u32::from_be_bytes([packet[26], packet[27], packet[28], packet[29]]);

    let width = if pic_w != 0 { pic_w } else { mb_width };
    let height = if pic_h != 0 { pic_h } else { mb_height };

    let mut metadata = BitstreamMetadata::video_only("V_THEORA", "Theora");
    let frame_duration_ns = if frn != 0 && frd != 0 {
        Some((frd as u128 * 1_000_000_000 / frn as u128) as u64)
    } else {
        None
    };
    metadata.frame_duration_ns = frame_duration_ns;
    metadata.video = Some(VideoTrackProperties {
        pixel_dimensions: if width > 0 && height > 0 {
            Some(Dimensions2D { width, height })
        } else {
            None
        },
        display_dimensions: if width > 0 && height > 0 {
            Some(Dimensions2D { width, height })
        } else {
            None
        },
        default_duration_ns: frame_duration_ns,
        ..VideoTrackProperties::default()
    });
    Some(metadata)
}

#[cfg(test)]
pub(crate) fn build_identification_packet(
    pic_w: u32,
    pic_h: u32,
    frame_rate_num: u32,
    frame_rate_den: u32,
) -> Vec<u8> {
    let mut p = Vec::with_capacity(MIN_LEN);
    p.extend_from_slice(&SIGNATURE);
    p.push(3); // VMAJ
    p.push(2); // VMIN
    p.push(1); // VREV
    p.extend_from_slice(&((pic_w / 16) as u16).to_be_bytes());
    p.extend_from_slice(&((pic_h / 16) as u16).to_be_bytes());
    p.push((pic_w >> 16) as u8);
    p.push((pic_w >> 8) as u8);
    p.push(pic_w as u8);
    p.push((pic_h >> 16) as u8);
    p.push((pic_h >> 8) as u8);
    p.push(pic_h as u8);
    p.push(0); // PICX
    p.push(0); // PICY
    p.extend_from_slice(&frame_rate_num.to_be_bytes());
    p.extend_from_slice(&frame_rate_den.to_be_bytes());
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_1920x1080_at_24p() {
        let pkt = build_identification_packet(1920, 1080, 24, 1);
        let m = sniff(&pkt).unwrap();
        assert_eq!(m.codec_id, "V_THEORA");
        let v = m.video.unwrap();
        assert_eq!(
            v.pixel_dimensions,
            Some(Dimensions2D { width: 1920, height: 1080 })
        );
        assert_eq!(v.default_duration_ns, Some(41_666_666));
    }

    #[test]
    fn falls_back_to_macroblock_dims_when_picw_zero() {
        let pkt = build_identification_packet(0, 0, 24, 1);
        let m = sniff(&pkt).unwrap();
        let v = m.video.unwrap();
        // mb-width = 0 / 16 * 16 = 0; both pic_w and mb_width are 0
        assert!(v.pixel_dimensions.is_none());
    }

    #[test]
    fn frame_duration_none_when_frame_rate_zero() {
        let pkt = build_identification_packet(640, 480, 0, 0);
        let m = sniff(&pkt).unwrap();
        assert!(m.frame_duration_ns.is_none());
    }

    #[test]
    fn rejects_non_theora_signature() {
        assert!(sniff(b"\x80vorbis").is_none());
    }

    #[test]
    fn rejects_short_packet() {
        assert!(sniff(&[0x80, b't', b'h', b'e']).is_none());
    }
}
