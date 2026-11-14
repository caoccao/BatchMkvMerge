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

//! VobButton (`.btn`) reader — DVD button information streams.
//!
//! mkvtoolnix's `r_vobbtn.cpp` reads 23 bytes and requires
//!
//! * bytes `[0..8]` == ASCII `"butonDVD"` (case-insensitive),
//! * bytes `[0x10..0x14]` == `00 00 01 BF` (PES private_stream_2 start code),
//! * bytes `[0x14..0x17]` == `03 D4 00`.
//!
//! We mirror that structural check so we don't false-positive on any random
//! file that happens to start with the magic.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

pub const MAGIC: [u8; 8] = *b"butonDVD";
const PES_MARKER: [u8; 4] = [0x00, 0x00, 0x01, 0xBF];
const PES_TAIL: [u8; 3] = [0x03, 0xD4, 0x00];
const PROBE_BYTES: usize = 32;

fn header_matches(buf: &[u8]) -> bool {
  if buf.len() < 0x17 {
    return false;
  }
  buf[..8].eq_ignore_ascii_case(&MAGIC) && buf[0x10..0x14] == PES_MARKER && buf[0x14..0x17] == PES_TAIL
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VobButtonReader;

impl Reader for VobButtonReader {
  fn name(&self) -> &'static str {
    "vobbtn"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = [0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read >= 0x17 && header_matches(&buf[..read]))
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = [0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    if read < 0x17 || !header_matches(&buf[..read]) {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::VobButton;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let dimensions = Dimensions2D {
      width: u16::from_be_bytes([buf[8], buf[9]]) as u32,
      height: u16::from_be_bytes([buf[10], buf[11]]) as u32,
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Buttons,
      codec: CodecInfo {
        id: "B_VOBBTN".to_string(),
        name: Some("VobButton".to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        video: Some(VideoTrackProperties {
          pixel_dimensions: Some(dimensions),
          display_dimensions: Some(dimensions),
          ..VideoTrackProperties::default()
        }),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

#[cfg(test)]
pub(crate) fn build_header() -> Vec<u8> {
  let mut blob = MAGIC.to_vec();
  blob.extend_from_slice(&720u16.to_be_bytes());
  blob.extend_from_slice(&480u16.to_be_bytes());
  blob.extend_from_slice(&[0u8; 4]);
  blob.extend_from_slice(&PES_MARKER);
  blob.extend_from_slice(&PES_TAIL);
  // bytes 0x17.. — irrelevant trailer for the structural probe.
  blob.extend_from_slice(&[0u8; 16]);
  blob
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn probe_accepts_valid_header() {
    let blob = build_header();
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(VobButtonReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_mixed_case_magic() {
    let mut blob = b"BuTonDvD".to_vec();
    blob.extend_from_slice(&[0u8; 8]);
    blob.extend_from_slice(&PES_MARKER);
    blob.extend_from_slice(&PES_TAIL);
    blob.extend_from_slice(&[0u8; 16]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(VobButtonReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_magic_only() {
    let mut blob = MAGIC.to_vec();
    blob.extend_from_slice(&[0u8; 32]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!VobButtonReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_other_magic() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"NOTMAGIC".to_vec()));
    assert!(!VobButtonReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"buton".to_vec()));
    assert!(!VobButtonReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_button_track() {
    let blob = build_header();
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.btn", 0);
    VobButtonReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::VobButton);
    assert_eq!(out.tracks[0].track_type, TrackType::Buttons);
    assert_eq!(out.tracks[0].codec.id, "B_VOBBTN");
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 720,
        height: 480
      })
    );
  }
}
