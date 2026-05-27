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

//! USF (Universal Subtitle Format) reader.
//!
//! USF is an XML subtitle format.  This reader is a pure-Rust port of
//! `mkvtoolnix/src/input/r_usf.cpp`.  Probing requires an `<?xml` or `<!--`
//! marker in the leading ~1000-character window (mirroring
//! `usf_reader_c::probe_file`) and then loads the document with a real XML
//! parser (quick-xml standing in for pugixml), validating that the document
//! element's fully-qualified name is exactly `USFSubtitles` (namespaced roots
//! such as `<usf:USFSubtitles>` are rejected, matching pugixml's
//! `document_element().name()`).
//!
//! Header reading mirrors the upstream three-step walk:
//!   * `parse_metadata` — default language from `<metadata><language code="">`.
//!   * `parse_subtitles` — one track per direct-child `<subtitles>` element,
//!     each with its own language from a child `<language code="">`.
//!   * `create_codec_private` — the whole document with every `<subtitles>`
//!     subtree removed, re-serialized as the shared per-track codec private.
//!
//! Default-language fallback for tracks lacking a valid language mirrors the
//! loop in `usf_reader_c::read_headers` (r_usf.cpp lines 64-65).

use quick_xml::Reader;
use quick_xml::Writer;
use quick_xml::events::Event;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader as MediaReader;

use super::encoding;

/// Upstream caps `mtx::xml::load_file` at 10 MiB during `probe_file`
/// (`r_usf.cpp` line 45), but `read_headers` reloads the full document.
const PROBE_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;

/// `usf_reader_c::probe_file` only accumulates leading lines until the buffer
/// reaches ~1000 characters before searching for the `<?xml` / `<!--` marker
/// (r_usf.cpp lines 35-42).  Native previously scanned the whole decoded
/// document (PARSER-236).
const MARKER_WINDOW_CHARS: usize = 1000;

/// Result of a successful USF document walk: the per-track metadata plus the
/// shared codec-private document (all `<subtitles>` subtrees removed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsfDocument {
  /// Default language code from `<metadata><language code="">`, if present.
  pub default_language: Option<String>,
  /// One entry per direct-child `<subtitles>` element; the optional per-track
  /// language code comes from its child `<language code="">`.
  pub tracks: Vec<Option<String>>,
  /// The whole document re-serialized with every `<subtitles>` subtree removed.
  pub codec_private: String,
}

/// Read the probe document bytes (capped at 10 MiB) and decode them to a UTF-8
/// string, BOM-stripped, for the XML parser.
fn read_probe_document(src: &mut FileSource, deadline: Option<&Deadline>) -> Result<(Vec<u8>, String), ParseError> {
  const CHUNK: usize = 64 * 1024;

  src.seek_to(0)?;
  let len = src
    .length()
    .map(|l| l.min(PROBE_DOCUMENT_BYTES as u64) as usize)
    .unwrap_or(PROBE_DOCUMENT_BYTES);
  let cap = len.min(PROBE_DOCUMENT_BYTES);
  let mut buf = Vec::with_capacity(cap);
  let mut remaining = cap;
  while remaining > 0 {
    if let Some(deadline) = deadline {
      deadline.check("usf::probe_document")?;
    }
    let wanted = remaining.min(CHUNK);
    let mut chunk = vec![0u8; wanted];
    let read = src.read_at_most(&mut chunk)?;
    if read == 0 {
      break;
    }
    buf.extend_from_slice(&chunk[..read]);
    remaining -= read;
    if read < wanted {
      break;
    }
  }
  let text = encoding::decode_lossy(&buf);
  Ok((buf, text))
}

/// Read the full USF document for `read_headers`.  Unlike `probe_file`,
/// mkvtoolnix does not apply the 10 MiB XML cap on this pass (`r_usf.cpp:53`).
fn read_header_document(src: &mut FileSource, deadline: &Deadline) -> Result<(Vec<u8>, String), ParseError> {
  let bytes = super::read_source_to_end(src, Some(deadline), "usf::read_document")?;
  let text = encoding::decode_lossy(&bytes);
  Ok((bytes, text))
}

