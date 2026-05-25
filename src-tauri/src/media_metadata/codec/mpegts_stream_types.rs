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

//! MPEG-TS PMT `stream_type` byte → codec lookup.
//!
//! Sourced from ISO/IEC 13818-1 Table 2-34 and the ATSC / DVB / ARIB
//! supplementary registrations.

use super::TrackKind;

/// Look up a PMT `stream_type` byte.
pub fn lookup(stream_type: u8) -> Option<StreamTypeEntry> {
  TABLE.iter().copied().find(|e| e.stream_type == stream_type)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamTypeEntry {
  pub stream_type: u8,
  pub name: &'static str,
  pub kind: TrackKind,
}

const TABLE: &[StreamTypeEntry] = &[
  StreamTypeEntry {
    stream_type: 0x01,
    name: "MPEG-1 video",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x02,
    name: "MPEG-2 video",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x03,
    name: "MPEG-1 audio (MP1/MP2)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x04,
    name: "MPEG-2 audio (MP1/MP2)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x05,
    name: "Private sections",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x06,
    name: "Private PES (AC-3/DTS/EAC3 in DVB)",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x07,
    name: "MHEG",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x08,
    name: "DSM-CC",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0A,
    name: "DSM-CC multiprotocol encapsulation",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0B,
    name: "DSM-CC carousel",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0C,
    name: "DSM-CC stream descriptors",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0D,
    name: "DSM-CC sections",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0E,
    name: "ISO/IEC 13818-1 auxiliary",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x0F,
    name: "AAC (ADTS, MPEG-2 part 7)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x10,
    name: "MPEG-4 part 2 video",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x11,
    name: "LATM/LOAS AAC (MPEG-4 part 3)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x12,
    name: "MPEG-4 generic PES",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x13,
    name: "MPEG-4 generic sections",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x15,
    name: "Synchronised text",
    kind: TrackKind::Subtitle,
  },
  StreamTypeEntry {
    stream_type: 0x1A,
    name: "MPEG-2 IPMP",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x1B,
    name: "AVC/H.264",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x1C,
    name: "MPEG-4 raw audio",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x1D,
    name: "MPEG-4 timed text",
    kind: TrackKind::Subtitle,
  },
  StreamTypeEntry {
    stream_type: 0x1E,
    name: "MPEG-4 auxiliary video",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x1F,
    name: "AVC sub-bitstream",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x20,
    name: "MVC sub-bitstream",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x21,
    name: "JPEG-2000",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x24,
    name: "HEVC/H.265",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x25,
    name: "HEVC temporal sub-bitstream",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x42,
    name: "AVS video (Chinese)",
    kind: TrackKind::Video,
  },
  StreamTypeEntry {
    stream_type: 0x7F,
    name: "IPMP",
    kind: TrackKind::Unknown,
  },
  StreamTypeEntry {
    stream_type: 0x80,
    name: "PCM audio (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x81,
    name: "AC-3 (BD/ATSC)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x82,
    name: "DTS (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x83,
    name: "TrueHD (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x84,
    name: "E-AC-3 (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x85,
    name: "DTS-HD HRA (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x86,
    name: "DTS-HD MA (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x87,
    name: "E-AC-3 (ATSC)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0x90,
    name: "PGS subtitles (BD)",
    kind: TrackKind::Subtitle,
  },
  StreamTypeEntry {
    stream_type: 0x91,
    name: "Interactive graphics (BD)",
    kind: TrackKind::Subtitle,
  },
  StreamTypeEntry {
    stream_type: 0x92,
    name: "Text subtitles (BD)",
    kind: TrackKind::Subtitle,
  },
  StreamTypeEntry {
    stream_type: 0xA1,
    name: "E-AC-3 secondary (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0xA2,
    name: "DTS-HD Express secondary (BD)",
    kind: TrackKind::Audio,
  },
  StreamTypeEntry {
    stream_type: 0xEA,
    name: "VC-1",
    kind: TrackKind::Video,
  },
];

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn looks_up_h264() {
    let m = lookup(0x1B).unwrap();
    assert_eq!(m.name, "AVC/H.264");
    assert_eq!(m.kind, TrackKind::Video);
  }

  #[test]
  fn looks_up_hevc() {
    let m = lookup(0x24).unwrap();
    assert_eq!(m.name, "HEVC/H.265");
  }

  #[test]
  fn looks_up_aac() {
    let m = lookup(0x0F).unwrap();
    assert_eq!(m.name, "AAC (ADTS, MPEG-2 part 7)");
    assert_eq!(m.kind, TrackKind::Audio);
  }

  #[test]
  fn looks_up_pgs_subtitles() {
    let m = lookup(0x90).unwrap();
    assert_eq!(m.name, "PGS subtitles (BD)");
    assert_eq!(m.kind, TrackKind::Subtitle);
  }

  #[test]
  fn unknown_stream_type_is_none() {
    assert!(lookup(0x00).is_none());
    assert!(lookup(0xFF).is_none());
  }

  #[test]
  fn bluray_secondary_audio_streams_recognised() {
    // BD-spec secondary streams (DV-Audio).  Cross-checked against
    // mkvtoolnix r_mpeg_ts.h: stream_audio_eac3_2 = 0xA1,
    // stream_audio_dts_hd2 = 0xA2.
    assert_eq!(lookup(0xA1).unwrap().name, "E-AC-3 secondary (BD)");
    assert_eq!(lookup(0xA2).unwrap().name, "DTS-HD Express secondary (BD)");
  }

  #[test]
  fn dsm_cc_variants_recognised() {
    // ISO/IEC 13818-6 sub-types.
    assert_eq!(lookup(0x0A).unwrap().kind, TrackKind::Unknown);
    assert_eq!(lookup(0x0C).unwrap().kind, TrackKind::Unknown);
    assert_eq!(lookup(0x0D).unwrap().kind, TrackKind::Unknown);
    assert_eq!(lookup(0x0E).unwrap().kind, TrackKind::Unknown);
  }

  #[test]
  fn table_has_no_duplicate_stream_types() {
    let mut seen = std::collections::HashSet::new();
    for entry in TABLE {
      assert!(
        seen.insert(entry.stream_type),
        "duplicate stream_type 0x{:02X}",
        entry.stream_type
      );
    }
  }
}
