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

//! Dirac elementary stream reader.
//!
//! The stream begins with a parse-info block whose magic is `BBCD` followed
//! by a one-byte parse-code.  Sequence-header parse-code = `0x00`.  We sniff
//! the magic + parse code and emit a single Dirac video track; the full
//! sequence-header bit-stream (interlace flag, frame rate, etc.) is left for
//! a fast-follow PR because identification needs just the codec ID.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

pub const PARSE_INFO_MAGIC: [u8; 4] = *b"BBCD";

#[derive(Debug, Default, Clone, Copy)]
pub struct DiracReader;

impl Reader for DiracReader {
    fn name(&self) -> &'static str {
        "dirac"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 5];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read >= 5 && head[..4] == PARSE_INFO_MAGIC && head[4] == 0x00)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut head = [0u8; 5];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut head)?;
        if read < 5 || head[..4] != PARSE_INFO_MAGIC {
            return Err(ParseError::Unrecognised);
        }

        out.container.format = ContainerFormat::Dirac;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Video,
            codec: CodecInfo {
                id: "V_DIRAC".to_string(),
                name: Some("Dirac".to_string()),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                video: Some(VideoTrackProperties::default()),
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn build_dirac_stream() -> Vec<u8> {
    let mut bytes = PARSE_INFO_MAGIC.to_vec();
    bytes.push(0x00); // sequence-header parse-code
    bytes.extend_from_slice(&[0u8; 16]); // placeholder body
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn probe_accepts_bbcd_magic_plus_sequence_header() {
        let bytes = build_dirac_stream();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(DiracReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_wrong_parse_code() {
        let mut bytes = build_dirac_stream();
        bytes[4] = 0x10; // not sequence header
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(!DiracReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_short_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"BBC".to_vec()));
        assert!(!DiracReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_dirac_track() {
        let bytes = build_dirac_stream();
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.drc", 0);
        DiracReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Dirac);
        assert_eq!(out.tracks[0].codec.id, "V_DIRAC");
    }

    #[test]
    fn read_headers_rejects_non_dirac_input() {
        let bytes = vec![0xAAu8; 16];
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.drc", 0);
        let err = DiracReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }
}
