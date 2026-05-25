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

//! SSA / ASS reader.
//!
//! Both versions begin with a `[Script Info]` section.  The distinguishing
//! factor between SSA (v4) and ASS (v4+) is the `ScriptType:` value and the
//! styles-section header (`[V4 Styles]` vs `[V4+ Styles]`).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::attachment::Attachment;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 16 * 1024;

/// PARSER-153: mkvtoolnix builds its `ssa_parser_c` over the whole text input,
/// so the complete global header is used as codec private data and embedded
/// fonts anywhere in the file are gathered.  We read up to this many bytes for
/// header parsing (well past the 16 KiB probe window) so large `[V4+ Styles]`
/// sections and trailing `[Fonts]` blocks are not truncated.
const MAX_PARSE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsaVariant {
  Ssa,
  Ass,
}

/// SSA sections that affect global-header membership and attachment parsing.
/// Mirrors `ssa_section_e` in `../mkvtoolnix/src/input/subtitles.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SsaSection {
  None,
  Info,
  V4Styles,
  Events,
  Fonts,
  Graphics,
}

pub fn classify(text: &str) -> Option<SsaVariant> {
  let mut script_info_seen = false;
  for line in text.lines() {
    let trimmed = line.trim();
    if trimmed.is_empty() {
      continue;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "[script info]" {
      script_info_seen = true;
      continue;
    }
    if lower == "[v4+ styles]" {
      return Some(SsaVariant::Ass);
    }
    if lower == "[v4 styles]" {
      return Some(SsaVariant::Ssa);
    }
    if let Some(rest) = lower.strip_prefix("scripttype:") {
      let v = rest.trim();
      if v.contains("v4.00+") {
        return Some(SsaVariant::Ass);
      }
      if v.contains("v4.00") {
        return Some(SsaVariant::Ssa);
      }
    }
  }
  if script_info_seen {
    // Header alone → assume modern ASS.
    Some(SsaVariant::Ass)
  } else {
    None
  }
}

/// Test whether `trimmed` (already left-trimmed) is the named section header,
/// case-insensitively.  Mirrors the upstream `^\s*\[Name\]` regexes — we accept
/// a leading-trim + bracketed-name match without trailing-content tolerance,
/// which is what every real-world SSA/ASS file emits.
fn is_section(trimmed: &str, name: &str) -> bool {
  let lower = trimmed.to_ascii_lowercase();
  lower == format!("[{name}]")
}

/// Result of the single-pass SSA/ASS header parse: the global header (codec
/// private data) with `[Fonts]` / `[Graphics]` / `[Events]` dialogue lines
/// excluded, plus the embedded attachments harvested from the font/graphics
/// sections.
pub struct SsaParse {
  pub global: Option<String>,
  pub attachments: Vec<Attachment>,
}

/// Port of `ssa_parser_c::parse` (`../mkvtoolnix/src/input/subtitles.cpp:354`).
///
/// Walks the file line by line, tracking the current `[...]` section.  Lines
/// belonging to `[Fonts]` / `[Graphics]` (including the section headers) and
/// `Dialogue:` lines inside `[Events]` are kept out of the global header.
/// Embedded fonts/graphics are gathered: a `fontname:` line names the next
/// attachment, subsequent lines accumulate its UU-encoded payload, and the
/// payload is flushed (decoded + MIME-guessed) whenever the section changes or
/// a new `fontname:` arrives.
pub fn parse_ssa(text: &str) -> SsaParse {
  let mut global = String::new();
  let mut attachments = Vec::new();
  let mut section = SsaSection::None;
  let mut previous_section = SsaSection::None;
  let mut attachment_name: Option<String> = None;
  let mut attachment_data_uu = String::new();

  for raw in text.lines() {
    // The upstream parser left-strips the section regexes (`^\s*\[`) and
    // matches `Format:` / `Dialogue:` against the left-trimmed line.
    let line = raw.trim_start();
    let mut add_to_global = true;

    if is_section(line, "v4+ styles") {
      section = SsaSection::V4Styles;
    } else if is_section(line, "v4 styles") {
      section = SsaSection::V4Styles;
    } else if is_section(line, "script info") {
      section = SsaSection::Info;
    } else if is_section(line, "events") {
      section = SsaSection::Events;
    } else if is_section(line, "graphics") {
      section = SsaSection::Graphics;
      add_to_global = false;
    } else if is_section(line, "fonts") {
      section = SsaSection::Fonts;
      add_to_global = false;
    } else if section == SsaSection::Events {
      // `Format:` lines stay in the global header; only `Dialogue:` payload is
      // demuxed and therefore excluded (matches the upstream branch where only
      // the Dialogue arm clears `add_to_global`).
      if line.to_ascii_lowercase().starts_with("dialogue:") {
        add_to_global = false;
      }
    } else if section == SsaSection::Fonts || section == SsaSection::Graphics {
      let lower = line.to_ascii_lowercase();
      if let Some(rest) = lower.strip_prefix("fontname:") {
        flush_attachment(&mut attachments, &mut attachment_name, &mut attachment_data_uu, section);
        // Recover the original-case name from the raw (trimmed) line.
        let name = line[line.len() - rest.len()..].trim();
        attachment_name = Some(name.to_string());
      } else {
        attachment_data_uu.push_str(line.trim());
      }
      add_to_global = false;
    }

    if add_to_global {
      global.push_str(raw);
      global.push_str("\r\n");
    }

    if previous_section != section {
      flush_attachment(&mut attachments, &mut attachment_name, &mut attachment_data_uu, previous_section);
    }
    previous_section = section;
  }

  // Flush any attachment still pending at EOF.
  flush_attachment(&mut attachments, &mut attachment_name, &mut attachment_data_uu, section);

  SsaParse {
    global: if global.is_empty() { None } else { Some(global) },
    attachments,
  }
}

/// Port of `ssa_parser_c::add_attachment_maybe`
/// (`../mkvtoolnix/src/input/subtitles.cpp:546`).  Decodes the accumulated
/// UU-payload, derives the decoded byte size, and guesses the MIME type from
/// the decoded bytes.  No-op (resetting state) unless both a name and payload
/// are present and the section is `[Fonts]` or `[Graphics]`.
fn flush_attachment(
  attachments: &mut Vec<Attachment>,
  name: &mut Option<String>,
  data_uu: &mut String,
  section: SsaSection,
) {
  let is_attachment_section = section == SsaSection::Fonts || section == SsaSection::Graphics;
  let file_name = match name.take() {
    Some(n) if !n.is_empty() && !data_uu.is_empty() && is_attachment_section => n,
    _ => {
      *data_uu = String::new();
      return;
    }
  };

  let data = decode_uu(data_uu.as_bytes());
  let mime_type = guess_font_mime_type(&data);
  let description = match section {
    SsaSection::Graphics => "SSA/ASS embedded picture",
    _ => "SSA/ASS embedded font",
  };
  attachments.push(Attachment {
    id: attachments.len() as u32 + 1,
    file_name,
    mime_type: Some(mime_type),
    description: Some(description.to_string()),
    size: data.len() as u64,
    uid_hex: None,
  });

  *data_uu = String::new();
}

/// Port of `ssa_parser_c::decode_chars` (`../mkvtoolnix/src/input/subtitles.cpp:603`).
///
/// SSA's UUencode-like scheme: groups of 4 input characters (each holding 6
/// bits after subtracting 33) decode to 3 output bytes; a trailing partial
/// group of 3/2 input chars decodes to 2/1 bytes.  Output size is therefore
/// `len/4*3 + (len%4==3 ? 2 : len%4==2 ? 1 : 0)`.
pub fn decode_uu(data_uu: &[u8]) -> Vec<u8> {
  let full = (data_uu.len() / 4) * 4;
  let rem = data_uu.len() % 4;
  let out_len = data_uu.len() / 4 * 3 + if rem == 3 { 2 } else if rem == 2 { 1 } else { 0 };
  let mut out = Vec::with_capacity(out_len);

  let mut i = 0;
  while i < full {
    decode_chars(&data_uu[i..i + 4], &mut out, 4);
    i += 4;
  }
  decode_chars(&data_uu[full..], &mut out, rem);
  out
}

fn decode_chars(input: &[u8], out: &mut Vec<u8>, bytes_in: usize) {
  if bytes_in == 0 {
    return;
  }
  let bytes_out = if bytes_in == 4 {
    3
  } else if bytes_in == 3 {
    2
  } else {
    1
  };
  let mut value: u32 = 0;
  for idx in 0..bytes_in {
    value |= (u32::from(input[idx]).wrapping_sub(33)) << (6 * (3 - idx));
  }
  for idx in 0..bytes_out {
    out.push(((value >> ((2 - idx) * 8)) & 0xff) as u8);
  }
}

/// Port of the MIME guess that mkvtoolnix performs for SSA/ASS embedded
/// attachments (`add_attachment_maybe` → `mtx::mime::guess_type_for_data` →
/// `get_font_mime_type_to_use(..., current)`).
///
/// Upstream delegates content sniffing to Qt's `QMimeDatabase`, which uses the
/// freedesktop shared-mime-info magic database.  We cannot link Qt in this
/// header-only port, so we reimplement the byte-signature subset that the
/// font/graphics types actually rely on and apply the same legacy→current font
/// MIME remapping mkvmerge uses by default (the `--engage
/// use_legacy_font_mime_types` opt-in keeps the legacy names; default is
/// current).  Unknown payloads fall back to `application/octet-stream`, which
/// is what `QMimeDatabase` returns for unrecognised data.
pub fn guess_font_mime_type(data: &[u8]) -> String {
  // Fonts (current freedesktop names).
  if data.len() >= 4 {
    match &data[0..4] {
      // OpenType with CFF outlines.
      b"OTTO" => return "font/otf".to_string(),
      // TrueType collection.
      b"ttcf" => return "font/collection".to_string(),
      // WOFF / WOFF2.
      b"wOFF" => return "font/woff".to_string(),
      b"wOF2" => return "font/woff2".to_string(),
      // Legacy PostScript Type 1 sfnt and the bare `true`/`typ1` TrueType tags.
      b"true" | b"typ1" => return "font/sfnt".to_string(),
      // TrueType outlines: sfnt version 0x00010000.
      [0x00, 0x01, 0x00, 0x00] => return "font/sfnt".to_string(),
      _ => {}
    }
  }

  // Common raster image types embedded under `[Graphics]`.
  if data.len() >= 8 && data[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
    return "image/png".to_string();
  }
  if data.len() >= 3 && data[0..3] == [0xFF, 0xD8, 0xFF] {
    return "image/jpeg".to_string();
  }
  if data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
    return "image/gif".to_string();
  }
  if data.len() >= 2 && &data[0..2] == b"BM" {
    return "image/bmp".to_string();
  }

  "application/octet-stream".to_string()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SsaReader;

impl Reader for SsaReader {
  fn name(&self) -> &'static str {
    "ssa"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read > 0 && classify(&encoding::decode_lossy(&buf[..read])).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    // PARSER-153: read the whole file (bounded) so the global header and any
    // embedded `[Fonts]` past the 16 KiB probe window are parsed in full.
    let cap = src
      .length()
      .map(|l| l as usize)
      .unwrap_or(MAX_PARSE_BYTES)
      .min(MAX_PARSE_BYTES)
      .max(PROBE_BYTES);
    let mut buf = vec![0u8; cap];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    buf.truncate(read);
    let detected = encoding::detect(&buf);
    let text = encoding::decode_lossy(&buf);
    let variant = classify(&text).ok_or(ParseError::Unrecognised)?;
    let parsed = parse_ssa(&text);
    let private = parsed.global;

    let (codec_id, codec_name, variant_label, format) = match variant {
      SsaVariant::Ass => ("S_TEXT/ASS", "ASS subtitles", "ASS", ContainerFormat::SsaAss),
      SsaVariant::Ssa => ("S_TEXT/SSA", "SSA subtitles", "SSA", ContainerFormat::SsaAss),
    };
    out.container.format = format;
    out.container.recognized = true;
    out.container.supported = true;
    out.attachments.extend(parsed.attachments);

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: codec_id.to_string(),
        name: Some(codec_name.to_string()),
        codec_private: private
          .as_deref()
          .map(|header| CodecPrivate::from_bytes(header.as_bytes())),
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: true,
          encoding: Some(detected.label.to_string()),
          variant: Some(variant_label.to_string()),
          teletext_page: None,
        }),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn classify_ass_via_styles_section() {
    let text = "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n";
    assert_eq!(classify(text), Some(SsaVariant::Ass));
  }

  #[test]
  fn classify_ssa_via_styles_section() {
    let text = "[Script Info]\n\n[V4 Styles]\n";
    assert_eq!(classify(text), Some(SsaVariant::Ssa));
  }

  #[test]
  fn classify_ass_via_script_type() {
    let text = "[Script Info]\nScriptType: v4.00+\n";
    assert_eq!(classify(text), Some(SsaVariant::Ass));
  }

  #[test]
  fn classify_ssa_via_script_type() {
    let text = "[Script Info]\nScriptType: v4.00\n";
    assert_eq!(classify(text), Some(SsaVariant::Ssa));
  }

  #[test]
  fn classify_returns_none_without_script_info() {
    assert!(classify("[Events]\n").is_none());
  }

  #[test]
  fn classify_falls_back_to_ass_when_only_script_info_seen() {
    assert_eq!(classify("[Script Info]\n"), Some(SsaVariant::Ass));
  }

  #[test]
  fn probe_accepts_ass_blob() {
    let blob = b"[Script Info]\nScriptType: v4.00+\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(SsaReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_ass_track() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\n[Events]\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.ass", 0);
    SsaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "S_TEXT/ASS");
    assert!(out.tracks[0].codec.codec_private.is_some());
  }

  #[test]
  fn read_headers_emits_ssa_track() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"[Script Info]\n[V4 Styles]\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.ssa", 0);
    SsaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "S_TEXT/SSA");
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
    assert!(!SsaReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-207: SSA UU-encode helper for round-trip tests -----------

  /// Encode `data` with the SSA UUencode-like scheme that `decode_uu` reverses
  /// (3 input bytes → 4 chars; partial 2/1-byte tails → 3/2 chars; each 6-bit
  /// group offset by +33).  Mirrors mkvtoolnix's `ssa_packetizer_c` encoder.
  fn uu_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
      let chunk = &data[i..(i + 3).min(data.len())];
      let bytes_in = chunk.len();
      let mut value: u32 = 0;
      for (idx, b) in chunk.iter().enumerate() {
        value |= u32::from(*b) << ((2 - idx) * 8);
      }
      let chars_out = if bytes_in == 3 {
        4
      } else if bytes_in == 2 {
        3
      } else {
        2
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
  fn uu_decode_round_trips_arbitrary_bytes() {
    for sample in [
      vec![0x00, 0x01, 0x00, 0x00],
      vec![0xDE, 0xAD, 0xBE, 0xEF, 0x10],
      vec![0x41, 0x42],
      vec![0x99],
      (0u8..=255).collect::<Vec<u8>>(),
    ] {
      let encoded = uu_encode(&sample);
      assert_eq!(decode_uu(encoded.as_bytes()), sample, "round-trip failed for {sample:?}");
      // Decoded size matches the upstream length derivation.
      assert_eq!(decode_uu(encoded.as_bytes()).len(), sample.len());
    }
  }

  #[test]
  fn guess_mime_recognises_fonts_and_images() {
    assert_eq!(guess_font_mime_type(&[0x00, 0x01, 0x00, 0x00, 0x00]), "font/sfnt");
    assert_eq!(guess_font_mime_type(b"OTTO\x00\x00"), "font/otf");
    assert_eq!(guess_font_mime_type(b"ttcf\x00\x00"), "font/collection");
    assert_eq!(guess_font_mime_type(b"wOFF...."), "font/woff");
    assert_eq!(guess_font_mime_type(b"wOF2...."), "font/woff2");
    assert_eq!(guess_font_mime_type(b"true...."), "font/sfnt");
    assert_eq!(
      guess_font_mime_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00]),
      "image/png"
    );
    assert_eq!(guess_font_mime_type(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
    assert_eq!(guess_font_mime_type(b"GIF89a "), "image/gif");
    assert_eq!(guess_font_mime_type(b"BM......"), "image/bmp");
    assert_eq!(guess_font_mime_type(&[0x12, 0x34, 0x56]), "application/octet-stream");
  }

  #[test]
  fn parse_excludes_fonts_section_from_global_header() {
    // A `[Fonts]` block that appears before `[Events]` must not leak into the
    // codec-private global header (the old slice-before-`[Events]` approach did).
    let font_bytes = [0x00u8, 0x01, 0x00, 0x00, 0xDE, 0xAD];
    let uu = uu_encode(&font_bytes);
    let text = format!(
      "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name\n\n[Fonts]\nfontname: Test.ttf\n{uu}\n\n[Events]\nFormat: Layer, Start, End\n"
    );
    let parsed = parse_ssa(&text);
    let global = parsed.global.unwrap();
    // The fonts section header and the UU payload are absent from the header.
    assert!(!global.contains("[Fonts]"), "global must exclude [Fonts] header");
    assert!(!global.contains("fontname:"), "global must exclude fontname lines");
    assert!(!global.contains(&uu), "global must exclude UU payload");
    // But the script-info / styles content survives.
    assert!(global.contains("[Script Info]"));
    assert!(global.contains("[V4+ Styles]"));
    // CRLF line endings, matching upstream `m_global += "\r\n"`.
    assert!(global.contains("\r\n"));
    // The font attachment is harvested with the decoded size + guessed MIME.
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].file_name, "Test.ttf");
    assert_eq!(parsed.attachments[0].size, font_bytes.len() as u64);
    assert_eq!(parsed.attachments[0].mime_type.as_deref(), Some("font/sfnt"));
  }

  #[test]
  fn parse_collects_graphics_attachment() {
    // A `[Graphics]` PNG attachment is parsed (not just `[Fonts]`).
    let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x42, 0x42];
    let uu = uu_encode(&png);
    let text = format!(
      "[Script Info]\nScriptType: v4.00+\n\n[Graphics]\nfilename: logo.png\nfontname: logo.png\n{uu}\n\n[Events]\n"
    );
    let parsed = parse_ssa(&text);
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].file_name, "logo.png");
    assert_eq!(parsed.attachments[0].size, png.len() as u64);
    assert_eq!(parsed.attachments[0].mime_type.as_deref(), Some("image/png"));
    assert_eq!(parsed.attachments[0].description.as_deref(), Some("SSA/ASS embedded picture"));
    // Graphics content must not leak into the header either.
    let global = parsed.global.unwrap();
    assert!(!global.contains("[Graphics]"));
    assert!(!global.contains(&uu));
  }

  #[test]
  fn parse_collects_multiple_fonts() {
    let f1 = [0x4Fu8, 0x54, 0x54, 0x4F, 0x01]; // OTTO...
    let f2 = [0x00u8, 0x01, 0x00, 0x00, 0x02]; // TrueType sfnt
    let text = format!(
      "[Script Info]\n\n[Fonts]\nfontname: a.otf\n{}\nfontname: b.ttf\n{}\n\n[Events]\n",
      uu_encode(&f1),
      uu_encode(&f2)
    );
    let parsed = parse_ssa(&text);
    assert_eq!(parsed.attachments.len(), 2);
    assert_eq!(parsed.attachments[0].file_name, "a.otf");
    assert_eq!(parsed.attachments[0].mime_type.as_deref(), Some("font/otf"));
    assert_eq!(parsed.attachments[0].id, 1);
    assert_eq!(parsed.attachments[1].file_name, "b.ttf");
    assert_eq!(parsed.attachments[1].mime_type.as_deref(), Some("font/sfnt"));
    assert_eq!(parsed.attachments[1].id, 2);
  }

  #[test]
  fn parse_ignores_empty_or_nameless_font_blocks() {
    // A `[Fonts]` section with payload but no `fontname:` produces no attachment;
    // a `fontname:` with no payload likewise produces nothing.
    let text = "[Script Info]\n\n[Fonts]\nSGVsbG8=\nfontname: empty.ttf\n\n[Events]\n";
    let parsed = parse_ssa(text);
    assert!(parsed.attachments.is_empty());
  }

  // ---- PARSER-153: full-file header / font parsing (past 16 KiB) --------

  fn build_large_ass_with_late_fonts() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n");
    // Pad the styles section past the 16 KiB probe window.
    while s.len() < PROBE_BYTES + 4096 {
      s.push_str("Style: Filler,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H80000000\n");
    }
    s.push_str("\n[Fonts]\nfontname: Late.ttf\nQUJDRA==\nRUZHSA==\n\n[Events]\nFormat: Layer, Start, End\n");
    s.into_bytes()
  }

  #[test]
  fn read_headers_recovers_fonts_past_probe_window() {
    use crate::media_metadata::deadline::Deadline;
    let blob = build_large_ass_with_late_fonts();
    assert!(blob.len() > PROBE_BYTES, "fixture must exceed the probe window");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ass", 0);
    SsaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    // The [Fonts] block lives past 16 KiB, so the old probe-window-only parse
    // missed it; the full-file parse now recovers it.
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].file_name, "Late.ttf");
    // The codec private (global header) includes the full styles section but
    // excludes the trailing [Fonts] block.
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    assert!(private.length as usize > PROBE_BYTES);
  }
}
