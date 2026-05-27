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

//! `mdia` (media) box — wraps `mdhd`, `hdlr`, `minf`.  `minf` in turn wraps
//! `stbl` which holds the sample tables we extract via [`super::stbl`].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::hdlr;
use super::mdhd;
use super::trak::TrackBuilder;

pub fn parse(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::mdia", deadline, |src, child| match &child.kind.0 {
    b"mdhd" => {
      // PARSER-146: an unsupported / zero-timescale mdhd marks the track
      // invalid (dropped later) instead of aborting the whole file parse —
      // mkvtoolnix skips just the offending track.
      match mdhd::parse(src, child) {
        Ok(m) => {
          builder.media_timescale = Some(m.timescale);
          builder.media_duration_units = Some(m.duration);
          builder.language_iso_639_2 = m.language_iso_639_2;
        }
        Err(ParseError::Malformed { .. }) => {
          builder.media_invalid = true;
        }
        Err(e) => return Err(e),
      }
      Ok(ChildAction::Consumed)
    }
    b"hdlr" => {
      let h = hdlr::parse(src, child)?;
      builder.handler_type = Some(h.handler_type);
      if !h.name.is_empty() {
        builder.handler_name = Some(h.name);
      }
      Ok(ChildAction::Consumed)
    }
    b"minf" => {
      parse_minf(src, child, deadline, builder)?;
      Ok(ChildAction::Consumed)
    }
    _ => Ok(ChildAction::Skip),
  })
}

fn parse_minf(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::minf", deadline, |src, child| {
    if child.kind.eq_ascii(b"stbl") {
      super::stbl::parse(src, child, deadline, builder)?;
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use crate::media_metadata::mp4::moov::hdlr::build_hdlr_payload;
  use crate::media_metadata::mp4::moov::mdhd::build_mdhd_payload_v0;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn parses_mdhd_and_hdlr_into_builder() {
    let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(48000, 1024, "fra"));
    let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"soun", "SoundHandler"));
    let mut payload = mdhd;
    payload.extend(hdlr);
    let mdia = encode_box(b"mdia", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(mdia));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &dl(), &mut b).unwrap();
    assert_eq!(b.media_timescale, Some(48000));
    assert_eq!(b.language_iso_639_2.as_deref(), Some("fra"));
    assert_eq!(b.handler_type, Some(*b"soun"));
    assert_eq!(b.handler_name.as_deref(), Some("SoundHandler"));
  }

  #[test]
  fn unknown_minf_child_is_skipped() {
    let bogus = encode_box(b"xxxx", &[0u8; 4]);
    let minf = encode_box(b"minf", &bogus);
    let mdia = encode_box(b"mdia", &minf);
    let mut s = FileSource::from_reader_for_test(Cursor::new(mdia));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &dl(), &mut b).unwrap();
    // nothing should have populated
    assert!(b.codec_id_str.is_none());
  }
}
