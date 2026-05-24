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
//! USF is an XML subtitle format.  We detect the root `<USFSubtitles ...>`
//! element after stripping any XML declaration / BOM.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::encoding;

const PROBE_BYTES: usize = 4 * 1024;

pub fn looks_like_usf(text: &str) -> bool {
    let mut cursor = text.trim_start_matches(|c: char| c.is_ascii_whitespace());
    if let Some(rest) = cursor.strip_prefix("<?xml") {
        match rest.find("?>") {
            Some(end) => cursor = rest[end + 2..].trim_start(),
            None => return false,
        }
    }
    // mkvtoolnix's r_usf.cpp tolerates leading XML comments before the
    // root element; consume any number of `<!-- ... -->` blocks.
    loop {
        cursor = cursor.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let rest = match cursor.strip_prefix("<!--") {
            Some(r) => r,
            None => break,
        };
        match rest.find("-->") {
            Some(end) => cursor = &rest[end + 3..],
            None => return false,
        }
    }
    cursor.starts_with("<USFSubtitles")
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UsfReader;

impl Reader for UsfReader {
    fn name(&self) -> &'static str {
        "usf"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        Ok(read > 0 && looks_like_usf(&encoding::decode_lossy(&buf[..read])))
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
        if !looks_like_usf(&text) {
            return Err(ParseError::Unrecognised);
        }

        out.container.format = ContainerFormat::Usf;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Subtitles,
            codec: CodecInfo {
                id: "S_TEXT/USF".to_string(),
                name: Some("USF (Universal Subtitle Format)".to_string()),
                codec_private: None,
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn looks_like_usf_accepts_bare_root_element() {
        assert!(looks_like_usf("<USFSubtitles>\n"));
    }

    #[test]
    fn looks_like_usf_accepts_xml_declaration_prefix() {
        assert!(looks_like_usf(
            "<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\">"
        ));
    }

    #[test]
    fn looks_like_usf_rejects_other_roots() {
        assert!(!looks_like_usf("<rss>"));
        assert!(!looks_like_usf("<?xml version=\"1.0\"?><html>"));
    }

    #[test]
    fn probe_accepts_usf_blob() {
        let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\">";
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
        assert!(UsfReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_usf_track() {
        use crate::media_metadata::deadline::Deadline;
        let blob = b"<USFSubtitles>\n";
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob.to_vec()));
        let mut out = MediaMetadata::new("clip.usf", 0);
        UsfReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Usf);
        assert_eq!(out.tracks[0].codec.id, "S_TEXT/USF");
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
        assert!(!UsfReader.probe(&mut s).unwrap());
    }

    #[test]
    fn looks_like_usf_rejects_unterminated_xml_decl() {
        assert!(!looks_like_usf("<?xml version=\"1.0\"\n<USFSubtitles>"));
    }

    #[test]
    fn looks_like_usf_tolerates_leading_xml_comments() {
        assert!(looks_like_usf(
            "<?xml version=\"1.0\"?>\n<!-- mux note -->\n<USFSubtitles>"
        ));
        assert!(looks_like_usf("<!-- a --><!-- b --><USFSubtitles>"));
    }

    #[test]
    fn looks_like_usf_rejects_unterminated_comment() {
        assert!(!looks_like_usf("<!-- never closed <USFSubtitles>"));
    }
}
