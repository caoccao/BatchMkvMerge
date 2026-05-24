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

//! AMF0 (Action Message Format) decoder for FLV script-data tags.  Port of
//! `mkvtoolnix/src/common/amf.cpp`.  We only surface number / bool / string
//! values from the `onMetaData` ECMA array — that's what `r_flv.cpp` uses
//! during identification (`framerate`, `width`, `height`).

use std::collections::HashMap;

const TYPE_NUMBER: u8 = 0x00;
const TYPE_BOOL: u8 = 0x01;
const TYPE_STRING: u8 = 0x02;
const TYPE_OBJECT: u8 = 0x03;
const TYPE_MOVIECLIP: u8 = 0x04;
const TYPE_NULL: u8 = 0x05;
const TYPE_UNDEFINED: u8 = 0x06;
const TYPE_REFERENCE: u8 = 0x07;
const TYPE_ECMAARRAY: u8 = 0x08;
const TYPE_OBJECT_END: u8 = 0x09;
const TYPE_ARRAY: u8 = 0x0A;
const TYPE_DATE: u8 = 0x0B;
const TYPE_LONG_STRING: u8 = 0x0C;

#[derive(Debug, Clone, PartialEq)]
pub enum AmfValue {
    Number(f64),
    Bool(bool),
    String(String),
    Other,
}

#[derive(Debug, Default, Clone)]
pub struct ScriptMetadata {
    pub meta_data: HashMap<String, AmfValue>,
}

impl ScriptMetadata {
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.meta_data.get(key) {
            Some(AmfValue::Number(v)) => Some(*v),
            _ => None,
        }
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        match self.meta_data.get(key) {
            Some(AmfValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }
}

struct AmfReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    in_meta_data: bool,
}

impl<'a> AmfReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0, in_meta_data: false }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Option<u8> {
        if self.remaining() < 1 {
            return None;
        }
        let v = self.bytes[self.pos];
        self.pos += 1;
        Some(v)
    }

    fn read_u16_be(&mut self) -> Option<u16> {
        if self.remaining() < 2 {
            return None;
        }
        let v = u16::from_be_bytes([self.bytes[self.pos], self.bytes[self.pos + 1]]);
        self.pos += 2;
        Some(v)
    }

    fn read_u32_be(&mut self) -> Option<u32> {
        if self.remaining() < 4 {
            return None;
        }
        let v = u32::from_be_bytes([
            self.bytes[self.pos],
            self.bytes[self.pos + 1],
            self.bytes[self.pos + 2],
            self.bytes[self.pos + 3],
        ]);
        self.pos += 4;
        Some(v)
    }

    fn read_f64_be(&mut self) -> Option<f64> {
        if self.remaining() < 8 {
            return None;
        }
        let mut a = [0u8; 8];
        a.copy_from_slice(&self.bytes[self.pos..self.pos + 8]);
        self.pos += 8;
        Some(f64::from_be_bytes(a))
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        if self.remaining() < n {
            return None;
        }
        self.pos += n;
        Some(())
    }

    fn read_string(&mut self, ty: u8) -> Option<String> {
        let len = if ty == TYPE_STRING {
            self.read_u16_be()? as usize
        } else {
            self.read_u32_be()? as usize
        };
        if len == 0 {
            // mkvtoolnix's `script_parser_c::read_string` skips a single
            // trailing byte after a zero-length string — preserve the same
            // behaviour so the cursor stays aligned with subsequent values.
            self.skip(1)?;
            return Some(String::new());
        }
        if self.remaining() < len {
            return None;
        }
        let raw = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        // Trim trailing NULs the way mkvtoolnix does.
        let trimmed = match raw.iter().rposition(|&b| b != 0) {
            Some(i) => &raw[..=i],
            None => &[],
        };
        Some(String::from_utf8_lossy(trimmed).into_owned())
    }

    fn read_value(&mut self, sink: &mut HashMap<String, AmfValue>) -> Option<(AmfValue, bool)> {
        let ty = self.read_u8()?;
        match ty {
            TYPE_NUMBER => Some((AmfValue::Number(self.read_f64_be()?), true)),
            TYPE_BOOL => Some((AmfValue::Bool(self.read_u8()? != 0), true)),
            TYPE_STRING | TYPE_LONG_STRING => {
                let s = self.read_string(ty)?;
                self.in_meta_data = s == "onMetaData";
                Some((AmfValue::String(s), true))
            }
            TYPE_OBJECT => {
                let mut dummy = HashMap::new();
                self.read_properties(&mut dummy)?;
                Some((AmfValue::Other, true))
            }
            TYPE_MOVIECLIP | TYPE_NULL | TYPE_UNDEFINED | TYPE_OBJECT_END => {
                Some((AmfValue::Other, true))
            }
            TYPE_REFERENCE => {
                self.skip(2)?;
                Some((AmfValue::Other, true))
            }
            TYPE_DATE => {
                self.skip(10)?;
                Some((AmfValue::Other, true))
            }
            TYPE_ECMAARRAY => {
                self.skip(4)?; // approximate array length
                let target_in_meta = self.in_meta_data;
                self.in_meta_data = false;
                if target_in_meta {
                    self.read_properties(sink)?;
                } else {
                    let mut dummy = HashMap::new();
                    self.read_properties(&mut dummy)?;
                }
                Some((AmfValue::Other, true))
            }
            TYPE_ARRAY => {
                let n = self.read_u32_be()?;
                for _ in 0..n {
                    self.read_value(sink)?;
                }
                Some((AmfValue::Other, true))
            }
            _ => Some((AmfValue::Other, false)),
        }
    }

    fn read_properties(&mut self, sink: &mut HashMap<String, AmfValue>) -> Option<()> {
        loop {
            let key = self.read_string(TYPE_STRING)?;
            let (value, ok) = self.read_value(sink)?;
            if key.is_empty() || !ok {
                return Some(());
            }
            sink.insert(key, value);
        }
    }
}

