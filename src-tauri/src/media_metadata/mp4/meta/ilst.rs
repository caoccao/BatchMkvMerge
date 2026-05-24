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

//! `ilst` (iTunes metadata list) walker.  Each child is a 4-byte tag box
//! wrapping one `data` atom:
//!
//! ```text
//! data {
//!   u32 size
//!   "data"
//!   u32 type_code     // 1 = UTF-8, 2 = UTF-16, 21 = signed int, ...
//!   u32 locale
//!   u8  value[..]
//! }
//! ```
//!
//! We map a known subset of tags onto either container fields
//! (`title`, `muxing_app`, `date_utc`) or the global `tags.global` bundle
//! when they don't correspond to anything else.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerProperties;
use crate::media_metadata::model::tag::TagEntry;
use crate::media_metadata::model::MediaMetadata;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

const TYPE_UTF8: u32 = 1;
const TYPE_UTF16: u32 = 2;
const TYPE_JPEG: u32 = 13;
const TYPE_PNG: u32 = 14;
const TYPE_SIGNED_INT: u32 = 21;

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    out: &mut MediaMetadata,
) -> Result<(), ParseError> {
    atom::walk_children(src, parent, "mp4::ilst", deadline, |src, child| {
        let key = child.kind.0;
        let value = match read_data_value(src, child, deadline) {
            Ok(v) => v,
            Err(_) => return Ok(ChildAction::Skip),
        };
        if let Some(value) = value {
            route(&key, value, &mut out.container.properties, &mut out.tags.global);
        }
        Ok(ChildAction::Consumed)
    })
}

#[derive(Debug, Clone)]
enum DataValue {
    Text(String),
    Int(i64),
    Image, // we don't decode image payloads
}

fn read_data_value(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
) -> Result<Option<DataValue>, ParseError> {
    let mut result: Option<DataValue> = None;
    atom::walk_children(src, parent, "mp4::ilst::tag", deadline, |src, child| {
        if !child.kind.eq_ascii(b"data") {
            return Ok(ChildAction::Skip);
        }
        let payload = atom::read_payload(src, child, 16 * 1024)?;
        if payload.len() < 8 {
            return Ok(ChildAction::Consumed);
        }
        let type_code = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
            & 0x00FF_FFFF;
        // skip 4 bytes of locale (payload[4..8])
        let body = &payload[8..];
        result = match type_code {
            TYPE_UTF8 => Some(DataValue::Text(
                String::from_utf8_lossy(body).into_owned(),
            )),
            TYPE_UTF16 => Some(DataValue::Text(decode_utf16_be(body))),
            TYPE_SIGNED_INT => Some(DataValue::Int(decode_signed_int(body))),
            TYPE_JPEG | TYPE_PNG => Some(DataValue::Image),
            _ => None,
        };
        Ok(ChildAction::Consumed)
    })?;
    Ok(result)
}

fn decode_utf16_be(bytes: &[u8]) -> String {
    let codepoints: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|w| u16::from_be_bytes([w[0], w[1]]))
        .collect();
    String::from_utf16_lossy(&codepoints)
}

fn decode_signed_int(bytes: &[u8]) -> i64 {
    let mut value: i64 = 0;
    if bytes.is_empty() {
        return 0;
    }
    let sign_extend = bytes[0] & 0x80 != 0;
    for &b in bytes {
        value = (value << 8) | b as i64;
    }
    let bits = bytes.len() as u32 * 8;
    if sign_extend && bits < 64 {
        let mask = !0i64 << bits;
        value |= mask;
    }
    value
}

fn route(
    key: &[u8; 4],
    value: DataValue,
    container: &mut ContainerProperties,
    global_tags: &mut Vec<TagEntry>,
) {
    let text = match value {
        DataValue::Text(t) => t,
        DataValue::Int(i) => i.to_string(),
        DataValue::Image => return, // identification doesn't expose artwork bytes
    };
    match key {
        b"\xA9nam" => container.title = Some(text),
        b"\xA9too" => container.muxing_app = Some(text),
        b"\xA9day" => container.date_utc = Some(text),
        _ => {
            global_tags.push(TagEntry {
                name: key_display(key),
                value: text,
                language: None,
            });
        }
    }
}

