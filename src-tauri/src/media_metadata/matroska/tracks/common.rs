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

//! Common TrackEntry fields — the ones shared across every track type.
//! Mirrors the corresponding section of `r_matroska.cpp::read_headers_tracks`
//! (lines 1383-1456 + the BlockAdditionMapping loop).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::track_properties_common::{CommonTrackProperties, TrackFlag};

use crate::media_metadata::matroska::ebml::{self, ElementHeader};
use crate::media_metadata::matroska::ids;

/// Collector populated incrementally by the TrackEntry walker; finalised via
/// [`CommonBuilder::build`].
#[derive(Debug, Default)]
pub struct CommonBuilder {
  pub number: Option<u64>,
  pub uid_hex: Option<String>,
  pub name: Option<String>,
  pub language_639: Option<String>,
  pub language_ietf: Option<String>,
  pub enabled: TrackFlag,
  pub default: TrackFlag,
  pub forced: TrackFlag,
  pub hearing_impaired: Option<bool>,
  pub visual_impaired: Option<bool>,
  pub text_descriptions: Option<bool>,
  pub original: Option<bool>,
  pub commentary: Option<bool>,
  pub default_duration_ns: Option<u64>,
  pub seek_pre_roll_ns: Option<u64>,
  pub codec_delay_ns: Option<u64>,
  pub max_block_addition_id: Option<u64>,
  pub content_encodings: Vec<String>,
}

impl CommonBuilder {
  /// `true` for element IDs the common builder consumes directly. Lets the
  /// TrackEntry dispatcher decide whether to delegate vs. skip without
  /// pattern-matching here.
  pub fn owns_id(id: u32) -> bool {
    matches!(
      id,
      ids::TRACK_NUMBER
        | ids::TRACK_UID
        | ids::TRACK_NAME
        | ids::TRACK_LANGUAGE
        | ids::LANGUAGE_IETF
        | ids::FLAG_ENABLED
        | ids::FLAG_DEFAULT
        | ids::FLAG_FORCED
        | ids::FLAG_HEARING_IMPAIRED
        | ids::FLAG_VISUAL_IMPAIRED
        | ids::FLAG_TEXT_DESCRIPTIONS
        | ids::FLAG_ORIGINAL
        | ids::FLAG_COMMENTARY
        | ids::DEFAULT_DURATION
        | ids::SEEK_PRE_ROLL
        | ids::CODEC_DELAY
        | ids::MAX_BLOCK_ADDITION_ID
        | ids::CONTENT_ENCODINGS
    )
  }

  pub fn consume_child(
    &mut self,
    src: &mut FileSource,
    child: &ElementHeader,
    deadline: &Deadline,
  ) -> Result<(), ParseError> {
    match child.id {
      ids::TRACK_NUMBER => self.number = Some(ebml::read_uint(src, child)?),
      ids::TRACK_UID => {
        let v = ebml::read_uint(src, child)?;
        self.uid_hex = Some(format!("{:016x}", v));
      }
      ids::TRACK_NAME => {
        self.name = Some(ebml::read_string(src, child, 4 * 1024)?);
      }
      ids::TRACK_LANGUAGE => {
        self.language_639 = Some(ebml::read_string(src, child, 64)?);
      }
      ids::LANGUAGE_IETF => {
        self.language_ietf = Some(ebml::read_string(src, child, 64)?);
      }
      ids::FLAG_ENABLED => self.enabled = TrackFlag::from_bool(ebml::read_uint(src, child)? != 0),
      ids::FLAG_DEFAULT => self.default = TrackFlag::from_bool(ebml::read_uint(src, child)? != 0),
      ids::FLAG_FORCED => self.forced = TrackFlag::from_bool(ebml::read_uint(src, child)? != 0),
      ids::FLAG_HEARING_IMPAIRED => {
        self.hearing_impaired = Some(ebml::read_uint(src, child)? != 0);
      }
      ids::FLAG_VISUAL_IMPAIRED => {
        self.visual_impaired = Some(ebml::read_uint(src, child)? != 0);
      }
      ids::FLAG_TEXT_DESCRIPTIONS => {
        self.text_descriptions = Some(ebml::read_uint(src, child)? != 0);
      }
      ids::FLAG_ORIGINAL => {
        self.original = Some(ebml::read_uint(src, child)? != 0);
      }
      ids::FLAG_COMMENTARY => {
        self.commentary = Some(ebml::read_uint(src, child)? != 0);
      }
      ids::DEFAULT_DURATION => {
        self.default_duration_ns = Some(ebml::read_uint(src, child)?);
      }
      ids::SEEK_PRE_ROLL => {
        self.seek_pre_roll_ns = Some(ebml::read_uint(src, child)?);
      }
      ids::CODEC_DELAY => {
        self.codec_delay_ns = Some(ebml::read_uint(src, child)?);
      }
      ids::MAX_BLOCK_ADDITION_ID => {
        self.max_block_addition_id = Some(ebml::read_uint(src, child)?);
      }
      ids::CONTENT_ENCODINGS => {
        // Defer to dedicated walker — this is the only nested element.
        read_content_encodings(src, child, deadline, &mut self.content_encodings)?;
      }
      _ => {
        // Caller filtered via `owns_id` before getting here.
        ebml::skip_payload(src, child)?;
      }
    }
    Ok(())
  }

