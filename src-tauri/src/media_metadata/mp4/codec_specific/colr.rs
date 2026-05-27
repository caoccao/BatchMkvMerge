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

//! `colr` — colour information atom (ISO/IEC 14496-12 §12.1.5).
//!
//! Two flavours:
//! - `nclx` / `nclc` — H.273 indices (primaries, transfer, matrix) plus
//!   optional `full_range_flag`.
//! - `prof` — ICC profile (we just record presence).

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{ColorMetadata, ColorRange, VideoTrackProperties};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  parse_with_cap(src, header, builder, u64::MAX)
}

pub fn parse_with_cap(
  src: &mut FileSource,
  header: &BoxHeader,
  builder: &mut TrackBuilder,
  payload_cap: u64,
) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, payload_cap)?;
  if payload.len() < 4 {
    return Ok(());
  }
  let kind = &payload[0..4];
  let video = builder.video.get_or_insert_with(VideoTrackProperties::default);
  let color = video.color.get_or_insert_with(ColorMetadata::default);
  match kind {
    b"nclx" | b"nclc" => {
      if payload.len() < 4 + 2 + 2 + 2 {
        return Ok(());
      }
      let primaries = u16::from_be_bytes([payload[4], payload[5]]) as u32;
      let transfer = u16::from_be_bytes([payload[6], payload[7]]) as u32;
      let matrix = u16::from_be_bytes([payload[8], payload[9]]) as u32;
      color.primaries = Some(primaries);
      color.transfer_characteristics = Some(transfer);
      color.matrix_coefficients = Some(matrix);
      // `nclx` adds 1 trailing byte with full_range_flag at the top bit.
      if kind == b"nclx" && payload.len() >= 11 {
        let full_range = payload[10] & 0x80 != 0;
        color.range = Some(if full_range {
          ColorRange::Full
        } else {
          ColorRange::Broadcast
        });
      }
    }
    b"prof" => {
      // We don't decode the ICC profile.  Nothing to record beyond the
      // already-tracked sample-entry depth.
    }
    _ => {}
  }
  Ok(())
}

#[cfg(test)]
pub(crate) fn build_nclx_payload(primaries: u16, transfer: u16, matrix: u16, full_range: bool) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(b"nclx");
  p.extend_from_slice(&primaries.to_be_bytes());
  p.extend_from_slice(&transfer.to_be_bytes());
  p.extend_from_slice(&matrix.to_be_bytes());
  p.push(if full_range { 0x80 } else { 0 });
  p
}

#[cfg(test)]
pub(crate) fn build_nclc_payload(primaries: u16, transfer: u16, matrix: u16) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(b"nclc");
  p.extend_from_slice(&primaries.to_be_bytes());
  p.extend_from_slice(&transfer.to_be_bytes());
  p.extend_from_slice(&matrix.to_be_bytes());
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn run(payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(b"colr", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  #[test]
  fn nclx_bt2020_pq_full_range() {
    let payload = build_nclx_payload(9, 16, 9, true);
    let b = run(payload);
    let c = b.video.unwrap().color.unwrap();
    assert_eq!(c.primaries, Some(9));
    assert_eq!(c.transfer_characteristics, Some(16));
    assert_eq!(c.matrix_coefficients, Some(9));
    assert_eq!(c.range, Some(ColorRange::Full));
  }

  #[test]
  fn nclx_broadcast_range() {
    let payload = build_nclx_payload(1, 1, 1, false);
    let b = run(payload);
    let c = b.video.unwrap().color.unwrap();
    assert_eq!(c.range, Some(ColorRange::Broadcast));
  }

  #[test]
  fn nclc_has_no_range() {
    let payload = build_nclc_payload(1, 1, 1);
    let b = run(payload);
    let c = b.video.unwrap().color.unwrap();
    assert_eq!(c.primaries, Some(1));
    assert!(c.range.is_none());
  }

  #[test]
  fn prof_is_a_noop_but_creates_color_struct() {
    let bytes = encode_box(b"colr", b"prof");
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    // We still get_or_insert the video + color structs, but neither field
    // gets populated.
    assert!(b.video.is_some());
  }

  #[test]
  fn prof_payload_larger_than_sixty_four_kib_is_accepted() {
    let mut payload = b"prof".to_vec();
    payload.extend(vec![0x42; 70 * 1024]);
    let b = run(payload);
    assert!(b.video.is_some());
  }

  #[test]
  fn truncated_nclx_is_silently_ignored() {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"nclx");
    payload.extend_from_slice(&[0u8; 2]); // only 2 bytes — too short
    let b = run(payload);
    let c = b.video.unwrap().color.unwrap();
    assert!(c.primaries.is_none());
  }

  #[test]
  fn unknown_kind_is_skipped() {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"ZZZZ");
    let b = run(payload);
    let v = b.video.unwrap();
    let c = v.color.unwrap();
    assert!(c.primaries.is_none());
  }
}