fn key_display(key: &[u8; 4]) -> String {
    // Replace the ©  sentinel (0xA9) with the ASCII © glyph for readability.
    key.iter()
        .map(|b| if *b == 0xA9 { '©' } else { *b as char })
        .collect()
}

#[cfg(test)]
pub(crate) fn build_data_box(type_code: u32, value: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&type_code.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // locale
    p.extend_from_slice(value);
    crate::media_metadata::mp4::atom::encode_box(b"data", &p)
}

#[cfg(test)]
pub(crate) fn build_ilst_tag(key: &[u8; 4], data_box: Vec<u8>) -> Vec<u8> {
    crate::media_metadata::mp4::atom::encode_box(key, &data_box)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mp4::atom::encode_box;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn run(payload: Vec<u8>) -> MediaMetadata {
        let bytes = encode_box(b"ilst", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = atom::read_box_header(&mut s).unwrap();
        let mut m = MediaMetadata::new("clip.mp4", 0);
        parse(&mut s, &h, &dl(), &mut m).unwrap();
        m
    }

    #[test]
    fn title_extracted_into_container() {
        let payload = build_ilst_tag(
            b"\xA9nam",
            build_data_box(TYPE_UTF8, b"My Movie"),
        );
        let m = run(payload);
        assert_eq!(m.container.properties.title.as_deref(), Some("My Movie"));
    }

    #[test]
    fn encoder_into_muxing_app() {
        let payload = build_ilst_tag(
            b"\xA9too",
            build_data_box(TYPE_UTF8, b"HandBrake 1.6.1"),
        );
        let m = run(payload);
        assert_eq!(
            m.container.properties.muxing_app.as_deref(),
            Some("HandBrake 1.6.1"),
        );
    }

    #[test]
    fn date_into_date_utc() {
        let payload = build_ilst_tag(
            b"\xA9day",
            build_data_box(TYPE_UTF8, b"2024-03-14"),
        );
        let m = run(payload);
        assert_eq!(m.container.properties.date_utc.as_deref(), Some("2024-03-14"));
    }

    #[test]
    fn artist_routes_to_global_tags() {
        let payload = build_ilst_tag(
            b"\xA9ART",
            build_data_box(TYPE_UTF8, b"Hans Zimmer"),
        );
        let m = run(payload);
        assert_eq!(m.tags.global.len(), 1);
        assert_eq!(m.tags.global[0].name, "©ART");
        assert_eq!(m.tags.global[0].value, "Hans Zimmer");
    }

    #[test]
    fn utf16_value_decoded() {
        // UTF-16 BE "日本"
        let payload = build_ilst_tag(
            b"\xA9nam",
            build_data_box(TYPE_UTF16, &[0x65, 0xE5, 0x67, 0x2C]),
        );
        let m = run(payload);
        assert_eq!(m.container.properties.title.as_deref(), Some("日本"));
    }

    #[test]
    fn signed_int_rendered_as_string() {
        // -1 in 1 byte
        let payload = build_ilst_tag(
            b"trkn",
            build_data_box(TYPE_SIGNED_INT, &[0xFF]),
        );
        let m = run(payload);
        assert_eq!(m.tags.global[0].value, "-1");
    }

    #[test]
    fn image_payload_dropped_silently() {
        let payload = build_ilst_tag(
            b"covr",
            build_data_box(TYPE_JPEG, &[0u8; 256]),
        );
        let m = run(payload);
        assert!(m.tags.global.is_empty());
    }

    #[test]
    fn malformed_data_box_skipped() {
        // Tag with no data child
        let tag = encode_box(b"\xA9nam", &[]);
        let m = run(tag);
        assert!(m.container.properties.title.is_none());
    }

    #[test]
    fn signed_int_decodes_positive_and_negative() {
        assert_eq!(decode_signed_int(&[0x05]), 5);
        assert_eq!(decode_signed_int(&[0xFF, 0xFB]), -5);
        assert_eq!(decode_signed_int(&[]), 0);
    }
}