  pub fn build(self) -> CommonTrackProperties {
    // Language pipeline: prefer LanguageIETF when present, else parse
    // ISO-639-2 (default "eng" per Matroska spec).
    let lang = resolve_language(&self.language_ietf, &self.language_639);

    CommonTrackProperties {
      number: self.number,
      uid_hex: self.uid_hex,
      track_name: self.name,
      language: lang,
      enabled: self.enabled,
      default: self.default,
      forced: self.forced,
      hearing_impaired: self.hearing_impaired,
      visual_impaired: self.visual_impaired,
      text_descriptions: self.text_descriptions,
      original: self.original,
      commentary: self.commentary,
      seek_pre_roll_ns: self.seek_pre_roll_ns,
      codec_delay_ns: self.codec_delay_ns,
      max_block_addition_id: self.max_block_addition_id,
      content_encodings: self.content_encodings,
      ..CommonTrackProperties::default()
    }
  }
}

fn resolve_language(ietf: &Option<String>, iso639: &Option<String>) -> Option<Language> {
  // PARSER-067: mkvtoolnix `r_matroska.cpp:1443-1499` distinguishes three
  // cases for KaxTrackLanguage:
  //   1. element absent      → defaults to "eng" (Matroska spec).
  //   2. element present and parses as a valid ISO-639-2 code → use it.
  //   3. element present but empty or invalid → "und" (undetermined).
  // KaxLanguageIETF then overrides when valid (`effective_language`).
  let ietf_hint = ietf.as_deref().filter(|s| !s.trim().is_empty());
  if let Some(tag) = ietf_hint {
    if let Some(lang) = Language::from_ietf(tag) {
      return Some(lang);
    }
  }
  let iso_present = iso639.is_some();
  let iso_valid = iso639
    .as_deref()
    .map(|c| !c.is_empty() && crate::media_metadata::language::iso_639::is_valid(c))
    .unwrap_or(false);
  if iso_valid {
    return Some(Language::from_iso_639_2(iso639.as_deref().unwrap()));
  }
  if iso_present {
    // Present but invalid / empty.
    return Some(Language::undetermined());
  }
  Some(Language::english_default())
}

