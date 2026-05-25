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

//! WebVTT reader.  W3C: a WebVTT file starts with the literal `WEBVTT`
//! followed by either a newline, a tab, or a space.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 1024;

pub fn looks_like_webvtt(text: &str) -> bool {
  if !text.starts_with("WEBVTT") {
    return false;
  }
  matches!(
    text.as_bytes().get(6).copied(),
    Some(b'\n') | Some(b'\r') | Some(b'\t') | Some(b' ') | None
  )
}

pub fn codec_private_header(text: &str) -> Option<String> {
  let mut header = Vec::new();
  for line in text.lines() {
    if line.contains("-->") {
      break;
    }
    header.push(line);
  }
  let joined = header.join("\n").trim_end().to_string();
  if joined.trim() == "WEBVTT" || joined.trim().is_empty() {
    None
  } else {
    Some(format!("{joined}\n"))
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WebVttReader;

impl Reader for WebVttReader {
  fn name(&self) -> &'static str {
    "webvtt"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read > 0 && looks_like_webvtt(&encoding::decode_lossy(&buf[..read])))
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    let detected = encoding::detect(&buf[..read]);
    let text = encoding::decode_lossy(&buf[..read]);
    if !looks_like_webvtt(&text) {
      return Err(ParseError::Unrecognised);
    }
    let private = codec_private_header(&text);

    out.container.format = ContainerFormat::Webvtt;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Subtitles,
      codec: CodecInfo {
        id: "S_TEXT/WEBVTT".to_string(),
        name: Some("WebVTT".to_string()),
        codec_private: private
          .as_deref()
          .map(|header| CodecPrivate::from_bytes(header.as_bytes())),
      },
      properties: TrackProperties {
        common,
        subtitle: Some(SubtitleTrackProperties {
          text_subtitles: true,
          encoding: Some(detected.label.to_string()),
          variant: Some("WebVTT".to_string()),
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
  fn looks_like_webvtt_accepts_canonical_signature() {
    assert!(looks_like_webvtt("WEBVTT\n"));
    assert!(looks_like_webvtt("WEBVTT\r\n"));
    assert!(looks_like_webvtt("WEBVTT title"));
    assert!(looks_like_webvtt("WEBVTT\tcomment"));
  }

  #[test]
  fn looks_like_webvtt_rejects_other_prefixes() {
    assert!(!looks_like_webvtt("WEBVTTX"));
    assert!(!looks_like_webvtt("webvtt\n"));
    assert!(!looks_like_webvtt("HELLO"));
  }

  #[test]
  fn probe_accepts_webvtt_blob() {
    let blob = b"WEBVTT\n\n00:00.000 --> 00:02.000\nHello\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    assert!(WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_webvtt_track() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"WEBVTT\n\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Webvtt);
    assert_eq!(out.tracks[0].codec.id, "S_TEXT/WEBVTT");
  }

  #[test]
  fn read_headers_preserves_style_region_header() {
    use crate::media_metadata::deadline::Deadline;
    let blob = b"WEBVTT\n\nSTYLE\n::cue { color: lime }\n\n00:00.000 --> 00:01.000\nHi\n";
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
    let mut out = MediaMetadata::new("clip.vtt", 0);
    WebVttReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(
      out.tracks[0]
        .codec
        .codec_private
        .as_ref()
        .unwrap()
        .hex
        .contains("5354594c45")
    );
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
    assert!(!WebVttReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_utf8_bom_prefix() {
    let mut blob = vec![0xEFu8, 0xBB, 0xBF];
    blob.extend_from_slice(b"WEBVTT\n");
    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    assert!(WebVttReader.probe(&mut s).unwrap());
  }
}