/// Returns true when the leading ~1000-character window carries the `<?xml` or
/// `<!--` marker that `usf_reader_c::probe_file` (r_usf.cpp lines 35-43)
/// requires before it attempts to load the document.  Only the leading window
/// is inspected, matching upstream's `content.length() < 1000` accumulation
/// loop — a marker appearing further into the file does not qualify.
fn has_xml_marker(text: &str) -> bool {
  let window: String = text.chars().take(MARKER_WINDOW_CHARS).collect();
  window.contains("<?xml") || window.contains("<!--")
}

/// The fully-qualified (namespace-prefixed) name of an element, lossily
/// decoded.  pugixml's `document_element().name()` keeps the prefix, so the
/// root comparison must be against the qualified name, not the local one
/// (PARSER-236).
fn qualified_name(element: &quick_xml::events::BytesStart<'_>) -> String {
  String::from_utf8_lossy(element.name().as_ref()).into_owned()
}

/// Extract the `code` attribute value (non-empty) from a start/empty element.
fn code_attribute(element: &quick_xml::events::BytesStart<'_>) -> Option<String> {
  let attr = element.try_get_attribute(b"code").ok().flatten()?;
  let value = attr.unescape_value().ok()?;
  if value.is_empty() {
    None
  } else {
    Some(value.into_owned())
  }
}

/// The local (namespace-stripped) name of an element as a `String`, lossily
/// decoded — element names are ASCII in practice.
fn local_name(element: &quick_xml::events::BytesStart<'_>) -> String {
  String::from_utf8_lossy(element.local_name().as_ref()).into_owned()
}

