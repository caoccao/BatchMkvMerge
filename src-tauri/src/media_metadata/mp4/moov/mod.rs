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

//! `moov` box tree — top-level movie metadata.
//!
//! Walks:
//! - `mvhd` — movie header (timescale + duration).
//! - `trak*` — one per track; dispatches to per-track sub-walkers
//!   (`tkhd`, `mdia → mdhd / hdlr / minf → stbl → stsd / stts`, `edts/elst`).
//! - `udta`/`meta` — iTunes metadata (delegated to [`super::meta`]).
//! - `mvex` — fragment defaults (delegated to [`super::fragments`]).
//!
//! Per-track state is accumulated in [`TrackBuilder`] and turned into the
//! protocol's `Track` shape by [`super::reader`] after the moov walk.

pub mod edts;
pub mod hdlr;
pub mod mdhd;
pub mod mdia;
pub mod mvhd;
pub mod stbl;
pub mod tkhd;
pub mod trak;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerProperties;
use crate::media_metadata::model::MediaMetadata;

use super::atom::{self, BoxHeader, ChildAction};

pub use trak::TrackBuilder;

/// Cap on a compressed `cmvd` payload and its inflated output.
const CMOV_CAP: u64 = 64 * 1024 * 1024;

/// Movie-level information collected during a `moov` walk.
#[derive(Debug, Default)]
pub struct MoovBuilder {
    pub timescale: Option<u32>,
    pub duration_units: Option<u64>,
    pub next_track_id: Option<u32>,
    pub tracks: Vec<TrackBuilder>,
    pub mvex_defaults: super::fragments::TrexDefaults,
}

impl MoovBuilder {
    /// Project the movie-level fields onto [`ContainerProperties`].
    pub fn finalise_container(&self, props: &mut ContainerProperties) {
        if let Some(ts) = self.timescale {
            props.movie_timescale = Some(ts);
        }
        if let (Some(ts), Some(units)) = (self.timescale, self.duration_units) {
            if ts > 0 {
                let ns = (units as u128).saturating_mul(1_000_000_000) / ts as u128;
                props.duration = Some(
                    crate::media_metadata::model::duration::DurationValue::from_ns(ns as u64),
                );
            }
        }
    }
}

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    builder: &mut MoovBuilder,
    out: &mut MediaMetadata,
) -> Result<(), ParseError> {
    atom::walk_children(src, parent, "mp4::moov", deadline, |src, child| match &child.kind.0 {
        b"mvhd" => {
            let h = mvhd::parse(src, child)?;
            builder.timescale = Some(h.timescale);
            builder.duration_units = Some(h.duration);
            builder.next_track_id = Some(h.next_track_id);
            Ok(ChildAction::Consumed)
        }
        b"trak" => {
            let track = trak::parse(src, child, deadline)?;
            builder.tracks.push(track);
            Ok(ChildAction::Consumed)
        }
        b"mvex" => {
            super::fragments::parse_mvex(src, child, deadline, &mut builder.mvex_defaults)?;
            Ok(ChildAction::Consumed)
        }
        // iTunes / QuickTime metadata under moov (PARSER-042).
        b"udta" => {
            super::meta::udta::parse_udta(src, child, deadline, out)?;
            Ok(ChildAction::Consumed)
        }
        b"meta" => {
            super::meta::udta::parse_meta(src, child, deadline, out)?;
            Ok(ChildAction::Consumed)
        }
        // Compressed QuickTime movie box (PARSER-041).
        b"cmov" => {
            handle_cmov(src, child, deadline, builder, out)?;
            Ok(ChildAction::Consumed)
        }
        _ => Ok(ChildAction::Skip),
    })
}

