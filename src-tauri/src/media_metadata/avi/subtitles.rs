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

//! AVI subtitle (text-track) detection — bounded port of
//! `mkvtoolnix/src/input/r_avi.cpp:118-174` (`parse_subtitle_chunks`).
//!
//! mkvtoolnix reads the *first* text chunk of every text track from the `movi`
//! list, parses its GAB2 payload, and only creates a subtitle demuxer when the
//! embedded content is recognised as SRT or SSA/ASS.  Unknown content is
//! dropped (`avi_subs_demuxer_t::TYPE_UNKNOWN` is never pushed).
//!
//! To stay inside the header-only / deadline model we walk `movi` chunk
//! *headers* only, reading the payload of just the first `NNtx` chunk for each
//! text stream and stopping once every text track has been resolved or the
//! caller's deadline expires.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::attachment::Attachment;

use super::riff::{self, ChunkHeader};

/// GAB2 magic at the start of a VirtualDub text chunk (`"GAB2\0"`).
const GAB2_TAG: &[u8; 4] = b"GAB2";
/// GAB2 block id carrying the subtitle file content.
const GAB2_ID_SUBTITLES: u16 = 4;

/// The recognised subtitle kinds.  Mirrors `avi_subs_demuxer_t::TYPE_*`; the
/// `TYPE_UNKNOWN` case is represented by `None` (the demuxer is dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AviSubtitleKind {
  Srt,
  Ssa,
}

/// One recognised subtitle demuxer extracted from a `movi` text chunk.
#[derive(Debug, Clone)]
pub struct AviSubtitleDemuxer {
  pub kind: AviSubtitleKind,
  /// Detected text encoding label (e.g. `UTF-8`).  Mirrors
  /// `mm_text_io_c::get_encoding()` surfaced via `id::encoding`.
  pub encoding: Option<String>,
  /// Embedded `[Fonts]` / `[Graphics]` attachments harvested from an SSA/ASS
  /// payload (PARSER-213, mirrors `avi_reader_c::identify_attachments`).  Empty
  /// for SRT and for SSA payloads without attachment sections.
  pub attachments: Vec<Attachment>,
}

impl AviSubtitleKind {
  /// Matroska codec id used by mkvtoolnix's subtitle packetizers.
  pub fn codec_id(self) -> &'static str {
    match self {
      Self::Srt => "S_TEXT/UTF8",
      Self::Ssa => "S_TEXT/ASS",
    }
  }

  /// Human-readable codec name (mirrors `codec_c::get_name`).
  pub fn codec_name(self) -> &'static str {
    match self {
      Self::Srt => "SRT",
      Self::Ssa => "SSA/ASS",
    }
  }
}

/// Build the 4-byte `NNtx` text-chunk tag for the text stream at overall
/// stream index `stream_index` (the position of the `strl` inside `hdrl`).
/// Mirrors avilib's tag construction (`avilib.c:2697-2714`) which uses the
/// two-digit decimal stream number followed by the two-character chunk id —
/// `tx` for text.
fn text_chunk_tag(stream_index: usize) -> [u8; 4] {
  let n = stream_index % 100;
  [(n / 10) as u8 + b'0', (n % 10) as u8 + b'0', b't', b'x']
}

