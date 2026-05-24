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

//! Top-level `AviReader` — drives the RIFF walk.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::avih::{self, MainAviHeader};
use super::identify;
use super::odml::{self, OdmlInfo};
use super::riff::{self, ChildAction, ChunkHeader};
use super::strl::{self, StreamBuilder};

#[derive(Debug, Default, Clone, Copy)]
pub struct AviReader;

impl Reader for AviReader {
    fn name(&self) -> &'static str {
        "avi"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 12];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 12 {
            return Ok(false);
        }
        // Only a primary `RIFF/AVI ` file is claimed. `AVIX` chunks are OpenDML
        // extension segments, not standalone files (PARSER-061).
        Ok(&head[0..4] == b"RIFF" && &head[8..12] == b"AVI ")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        src.seek_to(0)?;
        let riff_header = riff::read_chunk_header(src)?;
        if &riff_header.kind != b"RIFF" {
            return Err(ParseError::Malformed {
                format: "avi",
                offset: riff_header.start,
                reason: format!(
                    "expected RIFF, got '{}'",
                    riff::fourcc_string(&riff_header.kind)
                ),
            });
        }
        let form_type = riff::read_list_subtype(src)?;
        if &form_type != b"AVI " {
            return Err(ParseError::Malformed {
                format: "avi",
                offset: riff_header.start,
                reason: format!(
                    "RIFF form '{}' is not AVI",
                    riff::fourcc_string(&form_type)
                ),
            });
        }
        out.container.format = ContainerFormat::Avi;
        out.container.recognized = true;
        out.container.supported = true;

        let mut avih: Option<MainAviHeader> = None;
        let mut streams: Vec<StreamBuilder> = Vec::new();
        let mut odml_info = OdmlInfo::default();
        let mut found_hdrl = false;

        // Walk children of the outer RIFF list.  We don't use walk_list_children
        // here because we've already consumed the form_type FOURCC.
        let parent_end = riff_header.payload_end();
        let stream_end = src.length();
        loop {
            deadline.check("avi::reader")?;
            let pos = src.position();
            if pos >= parent_end {
                break;
            }
            if let Some(end) = stream_end {
                if pos >= end {
                    break;
                }
                if end - pos < 8 {
                    break;
                }
            }
            if parent_end - pos < 8 {
                break;
            }
            let child = match riff::read_chunk_header(src) {
                Ok(h) => h,
                Err(ParseError::UnexpectedEof { .. }) => break,
                Err(e) => return Err(e),
            };

            if child.is_list_container() {
                // Read the LIST sub-type to dispatch.
                let sub = riff::read_list_subtype(src)?;
                // Rewind so the LIST helpers see the sub-type FOURCC at the
                // expected offset.
                src.seek_to(child.payload_start())?;
                match &sub {
                    b"hdrl" => {
                        found_hdrl = true;
                        parse_hdrl(src, &child, deadline, &mut avih, &mut streams, &mut odml_info)?;
                    }
                    b"odml" => {
                        odml_info = odml::parse_odml_list(src, &child, deadline)?;
                    }
                    _ => {}
                }
                riff::skip_payload_with_pad(src, &child)?;
            } else {
                riff::skip_payload_with_pad(src, &child)?;
            }
        }

        if !found_hdrl {
            return Err(ParseError::Malformed {
                format: "avi",
                offset: 0,
                reason: "no hdrl LIST found".to_string(),
            });
        }

        identify::finalise(avih, streams, odml_info, out);
        Ok(())
    }
}