/// Walk the XML document once: validate the root element, collect the default
/// language + per-track languages, and build the codec-private document with
/// all `<subtitles>` subtrees removed.  Mirrors `parse_metadata`,
/// `parse_subtitles`, and `create_codec_private` from r_usf.cpp.
fn parse_document(text: &str, deadline: &Deadline) -> Result<Option<UsfDocument>, ParseError> {
  let mut reader = Reader::from_str(text);
  reader.config_mut().expand_empty_elements = false;

  let mut writer = Writer::new(Vec::<u8>::new());
  let mut doc = UsfDocument::default();

  // Open-element stack of local names; `depth()` is `stack.len()`.
  let mut stack: Vec<String> = Vec::new();
  let mut root_seen = false;
  // Depth (1-based, root == 1) of the `<subtitles>` element we are currently
  // skipping for codec-private + metadata purposes; `None` when not skipping.
  let mut subtitles_skip_depth: Option<usize> = None;
  // True while the cursor is directly inside the current `<subtitles>` element
  // (one level below it), so a `<language>` start there sets the track's code.
  let mut current_track_index: Option<usize> = None;

  loop {
    deadline.check("usf::parse_document")?;
    let event = match reader.read_event() {
      Ok(event) => event,
      Err(_) => return Ok(None),
    };

    match event {
      Event::Eof => break,

      Event::Start(ref element) => {
        let name = local_name(element);
        let depth = stack.len() + 1; // depth this element occupies (root == 1)

        if !root_seen {
          root_seen = true;
          if qualified_name(element) != "USFSubtitles" {
            return Ok(None);
          }
        }

        // Direct-child `<subtitles>` of the root (root == 1, children == 2).
        if subtitles_skip_depth.is_none() && depth == 2 && name == "subtitles" {
          // New top-level track; drop the whole subtree from codec-private.
          subtitles_skip_depth = Some(depth);
          current_track_index = Some(doc.tracks.len());
          doc.tracks.push(None);
          stack.push(name);
          continue;
        }

        if let Some(skip_depth) = subtitles_skip_depth {
          // A direct child `<language>` of the `<subtitles>` element carries
          // the per-track language code (r_usf.cpp line 98).
          if depth == skip_depth + 1 && name == "language" {
            if let (Some(idx), Some(code)) = (current_track_index, code_attribute(element)) {
              doc.tracks[idx] = Some(code);
            }
          }
          stack.push(name);
          continue;
        }

        // Default language: `<metadata><language code="">` directly under root
        // (r_usf.cpp line 80).  The `<language>` sits at depth 3 with a
        // `<metadata>` parent.
        if depth == 3 && name == "language" && stack.last().map(String::as_str) == Some("metadata") {
          if let Some(code) = code_attribute(element) {
            doc.default_language = Some(code);
          }
        }

        // Outside any skipped subtree → keep the element in codec-private.
        writer
          .write_event(Event::Start(element.clone()))
          .map_err(write_error)?;
        stack.push(name);
      }

      Event::End(ref element) => {
        let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
        let closing_depth = stack.len();
        stack.pop();

        if let Some(skip_depth) = subtitles_skip_depth {
          if closing_depth == skip_depth && name == "subtitles" {
            subtitles_skip_depth = None;
            current_track_index = None;
          }
          // The whole subtree (including this end tag) is dropped.
          continue;
        }

        writer.write_event(Event::End(element.clone())).map_err(write_error)?;
      }

      Event::Empty(ref element) => {
        let name = local_name(element);
        let depth = stack.len() + 1;

        if !root_seen {
          root_seen = true;
          if qualified_name(element) != "USFSubtitles" {
            return Ok(None);
          }
        }

        if subtitles_skip_depth.is_none() && depth == 2 && name == "subtitles" {
          // Empty `<subtitles/>` element — a track with no children.
          doc.tracks.push(None);
          continue;
        }

        if let Some(skip_depth) = subtitles_skip_depth {
          if depth == skip_depth + 1 && name == "language" {
            if let (Some(idx), Some(code)) = (current_track_index, code_attribute(element)) {
              doc.tracks[idx] = Some(code);
            }
          }
          continue;
        }

        if depth == 3 && name == "language" && stack.last().map(String::as_str) == Some("metadata") {
          if let Some(code) = code_attribute(element) {
            doc.default_language = Some(code);
          }
        }

        writer
          .write_event(Event::Empty(element.clone()))
          .map_err(write_error)?;
      }

      other => {
        // Declarations, comments, processing instructions, text, CDATA: keep
        // them in codec-private only when not inside a skipped subtree.
        if subtitles_skip_depth.is_none() {
          writer.write_event(other).map_err(write_error)?;
        }
      }
    }
  }

  if !root_seen {
    return Ok(None);
  }
  // pugixml rejects documents with unbalanced elements; quick-xml only flags a
  // *mismatched* end tag, so an unclosed element that simply runs into EOF
  // leaves the open-element stack non-empty.  Treat that as malformed.
  if !stack.is_empty() {
    return Ok(None);
  }

  doc.codec_private = String::from_utf8_lossy(&writer.into_inner()).into_owned();
  Ok(Some(doc))
}

