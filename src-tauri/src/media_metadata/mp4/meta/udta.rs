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

//! `udta` (user data) walker.  Two common shapes:
//!
//! 1. `udta` → `meta` → `hdlr`("mdir") + `ilst` (iTunes path).
//! 2. `udta` → `meta` → `keys` + `ilst` (QuickTime keyed path; we recognise
//!    the meta box but only the iTunes shape is decoded for now).
//!
//! We walk into either `meta` directly or through `udta`.  The actual tag
//! extraction happens in [`super::ilst`].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::ilst;

pub fn parse_udta(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    out: &mut MediaMetadata,
) -> Result<(), ParseError> {
    atom::walk_children(src, parent, "mp4::udta", deadline, |src, child| {
        if child.kind.eq_ascii(b"meta") {
            parse_meta(src, child, deadline, out)?;
            Ok(ChildAction::Consumed)
        } else {
            Ok(ChildAction::Skip)
        }
    })
}

pub fn parse_meta(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    out: &mut MediaMetadata,
) -> Result<(), ParseError> {
    // ISO `meta` is a FullBox (4-byte version+flags prefix); QuickTime `meta`
    // is a plain container.  Sniff which one we have by peeking the first
    // 8 bytes — if they look like a child box header, we treat it as QT.
    let payload_start = parent.payload_start();
    src.seek_to(payload_start)?;
    let peeked = match atom::peek_box_header(src) {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };
    let is_iso_full_box = !peeked.kind.is_human_readable();
    if is_iso_full_box {
        // Skip 4-byte FullBox header.
        src.seek_to(payload_start + 4)?;
    }
    let synthetic = BoxHeader {
        start: parent.start,
        kind: parent.kind,
        header_len: (src.position() - parent.start) as u8,
        total_size: parent.total_size,
    };
    atom::walk_children(src, &synthetic, "mp4::meta", deadline, |src, child| {
        if child.kind.eq_ascii(b"ilst") {
            ilst::parse(src, child, deadline, out)?;
            Ok(ChildAction::Consumed)
        } else {
            Ok(ChildAction::Skip)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mp4::atom::encode_box;
    use crate::media_metadata::mp4::meta::ilst::{build_data_box, build_ilst_tag};
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    #[test]
    fn parses_itunes_path_through_udta() {
        let tag = build_ilst_tag(b"\xA9nam", build_data_box(1, b"Track Name"));
        let ilst = encode_box(b"ilst", &tag);
        let mut meta_payload = vec![0u8; 4]; // ISO FullBox header
        meta_payload.extend(ilst);
        let meta = encode_box(b"meta", &meta_payload);
        let udta = encode_box(b"udta", &meta);
        let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut m = MediaMetadata::new("clip.mp4", 0);
        parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
        assert_eq!(m.container.properties.title.as_deref(), Some("Track Name"));
    }

    #[test]
    fn parses_quicktime_meta_without_fullbox_header() {
        // QuickTime meta has no FullBox prefix — first child is the hdlr or ilst.
        let tag = build_ilst_tag(b"\xA9nam", build_data_box(1, b"QT Name"));
        let ilst = encode_box(b"ilst", &tag);
        let meta = encode_box(b"meta", &ilst);
        let mut s = FileSource::from_reader_for_test(Cursor::new(meta));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut m = MediaMetadata::new("clip.mp4", 0);
        parse_meta(&mut s, &h, &dl(), &mut m).unwrap();
        assert_eq!(m.container.properties.title.as_deref(), Some("QT Name"));
    }

    #[test]
    fn unknown_meta_child_is_skipped() {
        let bogus = encode_box(b"junk", &[0u8; 8]);
        let mut meta_payload = vec![0u8; 4];
        meta_payload.extend(bogus);
        let meta = encode_box(b"meta", &meta_payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(meta));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut m = MediaMetadata::new("clip.mp4", 0);
        parse_meta(&mut s, &h, &dl(), &mut m).unwrap();
        assert!(m.container.properties.title.is_none());
    }

    #[test]
    fn udta_with_no_meta_is_a_noop() {
        let other = encode_box(b"xxxx", &[]);
        let udta = encode_box(b"udta", &other);
        let mut s = FileSource::from_reader_for_test(Cursor::new(udta));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut m = MediaMetadata::new("clip.mp4", 0);
        parse_udta(&mut s, &h, &dl(), &mut m).unwrap();
        assert!(m.container.properties.title.is_none());
    }
}