/// Decompress a `cmov` (compressed movie) box and re-parse the inflated `moov`
/// atom. Mirrors `r_qtmp4.cpp::handle_cmov_atom` (zlib only).
fn handle_cmov(
    src: &mut FileSource,
    cmov: &BoxHeader,
    deadline: &Deadline,
    builder: &mut MoovBuilder,
    out: &mut MediaMetadata,
) -> Result<(), ParseError> {
    let mut method: Option<[u8; 4]> = None;
    let mut compressed: Option<Vec<u8>> = None;
    atom::walk_children(src, cmov, "mp4::cmov", deadline, |src, child| match &child.kind.0 {
        b"dcom" => {
            let p = atom::read_payload(src, child, 64)?;
            if p.len() >= 4 {
                method = Some([p[0], p[1], p[2], p[3]]);
            }
            Ok(ChildAction::Consumed)
        }
        b"cmvd" => {
            compressed = Some(atom::read_payload(src, child, CMOV_CAP)?);
            Ok(ChildAction::Consumed)
        }
        _ => Ok(ChildAction::Skip),
    })?;

    let (Some(method), Some(cmvd)) = (method, compressed) else {
        return Ok(());
    };
    if &method != b"zlib" || cmvd.len() < 4 {
        return Ok(());
    }
    let uncompressed_size =
        u32::from_be_bytes([cmvd[0], cmvd[1], cmvd[2], cmvd[3]]) as usize;
    let inflated = inflate_zlib(&cmvd[4..], uncompressed_size)?;

    // The inflated payload is a complete `moov` atom; re-walk it from memory.
    let mut mem = FileSource::from_memory(inflated);
    let moov_header = match atom::read_box_header(&mut mem) {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };
    if &moov_header.kind.0 == b"moov" {
        parse(&mut mem, &moov_header, deadline, builder, out)?;
    }
    Ok(())
}

fn inflate_zlib(data: &[u8], expected: usize) -> Result<Vec<u8>, ParseError> {
    use std::io::Read;
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::with_capacity(expected.min(CMOV_CAP as usize));
    decoder
        .take(CMOV_CAP)
        .read_to_end(&mut out)
        .map_err(|e| ParseError::Malformed {
            format: "mp4",
            offset: 0,
            reason: format!("cmov zlib inflate failed: {e}"),
        })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mp4::atom::encode_box;
    use crate::media_metadata::mp4::moov::mvhd::build_mvhd_payload_v0;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    #[test]
    fn parses_mvhd_into_builder() {
        let mvhd_payload = build_mvhd_payload_v0(1000, 60_000, 3);
        let mvhd = encode_box(b"mvhd", &mvhd_payload);
        let moov = encode_box(b"moov", &mvhd);
        let mut s = FileSource::from_reader_for_test(Cursor::new(moov));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = MoovBuilder::default();
        parse(&mut s, &parent, &dl(), &mut b, &mut MediaMetadata::new("t.mp4", 0)).unwrap();
        assert_eq!(b.timescale, Some(1000));
        assert_eq!(b.duration_units, Some(60_000));
        assert_eq!(b.next_track_id, Some(3));
    }

    #[test]
    fn unknown_child_is_skipped() {
        let bogus = encode_box(b"junk", &[0u8; 4]);
        let moov = encode_box(b"moov", &bogus);
        let mut s = FileSource::from_reader_for_test(Cursor::new(moov));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = MoovBuilder::default();
        parse(&mut s, &parent, &dl(), &mut b, &mut MediaMetadata::new("t.mp4", 0)).unwrap();
        assert!(b.timescale.is_none());
    }

    #[test]
    fn finalise_container_emits_movie_timescale_and_duration() {
        let mut b = MoovBuilder::default();
        b.timescale = Some(1000);
        b.duration_units = Some(60_000);
        let mut props = ContainerProperties::default();
        b.finalise_container(&mut props);
        assert_eq!(props.movie_timescale, Some(1000));
        assert_eq!(props.duration.unwrap().ns, 60_000_000_000);
    }

    #[test]
    fn finalise_skips_duration_when_timescale_zero() {
        let mut b = MoovBuilder::default();
        b.timescale = Some(0);
        b.duration_units = Some(60_000);
        let mut props = ContainerProperties::default();
        b.finalise_container(&mut props);
        assert_eq!(props.movie_timescale, Some(0));
        assert!(props.duration.is_none());
    }
}
