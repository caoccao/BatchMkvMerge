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

//! HDMV TextST reader.
//!
//! mkvtoolnix's `r_hdmv_textst.cpp` recognises these files by a 6-byte ASCII
//! magic `"TextST"` followed by a Dialog Style segment (0x81).  Each segment
//! has the layout
//!
//! ```text
//! 1 byte   segment_type (0x81 Dialog Style, 0x82 Dialog Presentation, 0x80 END)
//! 2 bytes  segment_length (big-endian)
//! ...      segment payload
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 64 * 1024;
const SEGMENT_HEADER_LEN: usize = 3;
pub const MAGIC: [u8; 6] = *b"TextST";

const SEG_DIALOG_STYLE: u8 = 0x81;
const SEG_DIALOG_PRESENTATION: u8 = 0x82;
const SEG_END: u8 = 0x80;

fn is_valid_segment_type(b: u8) -> bool {
    matches!(b, SEG_DIALOG_STYLE | SEG_DIALOG_PRESENTATION | SEG_END)
}

/// Walks the segment chain.  Returns the count of valid segments when the
/// file starts with the `"TextST"` magic followed by a Dialog Style header.
pub fn count_segments(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < MAGIC.len() + SEGMENT_HEADER_LEN {
        return None;
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    if bytes[MAGIC.len()] != SEG_DIALOG_STYLE {
        return None;
    }
    let mut pos = MAGIC.len();
    let mut count = 0usize;
    while pos + SEGMENT_HEADER_LEN <= bytes.len() {
        let seg_type = bytes[pos];
        if !is_valid_segment_type(seg_type) {
            break;
        }
        let seg_len = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
        pos += SEGMENT_HEADER_LEN + seg_len;
        count += 1;
    }
    if count == 0 { None } else { Some(count) }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HdmvTextStReader;

impl Reader for HdmvTextStReader {
    fn name(&self) -> &'static str {
        "hdmv_textst"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        let read = src.read_at_most(&mut buf)?;
        src.seek_to(0)?;
        Ok(read >= SEGMENT_HEADER_LEN && count_segments(&buf[..read]).is_some())
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
        if count_segments(&buf[..read]).is_none() {
            return Err(ParseError::Unrecognised);
        }

        out.container.format = ContainerFormat::HdmvTextSt;
        out.container.recognized = true;
        out.container.supported = true;

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Subtitles,
            codec: CodecInfo {
                id: "S_HDMV/TEXTST".to_string(),
                name: Some("HDMV TextST".to_string()),
                codec_private: None,
            },
            properties: TrackProperties {
                common,
                subtitle: Some(SubtitleTrackProperties {
                    text_subtitles: true,
                    encoding: Some("UTF-8".to_string()),
                    variant: Some("HDMV TextST".to_string()),
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

    fn build_segment(seg_type: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(SEGMENT_HEADER_LEN + payload.len());
        bytes.push(seg_type);
        let len = payload.len() as u16;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn build_clip(segments: Vec<Vec<u8>>) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        for seg in segments {
            bytes.extend(seg);
        }
        bytes
    }

    #[test]
    fn count_segments_accepts_magic_then_style_then_presentation() {
        let blob = build_clip(vec![
            build_segment(SEG_DIALOG_STYLE, &[0u8; 8]),
            build_segment(SEG_DIALOG_PRESENTATION, &[0u8; 16]),
            build_segment(SEG_END, &[]),
        ]);
        assert_eq!(count_segments(&blob), Some(3));
    }

    #[test]
    fn count_segments_rejects_without_textst_magic() {
        let blob = build_segment(SEG_DIALOG_STYLE, &[0u8; 8]);
        assert!(count_segments(&blob).is_none());
    }

    #[test]
    fn count_segments_requires_style_first() {
        let blob = build_clip(vec![build_segment(SEG_DIALOG_PRESENTATION, &[0u8; 8])]);
        assert!(count_segments(&blob).is_none());
    }

    #[test]
    fn count_segments_rejects_invalid_type() {
        let blob = build_clip(vec![build_segment(0x42, &[])]);
        assert!(count_segments(&blob).is_none());
    }

    #[test]
    fn probe_accepts_textst_blob() {
        let blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &[0u8; 8])]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        assert!(HdmvTextStReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_emits_textst_track() {
        let blob = build_clip(vec![build_segment(SEG_DIALOG_STYLE, &[0u8; 8])]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
        let mut out = MediaMetadata::new("clip.textst", 0);
        HdmvTextStReader
            .read_headers(&mut s, &Deadline::new(60_000), &mut out)
            .unwrap();
        assert_eq!(out.container.format, ContainerFormat::HdmvTextSt);
        let sub = out.tracks[0].properties.subtitle.as_ref().unwrap();
        assert!(sub.text_subtitles);
    }

    #[test]
    fn probe_rejects_random_bytes() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 256]));
        assert!(!HdmvTextStReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_short_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0x81u8]));
        assert!(!HdmvTextStReader.probe(&mut s).unwrap());
    }
}
