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

//! `MpegPsReader` — walks start codes and collects unique stream IDs.

use std::collections::HashSet;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::identify::{self, classify_stream_id, StreamObservation};
use super::packet::{self, StartCode, PACK_HEADER};

const PROBE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegPsReader;

impl Reader for MpegPsReader {
    fn name(&self) -> &'static str {
        "mpeg_ps"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = vec![0u8; 256];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 4 {
            return Ok(false);
        }
        // Must begin with a pack header start code.
        Ok(read >= 4
            && head[0] == 0x00
            && head[1] == 0x00
            && head[2] == 0x01
            && head[3] == PACK_HEADER)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut probe)?;
        if read < 4 {
            return Err(ParseError::Unrecognised);
        }
        let bytes = &probe[..read];
        let mut seen_ids: HashSet<u8> = HashSet::new();
        let mut observations: Vec<StreamObservation> = Vec::new();
        let mut offset = 0usize;
        loop {
            deadline.check("mpeg_ps::reader")?;
            let Some((pos, sid)) = packet::find_start_code(bytes, offset) else {
                break;
            };
            offset = pos + 4;
            let candidate = match StartCode::from_byte(sid) {
                StartCode::Audio(b) | StartCode::Video(b) => Some(b),
                StartCode::PrivateStream1 => Some(0xBDu8),
                _ => None,
            };
            if let Some(c) = candidate {
                if seen_ids.insert(c) {
                    if let Some(obs) = classify_stream_id(c) {
                        observations.push(obs);
                    }
                }
            }
            if observations.len() >= 16 {
                break;
            }
        }
        identify::finalise(observations, out);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::model::container::ContainerFormat;
    use crate::media_metadata::model::track::TrackType;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn start_code(stream_id: u8) -> [u8; 4] {
        [0x00, 0x00, 0x01, stream_id]
    }

    fn build_ps(stream_ids: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&start_code(PACK_HEADER));
        bytes.extend_from_slice(&[0u8; 10]); // pack body
        for id in stream_ids {
            bytes.extend_from_slice(&start_code(*id));
            bytes.extend_from_slice(&8u16.to_be_bytes()); // packet length
            bytes.extend_from_slice(&[0u8; 8]);
        }
        bytes
    }

    #[test]
    fn probe_accepts_files_starting_with_pack_header() {
        let bytes = build_ps(&[0xE0]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(MpegPsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_files_without_pack_header() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
        assert!(!MpegPsReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_collects_unique_stream_ids() {
        let bytes = build_ps(&[0xE0, 0xC0, 0xE0, 0xC0, 0xBD]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::MpegPs);
        assert_eq!(out.tracks.len(), 3);
        let kinds: Vec<TrackType> = out.tracks.iter().map(|t| t.track_type).collect();
        assert!(kinds.contains(&TrackType::Video));
        assert!(kinds.contains(&TrackType::Audio));
    }

    #[test]
    fn read_headers_returns_unrecognised_on_empty_input() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(Vec::<u8>::new()));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        let err = MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Unrecognised));
    }

    #[test]
    fn padding_stream_is_ignored() {
        let bytes = build_ps(&[0xBE, 0xE0, 0xBE]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.mpg", 0);
        MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.tracks[0].track_type, TrackType::Video);
    }
}
