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

//! `ftyp` (File Type) box parsing.  Format per ISO/IEC 14496-12 §4.3:
//!
//! ```text
//! aligned(8) class FileTypeBox extends Box('ftyp') {
//!   unsigned int(32)  major_brand;
//!   unsigned int(32)  minor_version;
//!   unsigned int(32)  compatible_brands[];   // to end of box
//! }
//! ```
//!
//! We extract the major brand + every compatible brand so the reader can
//! disambiguate QuickTime (`qt  `) from MP4 (`isom`, `mp41`, `mp42`, `iso2`,
//! `iso4`, `iso5`, `iso6`, `avc1`, ...).  The minor version is discarded —
//! identification doesn't need it.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;

use super::atom::BoxHeader;

#[derive(Debug, Clone)]
pub struct FileType {
  pub major_brand: String,
  pub minor_version: u32,
  pub compatible_brands: Vec<String>,
}

impl FileType {
  /// Classify the brand set into a typed `ContainerFormat`.  QuickTime
  /// (`qt  `) wins outright; otherwise we look for an MP4-family brand in
  /// the major brand or any compatible brand.
  pub fn classify(&self) -> ContainerFormat {
    if self.major_brand == "qt  " || self.compatible_brands.iter().any(|b| b == "qt  ") {
      return ContainerFormat::QuickTime;
    }
    if is_mp4_brand(&self.major_brand) {
      return ContainerFormat::Mp4;
    }
    if self.compatible_brands.iter().any(|b| is_mp4_brand(b)) {
      return ContainerFormat::Mp4;
    }
    // No brand we recognise as MP4/QT; mkvmerge still treats the file as
    // a QtMp4 reader claim, so default to Mp4 (matches `qtmp4_reader_c`
    // behaviour when the brand list is empty / unrecognised).
    ContainerFormat::Mp4
  }
}

/// Recognised MP4 brand prefixes / exact matches.
fn is_mp4_brand(brand: &str) -> bool {
  matches!(
    brand,
    "isom"
      | "iso2"
      | "iso3"
      | "iso4"
      | "iso5"
      | "iso6"
      | "iso7"
      | "iso8"
      | "iso9"
      | "mp41"
      | "mp42"
      | "mp4v"
      | "M4V "
      | "M4A "
      | "M4P "
      | "M4B "
      | "avc1"
      | "av01"
      | "dby1"
      | "mp71"
      | "msdh"
      | "msix"
      | "f4v "
      | "MSNV"
      | "iml1"
      | "iml2"
  )
}

/// Read the ftyp payload from the cursor (cursor must be at `header.payload_start`).
pub fn parse(src: &mut FileSource, header: &BoxHeader) -> Result<FileType, ParseError> {
  let payload = header.payload_size().unwrap_or(0);
  if payload < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: header.start,
      reason: format!("ftyp payload {payload} bytes is shorter than 8"),
    });
  }
  let major_bytes = src.read_array::<4>()?;
  let minor_version = src.read_u32_be()?;
  let mut compatible = Vec::new();
  let mut consumed: u64 = 8;
  while consumed + 4 <= payload {
    let b = src.read_array::<4>()?;
    compatible.push(bytes_to_brand(&b));
    consumed += 4;
  }
  // Skip any trailing non-multiple-of-4 padding.
  if consumed < payload {
    let pad = payload - consumed;
    src.skip(pad)?;
  }
  Ok(FileType {
    major_brand: bytes_to_brand(&major_bytes),
    minor_version,
    compatible_brands: compatible,
  })
}