/// Walk the `movi` LIST looking for the first text chunk of each `text_streams`
/// entry, parse its GAB2 payload, and return the recognised subtitle demuxers
/// **in text-stream order**.  Only chunk headers are scanned, only the first
/// matching chunk per stream is read, and the scan stops early once every text
/// stream is resolved.
pub fn parse_subtitle_chunks(
  src: &mut FileSource,
  movi: &ChunkHeader,
  text_streams: &[usize],
  deadline: &Deadline,
) -> Result<Vec<AviSubtitleDemuxer>, ParseError> {
  if text_streams.is_empty() {
    return Ok(Vec::new());
  }
  // Per text stream: the recognised demuxer (if any) and whether its first
  // chunk has already been located so we never read a stream twice.
  let mut recognised: Vec<Option<AviSubtitleDemuxer>> = vec![None; text_streams.len()];
  let mut resolved: Vec<bool> = vec![false; text_streams.len()];
  let mut remaining = text_streams.len();

  // The first child of the movi LIST sits after the 4-byte sub-type FOURCC.
  let first_child = movi.payload_start() + 4;
  let parent_end = movi.payload_end();
  let stream_end = src.length();
  src.seek_to(first_child)?;

  while remaining > 0 {
    deadline.check("avi::subtitles")?;
    let pos = src.position();
    if pos >= parent_end || parent_end - pos < 8 {
      break;
    }
    if let Some(end) = stream_end {
      if pos >= end || end - pos < 8 {
        break;
      }
    }
    let child = match riff::read_chunk_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };
    if child.payload_end() > parent_end {
      break;
    }
    // `rec ` sub-lists wrap interleaved chunks — descend into them by
    // skipping the LIST sub-type FOURCC instead of skipping the payload.
    // Mirrors avilib's "may contain sub-lists" handling
    // (`avilib.c:2786-2792`).
    if &child.kind == b"LIST" {
      src.seek_to(child.payload_start() + 4)?;
      continue;
    }

    // Does this chunk belong to a still-unresolved text stream?
    let slot = text_streams
      .iter()
      .position(|&idx| child.kind == text_chunk_tag(idx))
      .filter(|&slot| !resolved[slot]);
    if let Some(slot) = slot {
      let bytes = riff::read_payload(src, &child, deadline.max_element_size())?;
      recognised[slot] = classify_gab2(&bytes);
      resolved[slot] = true;
      remaining -= 1;
      // read_payload advanced the cursor to payload_end; pad-align below.
      if child.needs_pad_byte() {
        src.seek_to(child.payload_end().saturating_add(1))?;
      }
      continue;
    }

    riff::skip_payload_with_pad(src, &child)?;
  }

  Ok(recognised.into_iter().flatten().collect())
}

/// Parse a GAB2 chunk payload and classify the embedded subtitle file.
///
/// Layout (mirrors `r_avi.cpp:139-170`):
///
/// ```text
/// u32be "GAB2"  (magic)
/// u8           (skipped)
/// repeat:
///   u16le id
///   u32le len
///   len bytes payload      (id == 4 ⇒ subtitle file content)
/// ```
///
/// The subtitle content is decoded then probed with the canonical SRT / SSA
/// probers.  Only recognised content yields a demuxer; unknown content returns
/// `None` (mkvtoolnix drops `TYPE_UNKNOWN`).
pub fn classify_gab2(bytes: &[u8]) -> Option<AviSubtitleDemuxer> {
  if bytes.len() < 5 || &bytes[0..4] != GAB2_TAG {
    return None;
  }
  let mut pos = 5usize; // skip "GAB2" + 1 reserved byte
  let mut subtitle: Option<Vec<u8>> = None;
  while pos + 6 <= bytes.len() {
    let id = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
    let len = u32::from_le_bytes([bytes[pos + 2], bytes[pos + 3], bytes[pos + 4], bytes[pos + 5]]) as usize;
    let data_start = pos + 6;
    let data_end = data_start.saturating_add(len).min(bytes.len());
    if id == GAB2_ID_SUBTITLES {
      subtitle = Some(bytes[data_start..data_end].to_vec());
    }
    pos = data_end;
  }

  let subtitle = subtitle?;
  if subtitle.is_empty() {
    return None;
  }
  classify_subtitle(&subtitle)
}