/// Parse an FLV script-data payload.  Surfaces the `onMetaData` map; other
/// values are dropped.
pub fn parse(bytes: &[u8]) -> ScriptMetadata {
    let mut reader = AmfReader::new(bytes);
    let mut meta = HashMap::new();
    while reader.pos < bytes.len() {
        match reader.read_value(&mut meta) {
            Some((_, true)) => continue,
            _ => break,
        }
    }
    ScriptMetadata { meta_data: meta }
}

#[cfg(test)]
pub(crate) fn build_on_meta_data(entries: &[(&str, AmfValue)]) -> Vec<u8> {
    let mut buf = Vec::new();
    // First value: AMF0 string "onMetaData"
    buf.push(TYPE_STRING);
    let key = b"onMetaData";
    buf.extend_from_slice(&(key.len() as u16).to_be_bytes());
    buf.extend_from_slice(key);
    // Second value: ECMA array (the metadata)
    buf.push(TYPE_ECMAARRAY);
    buf.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (k, v) in entries {
        let kb = k.as_bytes();
        buf.extend_from_slice(&(kb.len() as u16).to_be_bytes());
        buf.extend_from_slice(kb);
        match v {
            AmfValue::Number(n) => {
                buf.push(TYPE_NUMBER);
                buf.extend_from_slice(&n.to_be_bytes());
            }
            AmfValue::Bool(b) => {
                buf.push(TYPE_BOOL);
                buf.push(if *b { 1 } else { 0 });
            }
            AmfValue::String(s) => {
                buf.push(TYPE_STRING);
                buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
                buf.extend_from_slice(s.as_bytes());
            }
            AmfValue::Other => {
                buf.push(TYPE_NULL);
            }
        }
    }
    // Trailer: empty key + object_end marker
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.push(TYPE_OBJECT_END);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_video_dimensions_from_on_meta_data() {
        let blob = build_on_meta_data(&[
            ("width", AmfValue::Number(1920.0)),
            ("height", AmfValue::Number(1080.0)),
            ("framerate", AmfValue::Number(29.97)),
            ("hasAudio", AmfValue::Bool(true)),
            ("videocodecid", AmfValue::Number(7.0)),
            ("creator", AmfValue::String("Adobe Flash".to_string())),
        ]);
        let m = parse(&blob);
        assert_eq!(m.number("width"), Some(1920.0));
        assert_eq!(m.number("height"), Some(1080.0));
        assert!((m.number("framerate").unwrap() - 29.97).abs() < 1e-9);
        assert_eq!(m.number("videocodecid"), Some(7.0));
        assert_eq!(m.string("creator"), Some("Adobe Flash"));
    }

    #[test]
    fn parse_ignores_other_amf_objects_outside_on_meta_data() {
        // First a NULL value (no embedded ECMA array)
        let mut blob = vec![TYPE_NULL];
        blob.extend(build_on_meta_data(&[("width", AmfValue::Number(640.0))]));
        let m = parse(&blob);
        assert_eq!(m.number("width"), Some(640.0));
    }

    #[test]
    fn parse_returns_empty_on_truncated_payload() {
        let m = parse(&[TYPE_STRING, 0x00]);
        assert!(m.meta_data.is_empty());
    }

    #[test]
    fn parse_handles_unknown_type_gracefully() {
        // Type byte 0xEE is undefined — reader stops cleanly.
        let m = parse(&[0xEE]);
        assert!(m.meta_data.is_empty());
    }

    #[test]
    fn parse_recognises_long_string() {
        let mut blob = vec![TYPE_LONG_STRING];
        let payload = b"onMetaData";
        blob.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        blob.extend_from_slice(payload);
        // No ECMA array follows — confirms long-string is consumed without
        // throwing.
        let m = parse(&blob);
        assert!(m.meta_data.is_empty());
    }

    #[test]
    fn metadata_number_returns_none_for_string_value() {
        let blob = build_on_meta_data(&[("title", AmfValue::String("hello".to_string()))]);
        let m = parse(&blob);
        assert!(m.number("title").is_none());
        assert_eq!(m.string("title"), Some("hello"));
    }

    #[test]
    fn parse_skips_amf_array_entries_without_writing_to_meta() {
        // STRICT ARRAY containing a number — should be skipped, not surfaced.
        let blob = vec![TYPE_ARRAY, 0, 0, 0, 1, TYPE_NUMBER, 0x40, 0, 0, 0, 0, 0, 0, 0];
        let m = parse(&blob);
        assert!(m.meta_data.is_empty());
    }

    #[test]
    fn parse_skips_bool_movieclip_reference_date_object_outside_meta() {
        let mut blob = Vec::new();
        blob.push(TYPE_BOOL);
        blob.push(1);
        blob.push(TYPE_MOVIECLIP);
        blob.push(TYPE_NULL);
        blob.push(TYPE_UNDEFINED);
        blob.push(TYPE_OBJECT_END);
        blob.push(TYPE_REFERENCE);
        blob.extend_from_slice(&[0u8; 2]);
        blob.push(TYPE_DATE);
        blob.extend_from_slice(&[0u8; 10]);
        // Embedded OBJECT — must be consumed entirely.
        blob.push(TYPE_OBJECT);
        // Key + value + terminating empty key + object-end marker
        blob.extend_from_slice(&3u16.to_be_bytes());
        blob.extend_from_slice(b"key");
        blob.push(TYPE_BOOL);
        blob.push(0);
        blob.extend_from_slice(&0u16.to_be_bytes());
        blob.push(TYPE_OBJECT_END);
        // Finally the real onMetaData entry
        blob.extend(build_on_meta_data(&[("width", AmfValue::Number(720.0))]));

        let m = parse(&blob);
        assert_eq!(m.number("width"), Some(720.0));
    }

    #[test]
    fn parse_handles_zero_length_string() {
        // String type with size 0 — reader skips a single trailing byte then
        // moves on.
        let mut blob = vec![TYPE_STRING, 0, 0, 0xFF];
        blob.extend(build_on_meta_data(&[("framerate", AmfValue::Number(25.0))]));
        let m = parse(&blob);
        assert_eq!(m.number("framerate"), Some(25.0));
    }

    #[test]
    fn metadata_string_returns_none_for_number_value() {
        let blob = build_on_meta_data(&[("width", AmfValue::Number(640.0))]);
        let m = parse(&blob);
        assert!(m.string("width").is_none());
    }

    #[test]
    fn parse_returns_empty_when_amf_array_payload_truncated() {
        // ECMA_ARRAY header without the 4-byte array length — must short-circuit.
        let blob = vec![TYPE_ECMAARRAY, 0];
        let m = parse(&blob);
        assert!(m.meta_data.is_empty());
    }
}