fn write_error(err: std::io::Error) -> ParseError {
  ParseError::Malformed {
    format: "usf",
    offset: 0,
    reason: format!("failed to re-serialize USF codec private: {err}"),
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UsfReader;

impl MediaReader for UsfReader {
  fn name(&self) -> &'static str {
    "usf"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    // Probe has no caller-supplied budget; use a generous fixed one so a real
    // XML parse can validate the root element (r_usf.cpp lines 45-46).
    let deadline = Deadline::from_parts(std::time::Instant::now(), std::time::Duration::from_secs(60));
    self.probe_with_deadline(src, &deadline)
  }

  fn probe_with_deadline(&self, src: &mut FileSource, deadline: &Deadline) -> Result<bool, ParseError> {
    let (_bytes, text) = read_probe_document(src, Some(deadline))?;
    src.seek_to(0)?;
    if !has_xml_marker(&text) {
      return Ok(false);
    }
    Ok(parse_document(&text, deadline)?.is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    deadline.check("usf::read_headers")?;
    let (bytes, text) = read_header_document(src, deadline)?;
    if !has_xml_marker(&text) {
      return Err(ParseError::Unrecognised);
    }
    let doc = match parse_document(&text, deadline)? {
      Some(doc) => doc,
      None => return Err(ParseError::Unrecognised),
    };

    out.container.format = ContainerFormat::Usf;
    out.container.recognized = true;
    out.container.supported = true;

    let detected = encoding::detect(&bytes);
    let codec_private = CodecPrivate::from_bytes(doc.codec_private.as_bytes());

    for (idx, track_language) in doc.tracks.iter().enumerate() {
      let mut common = CommonTrackProperties::default();
      common.number = Some(idx as u64 + 1);
      // Per-track language, else the document default (r_usf.cpp lines 64-65).
      let language_code = track_language
        .as_deref()
        .or(doc.default_language.as_deref());
      if let Some(code) = language_code {
        common.language = Language::from_valid_hint(code);
      }
      out.tracks.push(Track {
        id: idx as i64,
        track_type: TrackType::Subtitles,
        codec: CodecInfo {
          id: "S_TEXT/USF".to_string(),
          name: Some("USF (Universal Subtitle Format)".to_string()),
          codec_private: Some(codec_private.clone()),
        },
        properties: TrackProperties {
          common,
          subtitle: Some(SubtitleTrackProperties {
            text_subtitles: true,
            encoding: Some(detected.label.to_string()),
            variant: Some("USF".to_string()),
            teletext_page: None,
          }),
          ..TrackProperties::default()
        },
      });
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn run(blob: &[u8]) -> (bool, MediaMetadata) {
    let mut probe_src = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let claimed = UsfReader.probe(&mut probe_src).unwrap();
    let mut src = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.usf", blob.len() as u64);
    let _ = UsfReader.read_headers(&mut src, &Deadline::new(60_000), &mut out);
    (claimed, out)
  }

  #[test]
  fn probe_requires_xml_marker() {
    // No `<?xml` / `<!--` marker → upstream probe_file returns false.
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"<USFSubtitles></USFSubtitles>".to_vec()));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_usf_blob_with_declaration() {
    let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\"></USFSubtitles>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn deadline_aware_probe_uses_caller_budget() {
    let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\"></USFSubtitles>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let err = <UsfReader as MediaReader>::probe_with_deadline(&UsfReader, &mut s, &Deadline::new(0)).unwrap_err();
    assert!(matches!(err, ParseError::Timeout { stage, .. } if stage == "usf::probe_document"));
  }

  #[test]
  fn probe_accepts_leading_comment() {
    let blob = b"<!-- mux note -->\n<USFSubtitles></USFSubtitles>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_wrong_root() {
    let blob = b"<?xml version=\"1.0\"?>\n<html></html>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_malformed_xml() {
    let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles><unclosed>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_usf_track_for_empty_root() {
    // mkvtoolnix emits no tracks when there is no `<subtitles>` element; the
    // container is still recognised.
    let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles></USFSubtitles>";
    let (claimed, out) = run(blob);
    assert!(claimed);
    assert_eq!(out.container.format, ContainerFormat::Usf);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_emits_one_track_per_subtitles_element() {
    let blob = b"<?xml version=\"1.0\"?><USFSubtitles>\
      <subtitles><language code=\"eng\"/><subtitle start=\"0\">hi</subtitle></subtitles>\
      <subtitles><language code=\"fra\"/></subtitles></USFSubtitles>";
    let (claimed, out) = run(blob);
    assert!(claimed);
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(
      out.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
      "eng"
    );
    assert_eq!(
      out.tracks[1].properties.common.language.as_ref().unwrap().iso639_2,
      "fra"
    );
    assert!(out.tracks[0].codec.codec_private.is_some());
    assert_eq!(out.tracks[0].codec.id, "S_TEXT/USF");
  }

  #[test]
  fn read_headers_loads_full_document_beyond_probe_cap() {
    // PARSER-374: only probe_file applies the 10 MiB cap; read_headers reloads
    // the full XML document and must still see later direct-child subtitles.
    let mut blob = b"<?xml version=\"1.0\"?><USFSubtitles>\
      <subtitles><language code=\"eng\"/></subtitles>"
      .to_vec();
    blob.resize(PROBE_DOCUMENT_BYTES + 1024, b' ');
    blob.extend_from_slice(b"<subtitles><language code=\"fra\"/></subtitles></USFSubtitles>");

    let mut src = FileSource::from_reader_for_test(Cursor::new(blob.clone()));
    let mut out = MediaMetadata::new("large.usf", blob.len() as u64);
    UsfReader
      .read_headers(&mut src, &Deadline::new(60_000), &mut out)
      .unwrap();

    assert_eq!(out.tracks.len(), 2);
    assert_eq!(
      out.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
      "eng"
    );
    assert_eq!(
      out.tracks[1].properties.common.language.as_ref().unwrap().iso639_2,
      "fra"
    );
  }

  #[test]
  fn default_language_fills_tracks_without_language() {
    let blob = b"<?xml version=\"1.0\"?><USFSubtitles>\
      <metadata><language code=\"ger\"/></metadata>\
      <subtitles><subtitle>a</subtitle></subtitles>\
      <subtitles><language code=\"fra\"/></subtitles></USFSubtitles>";
    let (_, out) = run(blob);
    assert_eq!(out.tracks.len(), 2);
    // Track 0 has no language → inherits the metadata default (deu).
    assert_eq!(
      out.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
      "deu"
    );
    // Track 1 keeps its own language.
    assert_eq!(
      out.tracks[1].properties.common.language.as_ref().unwrap().iso639_2,
      "fra"
    );
  }

  #[test]
  fn codec_private_strips_subtitles_subtrees() {
    let blob = b"<?xml version=\"1.0\"?><USFSubtitles>\
      <metadata><title>T</title></metadata>\
      <subtitles><subtitle>keep me out</subtitle></subtitles></USFSubtitles>";
    let (_, out) = run(blob);
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    let bytes = hex_to_bytes(&private.hex);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("<metadata>"));
    assert!(text.contains("<title>T</title>"));
    assert!(text.contains("USFSubtitles"));
    // The `<subtitles>` subtree (and its payload) is removed.
    assert!(!text.contains("<subtitles"));
    assert!(!text.contains("keep me out"));
  }

  #[test]
  fn invalid_language_code_is_omitted() {
    let blob = b"<?xml version=\"1.0\"?><USFSubtitles>\
      <subtitles><language code=\"zzz\"/></subtitles></USFSubtitles>";
    let (_, out) = run(blob);
    assert_eq!(out.tracks.len(), 1);
    assert!(out.tracks[0].properties.common.language.is_none());
  }

  // ---- PARSER-236: marker window + exact root name --------------------

  #[test]
  fn xml_marker_only_scanned_in_leading_window() {
    // A marker beyond the ~1000-char window is not seen (upstream stops
    // accumulating at 1000 chars).
    let mut blob = "x".repeat(1100);
    blob.push_str("<?xml version=\"1.0\"?>");
    assert!(!has_xml_marker(&blob));
    // Within the window it is found.
    assert!(has_xml_marker("<?xml version=\"1.0\"?>"));
    assert!(has_xml_marker("<!-- note -->"));
  }

  #[test]
  fn probe_rejects_namespaced_root() {
    // pugixml's document_element().name() keeps the `usf:` prefix, so a
    // namespaced root does not equal "USFSubtitles".
    let blob = b"<?xml version=\"1.0\"?>\n<usf:USFSubtitles></usf:USFSubtitles>";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_marker_outside_window() {
    let mut blob = vec![b'\n'; 1100];
    blob.extend_from_slice(b"<?xml version=\"1.0\"?><USFSubtitles></USFSubtitles>");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(!UsfReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_rejects_non_usf_xml() {
    let blob = b"<?xml version=\"1.0\"?><html></html>";
    let mut src = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("x.usf", blob.len() as u64);
    let err = UsfReader
      .read_headers(&mut src, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
      .step_by(2)
      .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
      .collect()
  }
}
