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

//! `stbl` (sample table) — wraps `stsd` (sample descriptions) and `stts`
//! (decoding time-to-sample).  Other sub-boxes (`stsc`, `stsz`, `stco`,
//! `co64`, `stss`, `ctts`) are intentionally skipped — identification mode
//! does not need them.

pub mod stsd;
pub mod stts;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::trak::TrackBuilder;

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    atom::walk_children(src, parent, "mp4::stbl", deadline, |src, child| match &child.kind.0 {
        b"stsd" => {
            stsd::parse(src, child, deadline, builder)?;
            Ok(ChildAction::Consumed)
        }
        b"stts" => {
            let s = stts::parse(src, child)?;
            builder.stts_first_sample_count = Some(s.first_sample_count);
            builder.stts_first_sample_delta = Some(s.first_sample_delta);
            Ok(ChildAction::Consumed)
        }
        _ => Ok(ChildAction::Skip),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mp4::atom::encode_box;
    use crate::media_metadata::mp4::moov::stbl::stsd::{
        build_audio_sample_entry_v0, build_stsd_payload,
    };
    use crate::media_metadata::mp4::moov::stbl::stts::build_stts_payload;
    use std::io::Cursor;

    #[test]
    fn parses_stsd_and_stts_into_builder() {
        let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48_000, &[]);
        let stsd = encode_box(b"stsd", &build_stsd_payload(&[entry]));
        let stts = encode_box(b"stts", &build_stts_payload(&[(60, 1000)]));
        let mut payload = stsd;
        payload.extend(stts);
        let stbl = encode_box(b"stbl", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        b.handler_type = Some(*b"soun");
        let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
        parse(&mut s, &parent, &deadline, &mut b).unwrap();
        assert_eq!(b.codec_id_str.as_deref(), Some("mp4a"));
        assert_eq!(b.stts_first_sample_count, Some(60));
        assert_eq!(b.stts_first_sample_delta, Some(1000));
    }

    #[test]
    fn unknown_stbl_child_skipped_silently() {
        let bogus = encode_box(b"junk", &[0u8; 4]);
        let stbl = encode_box(b"stbl", &bogus);
        let mut s = FileSource::from_reader_for_test(Cursor::new(stbl));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let deadline = crate::media_metadata::deadline::Deadline::new(60_000);
        parse(&mut s, &parent, &deadline, &mut b).unwrap();
        assert!(b.stts_first_sample_count.is_none());
    }
}
