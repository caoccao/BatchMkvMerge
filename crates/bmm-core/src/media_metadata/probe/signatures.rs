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

//! Magic-byte signatures used by the probe cascade.  These are *advisory* —
//! the authoritative claim for a file always comes from the chosen reader's
//! `Reader::probe` implementation, which consults the same bytes but with
//! reader-specific tolerance (e.g. EBML id prefix can sit after `0xEC` Void
//! padding, MP4 `ftyp` is preceded by a 4-byte length).
//!
//! Phase 3 only uses the EBML/Matroska signature; entries for other formats
//! are present so later phases can wire them in without re-shaping the table.

/// One magic-byte description.  Match semantics: the file's first
/// `offset .. offset + bytes.len()` slice must equal `bytes`.
#[derive(Debug, Clone, Copy)]
pub struct Signature {
  /// Stable label used in debug logs.
  pub name: &'static str,
  /// Byte offset where the magic sits.  Matroska is at offset 0; some
  /// formats (e.g. M2TS) carry leading padding.
  pub offset: usize,
  /// Byte pattern.
  pub bytes: &'static [u8],
}

/// EBML head — `1A 45 DF A3`.  Identifies Matroska, WebM, MKV/MKA/MKS, MK3D.
pub const EBML_HEAD: Signature = Signature {
  name: "ebml-head",
  offset: 0,
  bytes: &[0x1A, 0x45, 0xDF, 0xA3],
};

/// RIFF wrapper used by AVI/WAV (we don't distinguish here — readers do).
pub const RIFF: Signature = Signature {
  name: "riff",
  offset: 0,
  bytes: b"RIFF",
};

/// MP4 `ftyp` box — sits 4 bytes after start.  Note this is 4 ASCII chars.
pub const MP4_FTYP: Signature = Signature {
  name: "mp4-ftyp",
  offset: 4,
  bytes: b"ftyp",
};

/// Ogg page header.
pub const OGG: Signature = Signature {
  name: "ogg",
  offset: 0,
  bytes: b"OggS",
};

/// FLAC native magic.
pub const FLAC: Signature = Signature {
  name: "flac",
  offset: 0,
  bytes: b"fLaC",
};

/// Flash Video header.
pub const FLV: Signature = Signature {
  name: "flv",
  offset: 0,
  bytes: b"FLV",
};

/// HDMV PGS subtitle segment magic.
pub const PGS: Signature = Signature {
  name: "pgs",
  offset: 0,
  bytes: &[0x50, 0x47],
};

/// WAVPACK v4 frame magic.
pub const WAVPACK: Signature = Signature {
  name: "wavpack",
  offset: 0,
  bytes: b"wvpk",
};

/// IVF (AV1/VP8/VP9 container).
pub const IVF: Signature = Signature {
  name: "ivf",
  offset: 0,
  bytes: b"DKIF",
};

/// CoreAudio CAF.
pub const CAF: Signature = Signature {
  name: "caf",
  offset: 0,
  bytes: b"caff",
};

/// RealMedia.
pub const REALMEDIA: Signature = Signature {
  name: "realmedia",
  offset: 0,
  bytes: b".RMF",
};

/// TTA lossless audio.
pub const TTA: Signature = Signature {
  name: "tta",
  offset: 0,
  bytes: b"TTA1",
};

/// HDMV TextST subtitle stream — first 0x80 byte then header.  We only check
/// the leading 0x80 here; readers do the full disambiguation.
pub const HDMV_TEXTST: Signature = Signature {
  name: "hdmv-textst",
  offset: 0,
  bytes: &[0x80],
};

/// Test the signature against a probe slice — returns `false` if the slice is
/// too short to cover the signature window.
pub fn matches(sig: &Signature, head: &[u8]) -> bool {
  let end = match sig.offset.checked_add(sig.bytes.len()) {
    Some(e) => e,
    None => return false,
  };
  if head.len() < end {
    return false;
  }
  &head[sig.offset..end] == sig.bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ebml_head_matches_matroska_prefix() {
    let head = [0x1A, 0x45, 0xDF, 0xA3, 0x42, 0x86, 0x81, 0x01];
    assert!(matches(&EBML_HEAD, &head));
  }

  #[test]
  fn ebml_head_does_not_match_random_bytes() {
    let head = [0x00, 0x00, 0x00, 0x00];
    assert!(!matches(&EBML_HEAD, &head));
  }

  #[test]
  fn mp4_ftyp_requires_offset_4() {
    let head = [0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm'];
    assert!(matches(&MP4_FTYP, &head));
    // Without the leading length bytes the offset check should fail.
    assert!(!matches(&MP4_FTYP, b"ftyp"));
  }

  #[test]
  fn short_input_returns_false_not_panic() {
    assert!(!matches(&EBML_HEAD, &[0x1A, 0x45]));
    assert!(!matches(&MP4_FTYP, b"abc"));
  }

  #[test]
  fn empty_input_never_matches_a_nonempty_signature() {
    assert!(!matches(&EBML_HEAD, &[]));
    assert!(!matches(&RIFF, &[]));
    assert!(!matches(&OGG, &[]));
    assert!(!matches(&FLAC, &[]));
  }

  #[test]
  fn each_signature_matches_its_own_bytes() {
    // Build a head buffer that places the magic at its declared offset
    // and ensure each signature matches.
    for sig in [
      &EBML_HEAD,
      &RIFF,
      &MP4_FTYP,
      &OGG,
      &FLAC,
      &FLV,
      &PGS,
      &WAVPACK,
      &IVF,
      &CAF,
      &REALMEDIA,
      &TTA,
      &HDMV_TEXTST,
    ] {
      let mut head = vec![0u8; sig.offset + sig.bytes.len() + 4];
      head[sig.offset..sig.offset + sig.bytes.len()].copy_from_slice(sig.bytes);
      assert!(matches(sig, &head), "signature {} should self-match", sig.name);
    }
  }

  #[test]
  fn signature_names_are_unique_and_nonempty() {
    let sigs = [
      &EBML_HEAD,
      &RIFF,
      &MP4_FTYP,
      &OGG,
      &FLAC,
      &FLV,
      &PGS,
      &WAVPACK,
      &IVF,
      &CAF,
      &REALMEDIA,
      &TTA,
      &HDMV_TEXTST,
    ];
    let mut names: Vec<&str> = sigs.iter().map(|s| s.name).collect();
    names.sort_unstable();
    let len = names.len();
    names.dedup();
    assert_eq!(names.len(), len, "signature names must be unique");
    assert!(names.iter().all(|n| !n.is_empty()));
  }
}
