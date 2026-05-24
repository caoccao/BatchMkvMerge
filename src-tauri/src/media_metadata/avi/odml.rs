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

//! OpenDML extension — the LIST/odml > dmlh chunk that lets AVI files exceed
//! the 2 GB / 32-bit barrier.  We only read `dmlh::total_frames` (the
//! extended frame count), which mkvmerge uses when the avih `total_frames`
//! field rolls over for long files.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use super::riff::{self, ChildAction, ChunkHeader};

#[derive(Debug, Default, Clone, Copy)]
pub struct OdmlInfo {
    /// 32-bit total-frame count from the `dmlh` chunk.  `None` when the
    /// chunk is absent or unreadable.
    pub total_frames: Option<u32>,
}

pub fn parse_odml_list(
    src: &mut FileSource,
    parent: &ChunkHeader,
    deadline: &Deadline,
) -> Result<OdmlInfo, ParseError> {
    let mut info = OdmlInfo::default();
    riff::walk_list_children(src, parent, "avi::odml", deadline, |src, child| {
        if &child.kind == b"dmlh" {
            // First DWORD = total frames.  Larger structures exist in newer
            // OpenDML versions but identification only needs the count.
            let payload = riff::read_payload(src, child, 16 * 1024)?;
            if payload.len() >= 4 {
                info.total_frames = Some(u32::from_le_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                ]));
            }
            Ok(ChildAction::Consumed)
        } else {
            Ok(ChildAction::Skip)
        }
    })?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::avi::riff::{self, encode_chunk, encode_list};
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn parse_payload(children: Vec<Vec<u8>>) -> OdmlInfo {
        let list = encode_list(b"LIST", b"odml", &children);
        let mut s = FileSource::from_reader_for_test(Cursor::new(list));
        let parent = riff::read_chunk_header(&mut s).unwrap();
        parse_odml_list(&mut s, &parent, &dl()).unwrap()
    }

    #[test]
    fn dmlh_total_frames_extracted() {
        let dmlh = encode_chunk(b"dmlh", &123_456u32.to_le_bytes());
        let info = parse_payload(vec![dmlh]);
        assert_eq!(info.total_frames, Some(123_456));
    }

    #[test]
    fn empty_odml_list_returns_default() {
        let info = parse_payload(vec![]);
        assert!(info.total_frames.is_none());
    }

    #[test]
    fn dmlh_smaller_than_dword_is_ignored() {
        let dmlh = encode_chunk(b"dmlh", &[0u8; 2]);
        let info = parse_payload(vec![dmlh]);
        assert!(info.total_frames.is_none());
    }

    #[test]
    fn non_dmlh_children_skipped() {
        let other = encode_chunk(b"xxxx", &[1, 2, 3, 4]);
        let info = parse_payload(vec![other]);
        assert!(info.total_frames.is_none());
    }

    #[test]
    fn dmlh_with_extended_payload_still_reads_first_dword() {
        let mut payload = 9_000_000u32.to_le_bytes().to_vec();
        payload.extend_from_slice(&[0u8; 32]); // unused trailing bytes
        let dmlh = encode_chunk(b"dmlh", &payload);
        let info = parse_payload(vec![dmlh]);
        assert_eq!(info.total_frames, Some(9_000_000));
    }
}