/// Classify a raw subtitle payload as SRT or SSA, returning `None` for
/// unrecognised content.  SRT is probed first to match mkvtoolnix's order
/// (`r_avi.cpp:162-165`).
fn classify_subtitle(data: &[u8]) -> Option<AviSubtitleDemuxer> {
  let detected = crate::media_metadata::subtitles::encoding::detect(data);
  let text = crate::media_metadata::subtitles::encoding::decode_lossy(data);
  let encoding = Some(detected.label.to_string());
  if crate::media_metadata::subtitles::srt::has_srt_timecode_line(&text) {
    return Some(AviSubtitleDemuxer {
      kind: AviSubtitleKind::Srt,
      encoding,
      attachments: Vec::new(),
    });
  }
  if crate::media_metadata::subtitles::ssa::classify(&text).is_some() {
    // PARSER-213: harvest the embedded `[Fonts]` / `[Graphics]` attachments,
    // mirroring upstream's `identify_attachments` which re-parses the SSA
    // payload with an `ssa_parser_c` (`../mkvtoolnix/src/input/r_avi.cpp:942-959`).
    let attachments = crate::media_metadata::subtitles::ssa::parse_ssa(&text).attachments;
    return Some(AviSubtitleDemuxer {
      kind: AviSubtitleKind::Ssa,
      encoding,
      attachments,
    });
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;

  fn gab2_with_subtitle(content: &[u8]) -> Vec<u8> {
    let mut g = b"GAB2\0".to_vec();
    // filename block (id 2) — should be skipped.
    g.extend_from_slice(&2u16.to_le_bytes());
    g.extend_from_slice(&4u32.to_le_bytes());
    g.extend_from_slice(b"a.sr");
    // subtitle block (id 4)
    g.extend_from_slice(&4u16.to_le_bytes());
    g.extend_from_slice(&(content.len() as u32).to_le_bytes());
    g.extend_from_slice(content);
    g
  }

  #[test]
  fn text_chunk_tag_two_digit_stream() {
    assert_eq!(&text_chunk_tag(0), b"00tx");
    assert_eq!(&text_chunk_tag(2), b"02tx");
    assert_eq!(&text_chunk_tag(12), b"12tx");
  }

  #[test]
  fn classify_gab2_recognises_srt() {
    let srt = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHello\r\n";
    let demux = classify_gab2(&gab2_with_subtitle(srt)).unwrap();
    assert_eq!(demux.kind, AviSubtitleKind::Srt);
    assert_eq!(demux.encoding.as_deref(), Some("UTF-8"));
  }

  #[test]
  fn classify_gab2_recognises_ssa() {
    let ssa = b"[Script Info]\r\nScriptType: v4.00+\r\n[V4+ Styles]\r\n";
    let demux = classify_gab2(&gab2_with_subtitle(ssa)).unwrap();
    assert_eq!(demux.kind, AviSubtitleKind::Ssa);
    assert!(demux.attachments.is_empty());
  }

  /// Minimal SSA UUencode (mirrors `subtitles::ssa::decode_uu`'s inverse).
  fn uu_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
      let chunk = &data[i..(i + 3).min(data.len())];
      let mut value: u32 = 0;
      for (idx, b) in chunk.iter().enumerate() {
        value |= u32::from(*b) << ((2 - idx) * 8);
      }
      let chars_out = match chunk.len() {
        3 => 4,
        2 => 3,
        _ => 2,
      };
      for idx in 0..chars_out {
        let group = (value >> (6 * (3 - idx))) & 0x3f;
        out.push((group as u8 + 33) as char);
      }
      i += 3;
    }
    out
  }

  #[test]
  fn classify_gab2_ssa_harvests_font_attachment() {
    // PARSER-213: an SSA payload with an embedded `[Fonts]` block yields an
    // attachment with the decoded MIME type.
    let font = [0x00u8, 0x01, 0x00, 0x00, 0x00, 0x42, 0x43, 0x44];
    let mut ssa = String::from("[Script Info]\r\nScriptType: v4.00+\r\n[V4+ Styles]\r\n[Fonts]\r\n");
    ssa.push_str("fontname: myfont.ttf\r\n");
    ssa.push_str(&uu_encode(&font));
    ssa.push_str("\r\n");
    let demux = classify_gab2(&gab2_with_subtitle(ssa.as_bytes())).unwrap();
    assert_eq!(demux.kind, AviSubtitleKind::Ssa);
    assert_eq!(demux.attachments.len(), 1);
    assert_eq!(demux.attachments[0].file_name, "myfont.ttf");
    assert_eq!(demux.attachments[0].mime_type.as_deref(), Some("font/sfnt"));
    assert_eq!(demux.attachments[0].size, font.len() as u64);
  }

  #[test]
  fn classify_gab2_rejects_unrecognised_text() {
    let junk = b"this is just some plain text, not a subtitle file at all";
    assert!(classify_gab2(&gab2_with_subtitle(junk)).is_none());
  }

  #[test]
  fn classify_gab2_rejects_non_gab2() {
    assert!(classify_gab2(b"NOTGAB2....").is_none());
    assert!(classify_gab2(b"GAB").is_none());
  }

  #[test]
  fn classify_gab2_rejects_missing_subtitle_block() {
    // Only a filename block, no id == 4 payload.
    let mut g = b"GAB2\0".to_vec();
    g.extend_from_slice(&2u16.to_le_bytes());
    g.extend_from_slice(&4u32.to_le_bytes());
    g.extend_from_slice(b"a.sr");
    assert!(classify_gab2(&g).is_none());
  }

  #[test]
  fn codec_id_and_name_match() {
    assert_eq!(AviSubtitleKind::Srt.codec_id(), "S_TEXT/UTF8");
    assert_eq!(AviSubtitleKind::Srt.codec_name(), "SRT");
    assert_eq!(AviSubtitleKind::Ssa.codec_id(), "S_TEXT/ASS");
    assert_eq!(AviSubtitleKind::Ssa.codec_name(), "SSA/ASS");
  }
}
