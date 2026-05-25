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

pub fn global_header(text: &str) -> Option<String> {
  let lower = text.to_ascii_lowercase();
  let end = lower.find("[events]").unwrap_or(text.len());
  let header = text[..end].trim_end();
  if header.is_empty() {
    None
  } else {
    Some(format!("{header}\n"))
  }
}

pub fn font_attachments(text: &str) -> Vec<Attachment> {
  let mut attachments = Vec::new();
  let mut in_fonts = false;
  let mut current_name: Option<String> = None;
  let mut current_size = 0u64;
  for line in text.lines() {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with('[') && lower.ends_with(']') {
      if in_fonts && lower != "[fonts]" {
        flush_font(&mut attachments, &mut current_name, &mut current_size);
        in_fonts = false;
      } else if lower == "[fonts]" {
        in_fonts = true;
      }
      continue;
    }
    if !in_fonts {
      continue;
    }
    if lower.starts_with("fontname:") {
      flush_font(&mut attachments, &mut current_name, &mut current_size);
      let rest = trimmed.split_once(':').map(|(_, r)| r).unwrap_or("");
      let name = rest.trim();
      if !name.is_empty() {
        current_name = Some(name.to_string());
      }
    } else if current_name.is_some() && !trimmed.is_empty() {
      current_size += trimmed.len() as u64;
    }
  }
  flush_font(&mut attachments, &mut current_name, &mut current_size);
  attachments
}

fn flush_font(attachments: &mut Vec<Attachment>, name: &mut Option<String>, size: &mut u64) {
  if let Some(file_name) = name.take() {
    attachments.push(Attachment {
      id: attachments.len() as u32 + 1,
      file_name,
      mime_type: Some("application/x-truetype-font".to_string()),
      description: Some("SSA/ASS embedded font".to_string()),
      size: *size,
      uid_hex: None,
    });
    *size = 0;
  }
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
    let private = global_header(&text);

    let (codec_id, codec_name, variant_label, format) = match variant {
      SsaVariant::Ass => ("S_TEXT/ASS", "ASS subtitles", "ASS", ContainerFormat::SsaAss),
      SsaVariant::Ssa => ("S_TEXT/SSA", "SSA subtitles", "SSA", ContainerFormat::SsaAss),
    };
    out.container.format = format;
    out.container.recognized = true;
    out.container.supported = true;
    out.attachments.extend(font_attachments(&text));

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

  #[test]
  fn font_attachments_collect_embedded_fonts() {
    let text = "[Script Info]\nScriptType: v4.00+\n[Fonts]\nfontname: Test.ttf\nAAAA\nBBBB\n[Events]\n";
    let attachments = font_attachments(text);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].file_name, "Test.ttf");
    assert_eq!(attachments[0].size, 8);
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
    // The codec private (global header) includes the full styles section.
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    assert!(private.length as usize > PROBE_BYTES);
  }
}
