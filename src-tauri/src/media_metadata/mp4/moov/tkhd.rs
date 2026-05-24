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

//! `tkhd` (track header) box.  Per ISO/IEC 14496-12 §8.3.2.
//!
//! We extract:
//! - `track_id` — feeds `CommonTrackProperties.number`.
//! - `width` / `height` — 16.16 fixed-point, only meaningful for video tracks
//!   (display dimensions; the encoded raster lives on the sample description).

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone, Copy)]
pub struct TrackHeader {
    pub version: u8,
    pub track_id: u32,
    /// 16.16 fixed-point display width.  Always 0 for non-video tracks.
    pub width_fixed: u32,
    /// 16.16 fixed-point display height.
    pub height_fixed: u32,
    pub enabled: bool,
}

const FLAG_TRACK_ENABLED: u32 = 0x000001;

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<TrackHeader, ParseError> {
    let payload = header.payload_size().unwrap_or(0);
    if payload < 84 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: header.start,
            reason: format!("tkhd payload {payload} bytes is too small"),
        });
    }
    let version = src.read_u8()?;
    let flags_bytes = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let flags = ((flags_bytes[0] as u32) << 16)
        | ((flags_bytes[1] as u32) << 8)
        | flags_bytes[2] as u32;
    let track_id = match version {
        0 => {
            src.skip(4 + 4)?; // creation + modification (4+4)
            let id = src.read_u32_be()?;
            src.skip(4)?; // reserved
            src.skip(4)?; // duration
            id
        }
        _ => {
            src.skip(8 + 8)?; // creation + modification (8+8)
            let id = src.read_u32_be()?;
            src.skip(4)?; // reserved
            src.skip(8)?; // duration
            id
        }
    };
    // 2x4 reserved + 2 layer + 2 alt_group + 2 volume + 2 reserved + 36 matrix = 52 bytes
    src.skip(8 + 2 + 2 + 2 + 2 + 36)?;
    let width_fixed = src.read_u32_be()?;
    let height_fixed = src.read_u32_be()?;
    Ok(TrackHeader {
        version,
        track_id,
        width_fixed,
        height_fixed,
        enabled: flags & FLAG_TRACK_ENABLED != 0,
    })
}

/// Convert a 16.16 fixed-point value to a `u32` pixel count by dropping the
/// fractional part.  Matches what mkvmerge's identification output reports.
pub fn fixed_to_pixels(fixed: u32) -> u32 {
    fixed >> 16
}

#[cfg(test)]
pub(crate) fn build_tkhd_payload_v0(track_id: u32, width_px: u16, height_px: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(84);
    p.push(0); // version
    p.extend_from_slice(&[0u8, 0u8, 0x01u8]); // flags = track_enabled
    p.extend_from_slice(&0u32.to_be_bytes()); // creation
    p.extend_from_slice(&0u32.to_be_bytes()); // modification
    p.extend_from_slice(&track_id.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // reserved
    p.extend_from_slice(&0u32.to_be_bytes()); // duration
    p.extend_from_slice(&[0u8; 8]); // 2x4 reserved
    p.extend_from_slice(&[0u8; 2]); // layer
    p.extend_from_slice(&[0u8; 2]); // alt_group
    p.extend_from_slice(&[0u8; 2]); // volume
    p.extend_from_slice(&[0u8; 2]); // reserved
    p.extend_from_slice(&[0u8; 36]); // matrix
    let width = (width_px as u32) << 16;
    let height = (height_px as u32) << 16;
    p.extend_from_slice(&width.to_be_bytes());
    p.extend_from_slice(&height.to_be_bytes());
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mp4::atom::{self, encode_box};
    use std::io::Cursor;

    fn read(bytes: Vec<u8>) -> (BoxHeader, FileSource) {
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        (h, s)
    }

    #[test]
    fn parses_v0_tkhd() {
        let payload = build_tkhd_payload_v0(2, 1920, 1080);
        let bytes = encode_box(b"tkhd", &payload);
        let (h, mut s) = read(bytes);
        let t = parse(&mut s, &h).unwrap();
        assert_eq!(t.track_id, 2);
        assert_eq!(fixed_to_pixels(t.width_fixed), 1920);
        assert_eq!(fixed_to_pixels(t.height_fixed), 1080);
        assert!(t.enabled);
    }

    #[test]
    fn parses_v1_tkhd() {
        let mut p = Vec::new();
        p.push(1); // version
        p.extend_from_slice(&[0u8, 0u8, 0x01u8]); // flags
        p.extend_from_slice(&[0u8; 8]); // creation
        p.extend_from_slice(&[0u8; 8]); // modification
        p.extend_from_slice(&7u32.to_be_bytes()); // track_id
        p.extend_from_slice(&[0u8; 4]); // reserved
        p.extend_from_slice(&[0u8; 8]); // duration
        p.extend_from_slice(&[0u8; 8 + 2 + 2 + 2 + 2 + 36]);
        p.extend_from_slice(&((1280u32) << 16).to_be_bytes());
        p.extend_from_slice(&((720u32) << 16).to_be_bytes());
        let bytes = encode_box(b"tkhd", &p);
        let (h, mut s) = read(bytes);
        let t = parse(&mut s, &h).unwrap();
        assert_eq!(t.version, 1);
        assert_eq!(t.track_id, 7);
        assert_eq!(fixed_to_pixels(t.width_fixed), 1280);
        assert_eq!(fixed_to_pixels(t.height_fixed), 720);
    }

    #[test]
    fn flag_enabled_decoded() {
        let mut p = build_tkhd_payload_v0(1, 0, 0);
        // Zero out the flag byte (offset 3)
        p[3] = 0;
        let bytes = encode_box(b"tkhd", &p);
        let (h, mut s) = read(bytes);
        let t = parse(&mut s, &h).unwrap();
        assert!(!t.enabled);
    }

    #[test]
    fn rejects_truncated_payload() {
        let bytes = encode_box(b"tkhd", &[0u8; 16]);
        let (h, mut s) = read(bytes);
        let err = parse(&mut s, &h).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn fixed_to_pixels_drops_fractional() {
        assert_eq!(fixed_to_pixels(0x07800000), 1920);
        assert_eq!(fixed_to_pixels(0x00008000), 0); // 0.5
    }
}
