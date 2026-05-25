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

//! `pasp` — pixel aspect ratio.  Two unsigned 32-bit big-endian integers
//! (numerator + denominator).  Feeds `VideoCodecConfig.sample_aspect_ratio`.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{SampleAspectRatio, VideoCodecConfig};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, 16)?;
  if payload.len() < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("pasp payload {} bytes too small", payload.len()),
    });
  }
  let num = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
  let den = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
  let cfg = builder.video_codec_config.get_or_insert_with(VideoCodecConfig::default);
  cfg.sample_aspect_ratio = Some(SampleAspectRatio { num, den });
  Ok(())
}

#[cfg(test)]
pub(crate) fn build_pasp_payload(num: u32, den: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(8);
  p.extend_from_slice(&num.to_be_bytes());
  p.extend_from_slice(&den.to_be_bytes());
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn run(payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(b"pasp", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  #[test]
  fn extracts_sample_aspect_ratio() {
    let b = run(build_pasp_payload(40, 33));
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.sample_aspect_ratio, Some(SampleAspectRatio { num: 40, den: 33 }));
  }

  #[test]
  fn rejects_truncated() {
    let bytes = encode_box(b"pasp", &[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    let err = parse(&mut s, &h, &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