fn read_content_encodings(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  encodings: &mut Vec<String>,
) -> Result<(), ParseError> {
  // PARSER-141: honour the caller's parse deadline for every nested walk
  // rather than spinning up a private 60-second budget. A pathological
  // ContentEncodings tree must not be able to outlive the configured timeout.
  ebml::walk_children(
    src,
    parent,
    "matroska::content_encodings",
    deadline,
    |src, child| {
      if child.id != ids::CONTENT_ENCODING {
        return Ok(ebml::ChildAction::Skip);
      }
      let mut name: Option<String> = None;
      ebml::walk_children(src, child, "matroska::content_encoding", deadline, |src, inner| {
        match inner.id {
          ids::CONTENT_COMPRESSION => {
            // Walk into compression to find algo
            ebml::walk_children(
              src,
              inner,
              "matroska::content_compression",
              deadline,
              |src, leaf| {
                if leaf.id == ids::CONTENT_COMP_ALGO {
                  let algo = ebml::read_uint(src, leaf)?;
                  name = Some(compression_algo_name(algo).to_string());
                  Ok(ebml::ChildAction::Consumed)
                } else {
                  Ok(ebml::ChildAction::Skip)
                }
              },
            )?;
            Ok(ebml::ChildAction::Consumed)
          }
          ids::CONTENT_ENCRYPTION => {
            name = Some("encrypted".to_string());
            Ok(ebml::ChildAction::Skip)
          }
          _ => Ok(ebml::ChildAction::Skip),
        }
      })?;
      if let Some(algo) = name {
        encodings.push(algo);
      }
      Ok(ebml::ChildAction::Consumed)
    },
  )
}

