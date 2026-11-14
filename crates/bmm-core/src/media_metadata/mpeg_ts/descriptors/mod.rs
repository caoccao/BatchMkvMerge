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

//! MPEG-TS descriptor dispatcher.  Walks a concatenated descriptor list
//! (`tag (u8) | length (u8) | length bytes of body`) and routes each entry
//! to the matching decoder sub-module.

pub mod ac3;
pub mod dovi;
pub mod dts;
pub mod eac3;
pub mod iso_639_language;
pub mod registration;
pub mod service;
pub mod subtitling;
pub mod teletext;

pub const TAG_REGISTRATION: u8 = 0x05;
pub const TAG_ISO_639_LANGUAGE: u8 = 0x0A;
pub const TAG_TELETEXT: u8 = 0x56;
pub const TAG_SUBTITLING: u8 = 0x59;
pub const TAG_AC3: u8 = 0x6A;
pub const TAG_DTS: u8 = 0x7B;
pub const TAG_EAC3: u8 = 0x7A;
pub const TAG_SERVICE: u8 = 0x48;
/// HEVC video descriptor.  PARSER-251: mkvtoolnix does not handle this tag in
/// its PMT descriptor switch, so it never promotes a stream's codec — kept only
/// so the `missing_tag` bookkeeping (`has_disambiguating_tag`) can observe it.
pub const TAG_HEVC: u8 = 0x38;
pub const TAG_DOVI: u8 = 0xB0;

/// Aggregated descriptor information.  Each descriptor we recognise sets a
/// field; unknown descriptors are silently skipped.
#[derive(Debug, Default, Clone)]
pub struct DescriptorSummary {
  pub language_iso_639_2: Option<String>,
  /// First teletext page across all entries — kept for back-compat with
  /// pre-multi-page callers; full table is in [`teletext_entries`].
  pub teletext_page: Option<u32>,
  /// Every teletext entry decoded from a 0x56 descriptor (PARSER-092).
  pub teletext_entries: Vec<teletext::TeletextEntry>,
  pub is_ac3: bool,
  pub is_eac3: bool,
  pub is_dts: bool,
  /// PARSER-173: Dolby Vision descriptor (profile + optional base-layer PID).
  pub dovi: Option<dovi::DoviDescriptor>,
  pub service_name: Option<String>,
  /// DVB subtitling descriptor (0x59) entries — PARSER-091.
  pub subtitling_entries: Vec<subtitling::SubtitlingEntry>,
  /// Registration descriptor (0x05) information — PARSER-090.
  pub registration: Option<registration::RegistrationDescriptor>,
  /// `true` once any tag other than ISO-639 language (0x0A) has been
  /// observed.  Mirrors mkvtoolnix's `missing_tag` flag used to decide
  /// whether stream_type 0x06 should default to AC-3 (PARSER-093).
  pub has_disambiguating_tag: bool,
}

/// Walk a descriptor list and accumulate findings.
pub fn walk(descriptors: &[u8]) -> DescriptorSummary {
  let mut summary = DescriptorSummary::default();
  let mut pos = 0usize;
  while pos + 2 <= descriptors.len() {
    let tag = descriptors[pos];
    let len = descriptors[pos + 1] as usize;
    let body_start = pos + 2;
    let body_end = body_start + len;
    if body_end > descriptors.len() {
      break;
    }
    let body = &descriptors[body_start..body_end];
    // PARSER-093: mkvtoolnix sets `missing_tag = false` for any tag
    // other than 0x0A (ISO-639 language).  Track the same state so the
    // stream-table can decide whether stream_type 0x06 should default
    // to AC-3.
    if tag != TAG_ISO_639_LANGUAGE {
      summary.has_disambiguating_tag = true;
    }
    match tag {
      TAG_REGISTRATION => {
        if let Some(r) = registration::decode(body) {
          summary.registration = Some(r);
        }
      }
      TAG_ISO_639_LANGUAGE => {
        if let Some(lang) = iso_639_language::decode(body) {
          summary.language_iso_639_2 = Some(lang);
        }
      }
      TAG_TELETEXT => {
        summary.teletext_entries = teletext::decode_all(body);
        if let Some(first) = summary.teletext_entries.first() {
          summary.teletext_page = Some(first.page);
        }
      }
      TAG_SUBTITLING => {
        summary.subtitling_entries = subtitling::decode_all(body);
      }
      TAG_AC3 => {
        summary.is_ac3 = ac3::decode(body);
      }
      TAG_EAC3 => {
        summary.is_eac3 = eac3::decode(body);
      }
      TAG_DTS => {
        summary.is_dts = dts::decode(body);
      }
      TAG_DOVI => {
        summary.dovi = dovi::decode(body);
      }
      TAG_SERVICE => {
        if let Some(name) = service::decode(body) {
          summary.service_name = Some(name);
        }
      }
      _ => {}
    }
    pos = body_end;
  }
  summary
}

#[cfg(test)]
pub(crate) fn build_descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
  let mut v = Vec::with_capacity(2 + body.len());
  v.push(tag);
  v.push(body.len() as u8);
  v.extend_from_slice(body);
  v
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn walk_collects_known_tags() {
    let mut descriptors = Vec::new();
    descriptors.extend(build_descriptor(TAG_ISO_639_LANGUAGE, b"eng\x00"));
    descriptors.extend(build_descriptor(TAG_AC3, &[]));
    let s = walk(&descriptors);
    assert_eq!(s.language_iso_639_2.as_deref(), Some("eng"));
    assert!(s.is_ac3);
  }

  #[test]
  fn walk_skips_unknown_tags() {
    let descriptors = build_descriptor(0x99, &[1, 2, 3, 4]);
    let s = walk(&descriptors);
    assert!(s.language_iso_639_2.is_none());
    assert!(!s.is_ac3);
  }

  #[test]
  fn walk_handles_empty_buffer() {
    let s = walk(&[]);
    assert!(s.language_iso_639_2.is_none());
  }

  #[test]
  fn walk_stops_on_truncated_descriptor() {
    // tag, length=5, only 2 bytes of body
    let descriptors = vec![TAG_ISO_639_LANGUAGE, 0x05, 0x01, 0x02];
    let s = walk(&descriptors);
    assert!(s.language_iso_639_2.is_none());
  }
}