fn parse_hdrl(
    src: &mut FileSource,
    parent: &ChunkHeader,
    deadline: &Deadline,
    avih: &mut Option<MainAviHeader>,
    streams: &mut Vec<StreamBuilder>,
    odml_info: &mut OdmlInfo,
) -> Result<(), ParseError> {
    riff::walk_list_children(
        src,
        parent,
        "avi::hdrl",
        deadline,
        |src, child| match &child.kind {
            b"avih" => {
                *avih = Some(avih::parse(src, child)?);
                Ok(ChildAction::Consumed)
            }
            b"LIST" => {
                // Peek the sub-type: strl (per-stream) or odml (OpenDML header,
                // which conventionally lives inside hdrl — PARSER-058).
                let sub = riff::read_list_subtype(src)?;
                src.seek_to(child.payload_start())?;
                match &sub {
                    b"strl" => streams.push(strl::parse_strl(src, child, deadline)?),
                    b"odml" => *odml_info = odml::parse_odml_list(src, child, deadline)?,
                    _ => {}
                }
                Ok(ChildAction::Skip)
            }
            _ => Ok(ChildAction::Skip),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::avi::avih::build_avih_payload;
    use crate::media_metadata::avi::riff::{encode_chunk, encode_list};
    use crate::media_metadata::avi::strl::{
        build_bitmapinfoheader, build_strh_payload, build_waveformatex,
    };
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::model::track::TrackType;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn build_video_strl(width: u16, height: u16) -> Vec<u8> {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1001, 24000, 240, 0),
        );
        let strf = encode_chunk(
            b"strf",
            &build_bitmapinfoheader(width as i32, height as i32, 24, b"H264"),
        );
        let mut payload = strh;
        payload.extend(strf);
        encode_list(b"LIST", b"strl", &[payload])
    }

    fn build_audio_strl() -> Vec<u8> {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"auds", b"\0\0\0\0", 1, 48000, 0, 4),
        );
        let strf = encode_chunk(
            b"strf",
            &build_waveformatex(0x0055, 2, 48000, 16000, 4, 16, &[]),
        );
        let mut payload = strh;
        payload.extend(strf);
        encode_list(b"LIST", b"strl", &[payload])
    }

    fn build_avi(streams_payload: Vec<Vec<u8>>, with_odml: bool) -> Vec<u8> {
        let avih = encode_chunk(
            b"avih",
            &build_avih_payload(41_708, 5_000_000, 0x10, 240, streams_payload.len() as u32, 1920, 1080),
        );
        let mut hdrl_children = vec![avih];
        hdrl_children.extend(streams_payload);
        let hdrl = encode_list(b"LIST", b"hdrl", &hdrl_children);

        let mut riff_payload = b"AVI ".to_vec();
        riff_payload.extend(hdrl);
        if with_odml {
            let dmlh = encode_chunk(b"dmlh", &500_000u32.to_le_bytes());
            let odml = encode_list(b"LIST", b"odml", &[dmlh]);
            riff_payload.extend(odml);
        }
        // movi list (empty payload is fine for identification)
        let movi = encode_list(b"LIST", b"movi", &[]);
        riff_payload.extend(movi);

        // Manually wrap as RIFF with the AVI form type as the sub-FOURCC.
        let total_size = riff_payload.len() as u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&total_size.to_le_bytes());
        bytes.extend(riff_payload);
        bytes
    }

    #[test]
    fn probe_accepts_riff_avi_header() {
        let bytes = build_avi(vec![build_video_strl(640, 480)], false);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(AviReader.probe(&mut s).unwrap());
        assert_eq!(s.position(), 0);
    }

    #[test]
    fn probe_rejects_short_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
        assert!(!AviReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_riff_with_non_avi_form_type() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&100u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(&[0u8; 4]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(!AviReader.probe(&mut s).unwrap());
    }

    // ---- PARSER-061: standalone AVIX is not a primary AVI file -------------

    #[test]
    fn probe_rejects_standalone_avix() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&100u32.to_le_bytes());
        bytes.extend_from_slice(b"AVIX");
        bytes.extend_from_slice(&[0u8; 4]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(!AviReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_video_and_audio_tracks() {
        let bytes = build_avi(
            vec![build_video_strl(1920, 1080), build_audio_strl()],
            false,
        );
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.avi", 0);
        AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::Avi);
        assert_eq!(out.tracks.len(), 2);
        assert_eq!(out.tracks[0].track_type, TrackType::Video);
        assert_eq!(out.tracks[1].track_type, TrackType::Audio);
        let v = out.tracks[0].properties.video.as_ref().unwrap();
        assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
        let a = out.tracks[1].properties.audio.as_ref().unwrap();
        assert_eq!(a.sampling_frequency, Some(48000.0));
    }

    #[test]
    fn read_headers_uses_odml_total_frames_when_present() {
        let bytes = build_avi(vec![build_video_strl(640, 480)], true);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.avi", 0);
        AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert!(out.container.properties.duration.is_some());
    }

    #[test]
    fn missing_hdrl_returns_malformed() {
        let mut riff_payload = b"AVI ".to_vec();
        riff_payload.extend(encode_list(b"LIST", b"movi", &[]));
        let total_size = riff_payload.len() as u32;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&total_size.to_le_bytes());
        bytes.extend(riff_payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.avi", 0);
        let err = AviReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn non_riff_top_level_chunk_is_rejected() {
        let mut bytes = b"FAKE".to_vec();
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(b"AVI ");
        bytes.extend_from_slice(&[0u8; 8]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.avi", 0);
        let err = AviReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }
}
