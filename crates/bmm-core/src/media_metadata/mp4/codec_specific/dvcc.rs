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

//! `dvcC` / `dvvC` — Dolby Vision configuration box.
//!
//! PARSER-179: mkvtoolnix does NOT treat the `dvcC` / `dvvC` payload as the
//! primary decoder configuration record.  It stores the raw bytes as a
//! block-addition mapping via `add_data_as_block_addition`
//! (`r_qtmp4.cpp:3377-3378`): `block_addition_mapping_t{ id_type: fourcc,
//! id_extra_data: bytes }`.  We mirror that here — the raw payload is recorded
//! on the track builder keyed by the box FOURCC and the `VideoCodecConfig`
//! is left untouched.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

const MIN_PAYLOAD: usize = 4;

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
  if payload.len() < MIN_PAYLOAD {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("dvcC payload {} bytes too small", payload.len()),
    });
  }
  // PARSER-179: store as a block addition keyed by the box FOURCC, not as
  // the codec configuration record.
  let fourcc: String = header.kind.0.iter().map(|b| *b as char).collect();
  builder.block_additions.push((fourcc, payload));
  Ok(())
}

#[cfg(test)]
pub(crate) fn build_dvcc_payload(profile: u8, level: u8, bl: bool, el: bool, rpu: bool) -> Vec<u8> {
  let mut p = vec![0u8; 24];
  p[0] = 1; // version major
  p[1] = 0; // version minor
  p[2] = ((profile & 0x7F) << 1) | ((level >> 5) & 0x01);
  p[3] = ((level & 0x1F) << 3) | ((rpu as u8) << 2) | ((el as u8) << 1) | (bl as u8);
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn run(fourcc: &[u8; 4], payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(fourcc, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  // PARSER-179: r_qtmp4.cpp:3377-3378 records dvcC/dvvC via
  // add_data_as_block_addition rather than as the decoder config.
  #[test]
  fn dvcc_recorded_as_block_addition_not_codec_config() {
    let payload = build_dvcc_payload(8, 6, true, true, true);
    let b = run(b"dvcC", payload.clone());
    // No VideoCodecConfig is fabricated from the DV box.
    assert!(b.video_codec_config.is_none());
    assert_eq!(b.block_additions.len(), 1);
    assert_eq!(b.block_additions[0].0, "dvcC");
    assert_eq!(b.block_additions[0].1, payload);
  }

  #[test]
  fn dvvc_keyed_by_its_own_fourcc() {
    let payload = build_dvcc_payload(5, 3, true, false, false);
    let b = run(b"dvvC", payload.clone());
    assert_eq!(b.block_additions[0].0, "dvvC");
    assert_eq!(b.block_additions[0].1, payload);
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"dvcC", &[0u8; 2]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let err = parse(&mut s, &h, &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
