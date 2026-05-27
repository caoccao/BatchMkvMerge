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

//! `mdhd` (media header) box.  Per ISO/IEC 14496-12 §8.4.2.
//!
//! Carries the track's media timescale + duration, plus a packed 5-bit
//! per-character ISO-639-2/T language code.  The encoding is:
//!
//! ```text
//! language = (char1 + 0x60) << 10
//!          | (char2 + 0x60) << 5
//!          | (char3 + 0x60)
//! ```
//!
//! Each character is mapped from 'a'..='z' to 1..=26.  Invalid packed values
//! are left absent instead of being repaired into a synthetic language.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::BoxHeader;

#[derive(Debug, Clone)]
pub struct MediaHeader {
  pub version: u8,
  pub timescale: u32,
  pub duration: u64,
  pub language_iso_639_2: Option<String>,
}

pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<MediaHeader, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 24 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("mdhd payload {payload} bytes is too small"),
    });
  }
  let version = src.read_u8()?;
  let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
  // PARSER-146: mkvtoolnix accepts only mdhd versions 0 and 1
  // (r_qtmp4.cpp:655-682) — any other version means the field layout would
  // be misread, so reject the track.
  if version > 1 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("unsupported mdhd version {version}"),
    });
  }
  let (timescale, duration) = match version {
    0 => {
      src.skip(8)?; // creation + modification
      (src.read_u32_be()?, src.read_u32_be()? as u64)
    }
    _ => {
      src.skip(16)?;
      (src.read_u32_be()?, src.read_u64_be()?)
    }
  };
  // PARSER-146: a zero media timescale yields unusable timing — mkvtoolnix
  // errors out (r_qtmp4.cpp:686-687).
  if timescale == 0 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: "mdhd media timescale is zero".to_string(),
    });
  }
  let raw_language = src.read_u16_be()?;
  let language = decode_packed_language_opt(raw_language);
  // skip pre_defined (2 bytes)
  src.skip(2)?;
  Ok(MediaHeader {
    version,
    timescale,
    duration,
    language_iso_639_2: language,
  })
}

/// Decode the 15-bit packed ISO-639-2 language code (top bit is reserved).
pub fn decode_packed_language(raw: u16) -> String {
  decode_packed_language_opt(raw).unwrap_or_default()
}

/// Decode the 15-bit packed ISO-639-2 language code and keep only values whose
/// three packed characters are lowercase ASCII letters.
pub fn decode_packed_language_opt(raw: u16) -> Option<String> {
  let masked = raw & 0x7FFF;
  let c0 = ((masked >> 10) & 0x1F) as u8;
  let c1 = ((masked >> 5) & 0x1F) as u8;
  let c2 = (masked & 0x1F) as u8;
  let chars: [char; 3] = [decode_char(c0), decode_char(c1), decode_char(c2)];
  if chars.iter().any(|c| !c.is_ascii_lowercase()) {
    return None;
  }
  Some(chars.iter().collect())
}

fn decode_char(packed: u8) -> char {
  // packed = ascii_value - 0x60.  Valid range 1..=26 → 'a'..='z'.
  let ascii = packed as u32 + 0x60;
  if (b'a' as u32..=b'z' as u32).contains(&ascii) {
    ascii as u8 as char
  } else {
    '?'
  }
}

#[cfg(test)]
pub(crate) fn build_mdhd_payload_v0(timescale: u32, duration: u32, language: &str) -> Vec<u8> {
  let mut p = Vec::with_capacity(24);
  p.push(0); // version
  p.extend_from_slice(&[0u8; 3]); // flags
  p.extend_from_slice(&0u32.to_be_bytes()); // creation
  p.extend_from_slice(&0u32.to_be_bytes()); // modification
  p.extend_from_slice(&timescale.to_be_bytes());
  p.extend_from_slice(&duration.to_be_bytes());
  p.extend_from_slice(&encode_packed_language(language).to_be_bytes());
  p.extend_from_slice(&0u16.to_be_bytes()); // pre_defined
  p
}

#[cfg(test)]
pub(crate) fn encode_packed_language(language: &str) -> u16 {
  let mut bytes = language.as_bytes().iter().copied();
  let c0 = bytes.next().unwrap_or(0).saturating_sub(0x60);
  let c1 = bytes.next().unwrap_or(0).saturating_sub(0x60);
  let c2 = bytes.next().unwrap_or(0).saturating_sub(0x60);
  (((c0 as u16) & 0x1F) << 10) | (((c1 as u16) & 0x1F) << 5) | ((c2 as u16) & 0x1F)
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

  #[test]
  fn parses_v0_with_eng_language() {
    let payload = build_mdhd_payload_v0(48000, 1024, "eng");
    let bytes = encode_box(b"mdhd", &payload);
    let (h, mut s) = read(bytes);
    let m = parse(&mut s, &h).unwrap();
    assert_eq!(m.version, 0);
    assert_eq!(m.timescale, 48000);
    assert_eq!(m.duration, 1024);
    assert_eq!(m.language_iso_639_2.as_deref(), Some("eng"));
  }

  #[test]
  fn parses_v1_with_jpn_language_and_64_bit_duration() {
    let mut p = Vec::new();
    p.push(1);
    p.extend_from_slice(&[0u8; 3]);
    p.extend_from_slice(&[0u8; 8]); // creation
    p.extend_from_slice(&[0u8; 8]); // modification
    p.extend_from_slice(&48000u32.to_be_bytes());
    p.extend_from_slice(&(1u64 << 40).to_be_bytes()); // duration
    p.extend_from_slice(&encode_packed_language("jpn").to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes());
    let bytes = encode_box(b"mdhd", &p);
    let (h, mut s) = read(bytes);
    let m = parse(&mut s, &h).unwrap();
    assert_eq!(m.version, 1);
    assert_eq!(m.duration, 1u64 << 40);
    assert_eq!(m.language_iso_639_2.as_deref(), Some("jpn"));
  }

  #[test]
  fn all_zero_packed_language_is_absent() {
    assert!(decode_packed_language_opt(0).is_none());
  }

  #[test]
  fn packed_language_round_trip() {
    for code in ["eng", "fra", "jpn", "deu", "zho"] {
      let packed = encode_packed_language(code);
      assert_eq!(decode_packed_language(packed), code);
    }
  }

  #[test]
  fn packed_language_top_bit_ignored() {
    let packed = encode_packed_language("eng") | 0x8000;
    assert_eq!(decode_packed_language(packed), "eng");
  }

  #[test]
  fn non_letter_packed_returns_und() {
    // raw = 0x7FFF → every component is 0x1F = 31 → not a-z
    assert!(decode_packed_language_opt(0x7FFF).is_none());
  }

  #[test]
  fn rejects_truncated_payload() {
    let bytes = encode_box(b"mdhd", &[0u8; 8]);
    let (h, mut s) = read(bytes);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
