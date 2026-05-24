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

//! `hdlr` (handler reference) box.  Per ISO/IEC 14496-12 §8.4.3.
//!
//! Layout:
//!
//! ```text
//! version (1) + flags (3) + predefined (4) + handler_type (4)
//!   + reserved (12) + name (zero-terminated UTF-8)
//! ```
//!
//! The `handler_type` is the discriminator we map to our `TrackType`:
//! - `vide` → Video
//! - `soun` → Audio
//! - `subt` / `sbtl` → Subtitles
//! - `text` → Subtitles (QuickTime tx3g)
//! - `subp` → Subtitles
//! - `meta` / `mdir` → metadata handler (skipped, not a track)
//! - anything else → Unknown

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track::TrackType;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone)]
pub struct Handler {
    pub handler_type: [u8; 4],
    pub name: String,
}

impl Handler {
    /// Classify the handler type into the protocol's [`TrackType`].
    pub fn classify(&self) -> TrackType {
        match &self.handler_type {
            b"vide" => TrackType::Video,
            b"soun" => TrackType::Audio,
            b"subt" | b"sbtl" | b"text" | b"subp" => TrackType::Subtitles,
            _ => TrackType::Unknown,
        }
    }

    /// `true` for non-track handlers (`meta`, `mdir`, …) — the trak walker
    /// will silently drop these.
    pub fn is_metadata_handler(&self) -> bool {
        matches!(&self.handler_type, b"meta" | b"mdir")
    }
}

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<Handler, ParseError> {
    let payload = header.payload_size().unwrap_or(0);
    if payload < 24 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: header.start,
            reason: format!("hdlr payload {payload} bytes is too small"),
        });
    }
    // 1B version + 3B flags + 4B pre_defined
    src.skip(1 + 3 + 4)?;
    let handler_type = src.read_array::<4>()?;
    src.skip(12)?; // reserved[3]
    let name_len = payload.saturating_sub(24);
    let raw = if name_len == 0 {
        Vec::new()
    } else {
        let mut buf = vec![0u8; name_len as usize];
        src.read_exact(&mut buf)?;
        // Strip trailing NUL bytes.
        while let Some(0) = buf.last() {
            buf.pop();
        }
        buf
    };
    let name = String::from_utf8_lossy(&raw).into_owned();
    Ok(Handler { handler_type, name })
}

#[cfg(test)]
pub(crate) fn build_hdlr_payload(handler_type: &[u8; 4], name: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(24 + name.len() + 1);
    p.push(0); // version
    p.extend_from_slice(&[0u8; 3]); // flags
    p.extend_from_slice(&[0u8; 4]); // pre_defined
    p.extend_from_slice(handler_type);
    p.extend_from_slice(&[0u8; 12]); // reserved
    p.extend_from_slice(name.as_bytes());
    p.push(0); // NUL terminator
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

    fn parsed(handler_type: &[u8; 4], name: &str) -> Handler {
        let bytes = encode_box(b"hdlr", &build_hdlr_payload(handler_type, name));
        let (h, mut s) = read(bytes);
        parse(&mut s, &h).unwrap()
    }

    #[test]
    fn vide_classified_as_video() {
        let h = parsed(b"vide", "VideoHandler");
        assert_eq!(h.classify(), TrackType::Video);
        assert_eq!(h.name, "VideoHandler");
    }

    #[test]
    fn soun_classified_as_audio() {
        assert_eq!(parsed(b"soun", "").classify(), TrackType::Audio);
    }

    #[test]
    fn subt_and_sbtl_classified_as_subtitles() {
        assert_eq!(parsed(b"subt", "").classify(), TrackType::Subtitles);
        assert_eq!(parsed(b"sbtl", "").classify(), TrackType::Subtitles);
        assert_eq!(parsed(b"text", "").classify(), TrackType::Subtitles);
    }

    #[test]
    fn meta_handler_recognised() {
        let h = parsed(b"meta", "");
        assert_eq!(h.classify(), TrackType::Unknown);
        assert!(h.is_metadata_handler());
    }

    #[test]
    fn unknown_handler_is_unknown() {
        assert_eq!(parsed(b"XXXX", "").classify(), TrackType::Unknown);
    }

    #[test]
    fn name_round_trips() {
        let h = parsed(b"vide", "MyHandler");
        assert_eq!(h.name, "MyHandler");
    }

    #[test]
    fn empty_name_is_empty_string() {
        let h = parsed(b"vide", "");
        assert_eq!(h.name, "");
    }

    #[test]
    fn rejects_truncated_payload() {
        let bytes = encode_box(b"hdlr", &[0u8; 8]);
        let (h, mut s) = read(bytes);
        let err = parse(&mut s, &h).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }
}