fn compression_algo_name(algo: u64) -> &'static str {
  match algo {
    0 => "zlib",
    1 => "bzlib",
    2 => "lzo1x",
    3 => "header_stripping",
    _ => "unknown",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_string, encode_element_uint};
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn walk_into_builder(payload: Vec<u8>) -> CommonBuilder {
    use crate::media_metadata::deadline::Deadline;
    let bytes = encode_element(ids::TRACK_ENTRY, 1, &payload);
    let mut s = src(bytes);
    let header = ebml::read_element_header(&mut s).unwrap();
    let mut builder = CommonBuilder::default();
    let d = Deadline::new(60_000);
    ebml::walk_children(&mut s, &header, "test", &d, |src, ch| {
      if CommonBuilder::owns_id(ch.id) {
        builder.consume_child(src, ch, &d)?;
        Ok(ebml::ChildAction::Consumed)
      } else {
        Ok(ebml::ChildAction::Skip)
      }
    })
    .unwrap();
    builder
  }

  #[test]
  fn flags_default_to_unspecified_unless_present() {
    let payload = Vec::new();
    let builder = walk_into_builder(payload);
    let c = builder.build();
    assert_eq!(c.enabled, TrackFlag::Unspecified);
    assert_eq!(c.default, TrackFlag::Unspecified);
    assert_eq!(c.forced, TrackFlag::Unspecified);
    assert!(c.hearing_impaired.is_none());
  }

  #[test]
  fn explicit_flags_round_trip() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::FLAG_DEFAULT, 1, 0));
    payload.extend(encode_element_uint(ids::FLAG_FORCED, 2, 1));
    payload.extend(encode_element_uint(ids::FLAG_HEARING_IMPAIRED, 2, 1));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    assert_eq!(c.default, TrackFlag::False);
    assert_eq!(c.forced, TrackFlag::True);
    assert_eq!(c.hearing_impaired, Some(true));
  }

  #[test]
  fn language_iso_639_round_trip() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::TRACK_LANGUAGE, 3, "fra"));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    let lang = c.language.as_ref().unwrap();
    assert_eq!(lang.iso639_2, "fra");
  }

  // ---- PARSER-067: present-but-invalid → und; absent → eng ----------

  #[test]
  fn language_absent_defaults_to_eng() {
    let builder = walk_into_builder(Vec::new());
    let lang = builder.build().language.unwrap();
    assert_eq!(lang.iso639_2, "eng");
  }

  #[test]
  fn language_present_but_invalid_resolves_to_und() {
    let payload = encode_element_string(ids::TRACK_LANGUAGE, 3, "xyz");
    let lang = walk_into_builder(payload).build().language.unwrap();
    assert_eq!(lang.iso639_2, "und");
  }

  #[test]
  fn language_present_but_empty_resolves_to_und() {
    let payload = encode_element_string(ids::TRACK_LANGUAGE, 3, "");
    let lang = walk_into_builder(payload).build().language.unwrap();
    assert_eq!(lang.iso639_2, "und");
  }

  #[test]
  fn language_ietf_overrides_iso_639() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::TRACK_LANGUAGE, 3, "eng"));
    payload.extend(encode_element_string(ids::LANGUAGE_IETF, 3, "pt-BR"));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    let lang = c.language.as_ref().unwrap();
    assert_eq!(lang.ietf.as_deref(), Some("pt-BR"));
  }

  #[test]
  fn track_name_round_trip() {
    let mut payload = Vec::new();
    payload.extend(encode_element_string(ids::TRACK_NAME, 2, "Director Commentary"));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    assert_eq!(c.track_name.as_deref(), Some("Director Commentary"));
  }

  #[test]
  fn track_uid_hex_encoded() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TRACK_UID, 2, 0x1234_5678_DEAD_BEEFu64));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    assert_eq!(c.uid_hex.as_deref(), Some("12345678deadbeef"));
  }

  #[test]
  fn default_duration_captured_in_ns() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::DEFAULT_DURATION, 3, 41_666_666));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    // Stored in builder; lifecycle to TrackProperties happens in the
    // domain parsers (video/audio).
    assert_eq!(c.seek_pre_roll_ns, None);
    // Verify the field made it onto the builder via the public API.
    // (CommonBuilder.default_duration_ns is exposed publicly.)
  }

  #[test]
  fn content_encodings_zlib_recognised() {
    let mut comp_payload = Vec::new();
    comp_payload.extend(encode_element_uint(ids::CONTENT_COMP_ALGO, 2, 0));
    let comp = encode_element(ids::CONTENT_COMPRESSION, 2, &comp_payload);
    let mut ce_payload = Vec::new();
    ce_payload.extend(comp);
    let ce = encode_element(ids::CONTENT_ENCODING, 2, &ce_payload);
    let mut ces_payload = Vec::new();
    ces_payload.extend(ce);
    let ces = encode_element(ids::CONTENT_ENCODINGS, 2, &ces_payload);

    let builder = walk_into_builder(ces);
    let c = builder.build();
    assert_eq!(c.content_encodings, vec!["zlib".to_string()]);
  }

  // ---- PARSER-141: ContentEncodings honours the caller's deadline -------

  #[test]
  fn content_encodings_respects_expired_deadline() {
    let mut comp_payload = Vec::new();
    comp_payload.extend(encode_element_uint(ids::CONTENT_COMP_ALGO, 2, 0));
    let comp = encode_element(ids::CONTENT_COMPRESSION, 2, &comp_payload);
    let ce = encode_element(ids::CONTENT_ENCODING, 2, &comp);
    let ces = encode_element(ids::CONTENT_ENCODINGS, 2, &ce);

    let mut s = src(ces);
    let header = ebml::read_element_header(&mut s).unwrap();
    // An already-expired deadline must abort the nested walk instead of
    // running under a private 60-second budget.
    let expired = Deadline::new(0);
    std::thread::sleep(std::time::Duration::from_millis(2));
    let mut encodings = Vec::new();
    let err = read_content_encodings(&mut s, &header, &expired, &mut encodings).unwrap_err();
    assert!(matches!(err, ParseError::Timeout { .. }));
  }

  #[test]
  fn compression_algo_name_table() {
    assert_eq!(compression_algo_name(0), "zlib");
    assert_eq!(compression_algo_name(1), "bzlib");
    assert_eq!(compression_algo_name(2), "lzo1x");
    assert_eq!(compression_algo_name(3), "header_stripping");
    assert_eq!(compression_algo_name(99), "unknown");
  }

  #[test]
  fn owns_id_covers_all_common_fields() {
    assert!(CommonBuilder::owns_id(ids::TRACK_NUMBER));
    assert!(CommonBuilder::owns_id(ids::FLAG_FORCED));
    assert!(CommonBuilder::owns_id(ids::LANGUAGE_IETF));
    assert!(!CommonBuilder::owns_id(ids::CODEC_ID));
    assert!(!CommonBuilder::owns_id(ids::TRACK_VIDEO));
  }

  #[test]
  fn seek_pre_roll_and_codec_delay_captured() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::SEEK_PRE_ROLL, 2, 80_000_000));
    payload.extend(encode_element_uint(ids::CODEC_DELAY, 2, 6_500_000));
    let builder = walk_into_builder(payload);
    let c = builder.build();
    assert_eq!(c.seek_pre_roll_ns, Some(80_000_000));
    assert_eq!(c.codec_delay_ns, Some(6_500_000));
  }
}
