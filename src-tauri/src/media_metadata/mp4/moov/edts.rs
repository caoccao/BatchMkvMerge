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

//! `edts` (edit list) → `elst` (entry list).  Per ISO/IEC 14496-12 §8.6.6.
//!
//! `elst` carries one or more entries:
//!
//! ```text
//! v0: segment_duration (u32) + media_time (i32) + media_rate_int (i16) + media_rate_frac (i16)
//! v1: segment_duration (u64) + media_time (i64) + media_rate_int (i16) + media_rate_frac (i16)
//! ```
//!
//! For identification we sum `segment_duration` across all entries to derive
//! the track's effective edited duration and note whether any entry begins
//! with a non-trivial sync point (negative `media_time` ⇒ empty padding,
//! positive ⇒ presentation offset).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::trak::TrackBuilder;

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    atom::walk_children(src, parent, "mp4::edts", deadline, |src, child| {
        if !child.kind.eq_ascii(b"elst") {
            return Ok(ChildAction::Skip);
        }
        parse_elst(src, child, builder)?;
        Ok(ChildAction::Consumed)
    })
}

fn parse_elst(
    src: &mut FileSource,
    header: &BoxHeader,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    let payload = header.payload_size().unwrap_or(0);
    if payload < 8 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: header.start,
            reason: format!("elst payload {payload} bytes is too small"),
        });
    }
    let version = src.read_u8()?;
    let _flags = [src.read_u8()?, src.read_u8()?, src.read_u8()?];
    let entry_count = src.read_u32_be()?;
    // 1 entry = 12 bytes (v0) or 20 bytes (v1).
    let entry_bytes = if version == 0 { 12u64 } else { 20 };
    let expected = entry_count as u64 * entry_bytes;
    if expected > payload - 8 {
        // Truncated; treat as malformed.
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: header.start,
            reason: format!(
                "elst declares {entry_count} entries needing {expected} bytes but payload has only {}",
                payload - 8
            ),
        });
    }

    let mut total = 0u64;
    let mut has_offset = false;
    for _ in 0..entry_count {
        let (segment_duration, media_time) = if version == 0 {
            (src.read_u32_be()? as u64, src.read_u32_be()? as i32 as i64)
        } else {
            (src.read_u64_be()?, src.read_u64_be()? as i64)
        };
        // media_rate is 4 bytes (int + frac) for both versions.
        src.skip(4)?;
        total = total.saturating_add(segment_duration);
        if media_time != 0 && media_time != -1 {
            has_offset = true;
        }
    }
    builder.edts_total_duration = Some(total);
    builder.edts_has_offset = has_offset;
    Ok(())
}

#[cfg(test)]
pub(crate) fn build_elst_v0(entries: &[(u32, i32)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(0); // version
    p.extend_from_slice(&[0u8; 3]); // flags
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (dur, media_time) in entries {
        p.extend_from_slice(&dur.to_be_bytes());
        p.extend_from_slice(&media_time.to_be_bytes());
        p.extend_from_slice(&[0u8; 4]); // media_rate
    }
    p
}

#[cfg(test)]
pub(crate) fn build_elst_v1(entries: &[(u64, i64)]) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(1); // version
    p.extend_from_slice(&[0u8; 3]); // flags
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (dur, media_time) in entries {
        p.extend_from_slice(&dur.to_be_bytes());
        p.extend_from_slice(&media_time.to_be_bytes());
        p.extend_from_slice(&[0u8; 4]);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::mp4::atom::encode_box;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn run(payload: Vec<u8>) -> TrackBuilder {
        let elst = encode_box(b"elst", &payload);
        let edts = encode_box(b"edts", &elst);
        let mut s = FileSource::from_reader_for_test(Cursor::new(edts));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        parse(&mut s, &parent, &dl(), &mut b).unwrap();
        b
    }

    #[test]
    fn sums_v0_segment_durations() {
        let b = run(build_elst_v0(&[(100, 0), (200, 0)]));
        assert_eq!(b.edts_total_duration, Some(300));
        assert!(!b.edts_has_offset);
    }

    #[test]
    fn sums_v1_segment_durations() {
        let b = run(build_elst_v1(&[(100u64, 0i64), (200, 0)]));
        assert_eq!(b.edts_total_duration, Some(300));
    }

    #[test]
    fn negative_one_media_time_does_not_flag_offset() {
        // -1 = empty padding, not a sync offset.
        let b = run(build_elst_v0(&[(100, -1)]));
        assert!(!b.edts_has_offset);
    }

    #[test]
    fn positive_media_time_flags_offset() {
        let b = run(build_elst_v0(&[(100, 42)]));
        assert!(b.edts_has_offset);
    }

    #[test]
    fn rejects_truncated_payload() {
        let elst = encode_box(b"elst", &[0u8; 4]);
        let edts = encode_box(b"edts", &elst);
        let mut s = FileSource::from_reader_for_test(Cursor::new(edts));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let err = parse(&mut s, &parent, &dl(), &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn rejects_entry_count_overflowing_payload() {
        let mut p = Vec::new();
        p.push(0);
        p.extend_from_slice(&[0u8; 3]);
        p.extend_from_slice(&999u32.to_be_bytes()); // declares 999 entries
        let elst = encode_box(b"elst", &p);
        let edts = encode_box(b"edts", &elst);
        let mut s = FileSource::from_reader_for_test(Cursor::new(edts));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        let err = parse(&mut s, &parent, &dl(), &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn ignores_non_elst_children() {
        let other = encode_box(b"xxxx", &[]);
        let edts = encode_box(b"edts", &other);
        let mut s = FileSource::from_reader_for_test(Cursor::new(edts));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        parse(&mut s, &parent, &dl(), &mut b).unwrap();
        assert!(b.edts_total_duration.is_none());
    }
}
