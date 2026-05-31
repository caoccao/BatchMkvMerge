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

//! FOURCC → codec name lookup for AVI `biCompression` and MP4 sample-entry
//! types.  Both spellings (`H264` and `h264`, `XVID` and `xvid`, ...) are
//! handled by upper-casing the input.

use super::TrackKind;

/// Look up a FOURCC.  The input may be either four ASCII bytes or a
/// trimmed string (e.g. `"H264"`, `"avc1"`).  Returns `None` for unknown
/// codes.
pub fn lookup(fourcc: &str) -> Option<FourccEntry> {
  let key = normalise(fourcc)?;
  TABLE.iter().copied().find(|e| e.code == key)
}

/// Same as [`lookup`] but takes a raw 4-byte array.
pub fn lookup_bytes(bytes: [u8; 4]) -> Option<FourccEntry> {
  let s: String = bytes.iter().map(|b| *b as char).collect();
  lookup(&s)
}

fn normalise(input: &str) -> Option<String> {
  // Strip control bytes (NUL, leading 0xa9, etc.) but DO NOT trim spaces —
  // FOURCCs use trailing spaces as padding (e.g. "AC3 ", "MP3 ").
  let cleaned: String = input.chars().filter(|c| !c.is_ascii_control()).collect();
  if cleaned.len() != 4 {
    return None;
  }
  Some(cleaned.to_ascii_uppercase())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FourccEntry {
  /// Upper-case ASCII FOURCC (the catalogue key).
  pub code: &'static str,
  pub name: &'static str,
  pub kind: TrackKind,
}

const TABLE: &[FourccEntry] = &[
  // Video
  FourccEntry {
    code: "AVC1",
    name: "AVC/H.264",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "AV01",
    name: "AV1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "DIV3",
    name: "DivX 3 Low-Motion",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "DIVX",
    name: "DivX 4/5",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "DRAC",
    name: "Dirac",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "DVSD",
    name: "DV (SD)",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "FFV1",
    name: "FFV1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "FLV1",
    name: "Sorenson Spark (FLV1)",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "H264",
    name: "AVC/H.264",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "H265",
    name: "HEVC/H.265",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "HEV1",
    name: "HEVC/H.265",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "HVC1",
    name: "HEVC/H.265",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "JPEG",
    name: "Motion JPEG",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MJPG",
    name: "Motion JPEG",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MP41",
    name: "MPEG-4 v1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MP42",
    name: "MPEG-4 v2",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MP43",
    name: "MPEG-4 v3",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MP4V",
    name: "MPEG-4 (generic)",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MPG1",
    name: "MPEG-1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MPG2",
    name: "MPEG-2",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "MPEG",
    name: "MPEG-1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "PRES",
    name: "Apple ProRes",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "RV10",
    name: "RealVideo 1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "RV20",
    name: "RealVideo 2",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "RV30",
    name: "RealVideo 3",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "RV40",
    name: "RealVideo 4",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "THEO",
    name: "Theora",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "VC1H",
    name: "VC-1 (high profile)",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "VC1L",
    name: "VC-1 (low profile)",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "VP08",
    name: "VP8",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "VP09",
    name: "VP9",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "WMV1",
    name: "Windows Media Video 7",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "WMV2",
    name: "Windows Media Video 8",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "WMV3",
    name: "Windows Media Video 9",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "WVC1",
    name: "VC-1",
    kind: TrackKind::Video,
  },
  FourccEntry {
    code: "XVID",
    name: "Xvid",
    kind: TrackKind::Video,
  },
  // Audio
  FourccEntry {
    code: "AAC ",
    name: "AAC",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "AC-3",
    name: "AC-3",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "AC3 ",
    name: "AC-3",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "ALAC",
    name: "Apple Lossless (ALAC)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "DTS ",
    name: "DTS",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "DTSC",
    name: "DTS (core)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "DTSE",
    name: "DTS Express",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "DTSH",
    name: "DTS-HD",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "EC-3",
    name: "E-AC-3",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "FLAC",
    name: "FLAC",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "MP4A",
    name: "MPEG-4 audio (AAC)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "MP3 ",
    name: "MP3",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: ".MP3",
    name: "MP3",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "OPUS",
    name: "Opus",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "OGG ",
    name: "Ogg",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "PCM ",
    name: "PCM",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "LPCM",
    name: "PCM",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "IN24",
    name: "PCM (signed, 24-bit)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "RAW ",
    name: "Raw PCM",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "SOWT",
    name: "PCM (signed, little-endian)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "TWOS",
    name: "PCM (signed, big-endian)",
    kind: TrackKind::Audio,
  },
  FourccEntry {
    code: "VORB",
    name: "Vorbis",
    kind: TrackKind::Audio,
  },
];

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn looks_up_video_fourccs() {
    let m = lookup("H264").unwrap();
    assert_eq!(m.name, "AVC/H.264");
    assert_eq!(m.kind, TrackKind::Video);
    let m = lookup("avc1").unwrap();
    assert_eq!(m.name, "AVC/H.264");
    let m = lookup("XVID").unwrap();
    assert_eq!(m.name, "Xvid");
  }

  #[test]
  fn looks_up_audio_fourccs() {
    let m = lookup("MP4A").unwrap();
    assert_eq!(m.name, "MPEG-4 audio (AAC)");
    let m = lookup("FLAC").unwrap();
    assert_eq!(m.name, "FLAC");
  }

  #[test]
  fn lookup_bytes_matches_lookup_str() {
    let bytes = lookup_bytes([b'a', b'v', b'c', b'1']).unwrap();
    let s = lookup("avc1").unwrap();
    assert_eq!(bytes, s);
  }

  #[test]
  fn upcase_normalisation() {
    assert_eq!(lookup("h264").unwrap().name, "AVC/H.264");
    assert_eq!(lookup("H264").unwrap().name, "AVC/H.264");
    assert_eq!(lookup("h264").unwrap(), lookup("H264").unwrap());
  }

  #[test]
  fn space_padded_fourccs_match_when_padding_kept() {
    // "AC3" alone is 3 chars and is rejected because FOURCCs are 4.
    assert!(lookup("AC3").is_none());
    // Padded with a space it matches.
    assert_eq!(lookup("AC3 ").unwrap().name, "AC-3");
  }

  #[test]
  fn wrong_length_is_none() {
    assert!(lookup("ABC").is_none());
    assert!(lookup("ABCDE").is_none());
    assert!(lookup("").is_none());
  }

  #[test]
  fn control_characters_stripped() {
    // FourCCs in MP4 can have leading 0xa9 ©; we tolerate stripping control bytes.
    let with_nul = "\0AC1";
    assert!(
      lookup(with_nul).is_none(),
      "lookup should reject 3-char fourcc after stripping"
    );
  }

  #[test]
  fn unknown_fourcc_is_none() {
    assert!(lookup("ZZZZ").is_none());
  }
}