fn bytes_to_brand(b: &[u8; 4]) -> String {
  b.iter()
    .map(|byte| {
      if (0x20..=0x7E).contains(byte) {
        *byte as char
      } else {
        '?'
      }
    })
    .collect()
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

  fn build_ftyp(major: &[u8; 4], minor: u32, compats: &[&[u8; 4]]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(major);
    payload.extend_from_slice(&minor.to_be_bytes());
    for c in compats {
      payload.extend_from_slice(*c);
    }
    encode_box(b"ftyp", &payload)
  }

  #[test]
  fn parses_minimal_ftyp() {
    let bytes = build_ftyp(b"isom", 512, &[]);
    let (h, mut s) = read(bytes);
    let ft = parse(&mut s, &h).unwrap();
    assert_eq!(ft.major_brand, "isom");
    assert_eq!(ft.minor_version, 512);
    assert!(ft.compatible_brands.is_empty());
  }

  #[test]
  fn parses_compatible_brand_list() {
    let bytes = build_ftyp(b"mp42", 0, &[b"isom", b"mp41", b"avc1"]);
    let (h, mut s) = read(bytes);
    let ft = parse(&mut s, &h).unwrap();
    assert_eq!(ft.major_brand, "mp42");
    assert_eq!(ft.compatible_brands, vec!["isom", "mp41", "avc1"]);
  }

  #[test]
  fn classifies_quicktime_via_major_brand() {
    let ft = FileType {
      major_brand: "qt  ".to_string(),
      minor_version: 0,
      compatible_brands: vec![],
    };
    assert_eq!(ft.classify(), ContainerFormat::QuickTime);
  }

  #[test]
  fn classifies_quicktime_via_compatible_brand() {
    let ft = FileType {
      major_brand: "mp42".to_string(),
      minor_version: 0,
      compatible_brands: vec!["qt  ".to_string()],
    };
    assert_eq!(ft.classify(), ContainerFormat::QuickTime);
  }

  #[test]
  fn classifies_isom_as_mp4() {
    let ft = FileType {
      major_brand: "isom".to_string(),
      minor_version: 0,
      compatible_brands: vec![],
    };
    assert_eq!(ft.classify(), ContainerFormat::Mp4);
  }

  #[test]
  fn classifies_mp42_as_mp4() {
    let ft = FileType {
      major_brand: "mp42".to_string(),
      minor_version: 0,
      compatible_brands: vec![],
    };
    assert_eq!(ft.classify(), ContainerFormat::Mp4);
  }

  #[test]
  fn classifies_compatible_brand_when_major_is_unknown() {
    let ft = FileType {
      major_brand: "XXXX".to_string(),
      minor_version: 0,
      compatible_brands: vec!["mp42".to_string()],
    };
    assert_eq!(ft.classify(), ContainerFormat::Mp4);
  }

  #[test]
  fn classifies_unknown_brand_defaults_to_mp4() {
    let ft = FileType {
      major_brand: "????".to_string(),
      minor_version: 0,
      compatible_brands: vec![],
    };
    assert_eq!(ft.classify(), ContainerFormat::Mp4);
  }

  #[test]
  fn classifies_m4a_brand_as_mp4() {
    let ft = FileType {
      major_brand: "M4A ".to_string(),
      minor_version: 0,
      compatible_brands: vec![],
    };
    assert_eq!(ft.classify(), ContainerFormat::Mp4);
  }

  #[test]
  fn rejects_payload_smaller_than_8_bytes() {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"isom");
    // Only 4 bytes payload → not enough for major + minor.
    let bytes = encode_box(b"ftyp", &payload);
    let (h, mut s) = read(bytes);
    let err = parse(&mut s, &h).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn skips_trailing_padding() {
    // Build an ftyp with 4-byte major + 4-byte minor + 3 bytes of trailing
    // garbage (not a multiple of 4).
    let mut payload = Vec::new();
    payload.extend_from_slice(b"isom");
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload.extend_from_slice(&[0xFFu8; 3]);
    let bytes = encode_box(b"ftyp", &payload);
    let (h, mut s) = read(bytes);
    let ft = parse(&mut s, &h).unwrap();
    assert_eq!(ft.major_brand, "isom");
    assert!(ft.compatible_brands.is_empty());
    // Cursor should be at the end of the box payload.
    assert_eq!(s.position(), h.payload_start() + 11);
  }

  #[test]
  fn non_ascii_brand_byte_rendered_as_question_mark() {
    let mut payload = Vec::new();
    payload.push(b'a');
    payload.push(0xFF);
    payload.push(b'c');
    payload.push(b'd');
    payload.extend_from_slice(&0u32.to_be_bytes());
    let bytes = encode_box(b"ftyp", &payload);
    let (h, mut s) = read(bytes);
    let ft = parse(&mut s, &h).unwrap();
    assert_eq!(ft.major_brand, "a?cd");
  }
}
