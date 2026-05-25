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

//! Signature prober for **recognised but unsupported** file formats —
//! port of `mkvtoolnix/src/input/unsupported_types_signature_prober.cpp`.
//!
//! mkvtoolnix runs this prober at the top of `probe_file_format`
//! (`reader_detection_and_creation.cpp:266`).  When a signature matches it
//! emits an `id_result_container_unsupported` and stops further probing.
//! We mirror that: the [`probe`] function returns a [`ContainerFormat`]
//! when it matches, the caller stamps `recognized = true` /
//! `supported = false`, and the cascade short-circuits.  PARSER-063.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::FileSource;
use crate::media_metadata::model::container::ContainerFormat;

/// Sub-signature: a fixed byte pattern that must match at `offset` from
/// the start of the file.  All entries in a [`Signature`] must match.
struct SubSignature {
  offset: usize,
  bytes: &'static [u8],
}

struct Signature {
  /// The container variant to surface when every sub-signature matches.
  format: ContainerFormat,
  sub: &'static [SubSignature],
}

/// Mirrors the six signatures in
/// `unsupported_types_signature_prober.cpp::probe_file` (lines 65-74).
const SIGNATURES: &[Signature] = &[
  // ADIF AAC — no demuxer in mkvtoolnix; recognised as AAC variant.
  Signature {
    format: ContainerFormat::Aac,
    sub: &[SubSignature {
      offset: 0,
      bytes: &[0x41, 0x44, 0x49, 0x46],
    }],
  },
  // ASF / WMV / WMA (Microsoft Advanced Streaming Format).
  Signature {
    format: ContainerFormat::Asf,
    sub: &[SubSignature {
      offset: 0,
      bytes: &[0x30, 0x26, 0xB2, 0x75],
    }],
  },
  // CDXA (`RIFF` + `CDXA` at offset 8).
  Signature {
    format: ContainerFormat::Cdxa,
    sub: &[
      SubSignature {
        offset: 0,
        bytes: &[0x52, 0x49, 0x46, 0x46],
      },
      SubSignature {
        offset: 8,
        bytes: &[0x43, 0x44, 0x58, 0x41],
      },
    ],
  },
  // HD-Sub (`SP` magic at offset 0).
  Signature {
    format: ContainerFormat::HdSub,
    sub: &[SubSignature {
      offset: 0,
      bytes: &[0x53, 0x50],
    }],
  },
  // Internet Video Recording (`.R1M` magic).
  Signature {
    format: ContainerFormat::Unknown,
    sub: &[SubSignature {
      offset: 0,
      bytes: &[0x2E, 0x52, 0x31, 0x4D],
    }],
  },
  // Windows Television DVR (16-byte GUID).
  Signature {
    format: ContainerFormat::Unknown,
    sub: &[SubSignature {
      offset: 0,
      bytes: &[
        0xB7, 0xD8, 0x00, 0x20, 0x37, 0x49, 0xDA, 0x11, 0xA6, 0x4E, 0x00, 0x07, 0xE9, 0x5E, 0xAD, 0x8D,
      ],
    }],
  },
];

/// Inspect the file head and return `Some(format)` when one of the recognised
/// unsupported signatures matches.  The cursor is rewound to 0 before return.
pub fn probe(src: &mut FileSource) -> Result<Option<ContainerFormat>, ParseError> {
  // Read enough bytes to cover the longest signature (16-byte WinTV DVR GUID
  // + the 12-byte CDXA prefix).  32 bytes is more than enough.
  let mut head = [0u8; 32];
  src.seek_to(0)?;
  let n = src.read_at_most(&mut head)?;
  src.seek_to(0)?;
  for sig in SIGNATURES {
    if sig.sub.iter().all(|s| matches_at(&head[..n], s.offset, s.bytes)) {
      return Ok(Some(sig.format));
    }
  }
  Ok(None)
}

fn matches_at(head: &[u8], offset: usize, expected: &[u8]) -> bool {
  let end = match offset.checked_add(expected.len()) {
    Some(v) => v,
    None => return false,
  };
  if end > head.len() {
    return false;
  }
  &head[offset..end] == expected
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn src_for(bytes: &[u8]) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes.to_vec()))
  }

  #[test]
  fn matches_adif_at_offset_zero() {
    let mut s = src_for(b"ADIF\x00\x00\x00\x00");
    assert_eq!(probe(&mut s).unwrap(), Some(ContainerFormat::Aac));
  }

  #[test]
  fn matches_asf_guid_prefix() {
    let mut s = src_for(&[0x30, 0x26, 0xB2, 0x75, 0x00]);
    assert_eq!(probe(&mut s).unwrap(), Some(ContainerFormat::Asf));
  }

  #[test]
  fn matches_cdxa_riff_with_cdxa_at_offset_eight() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(b"CDXA");
    let mut s = src_for(&bytes);
    assert_eq!(probe(&mut s).unwrap(), Some(ContainerFormat::Cdxa));
  }

  #[test]
  fn matches_hd_sub_sp_prefix() {
    let mut s = src_for(b"SP\x00\x00");
    assert_eq!(probe(&mut s).unwrap(), Some(ContainerFormat::HdSub));
  }

  #[test]
  fn matches_windows_tv_dvr_guid() {
    let bytes = [
      0xB7, 0xD8, 0x00, 0x20, 0x37, 0x49, 0xDA, 0x11, 0xA6, 0x4E, 0x00, 0x07, 0xE9, 0x5E, 0xAD, 0x8D,
    ];
    let mut s = src_for(&bytes);
    assert_eq!(probe(&mut s).unwrap(), Some(ContainerFormat::Unknown));
  }

  #[test]
  fn no_signature_match_returns_none() {
    let mut s = src_for(&[0xAA; 32]);
    assert!(probe(&mut s).unwrap().is_none());
  }

  #[test]
  fn riff_without_cdxa_does_not_match() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(b"WAVE");
    let mut s = src_for(&bytes);
    assert!(probe(&mut s).unwrap().is_none());
  }

  #[test]
  fn cursor_is_rewound_to_zero() {
    let mut s = src_for(b"ADIF\x00");
    let _ = probe(&mut s).unwrap();
    assert_eq!(s.position(), 0);
  }
}
