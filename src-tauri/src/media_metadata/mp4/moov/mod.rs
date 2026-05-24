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

use super::atom::{self, BoxHeader, ChildAction};

pub use trak::TrackBuilder;

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
        _ => Ok(ChildAction::Skip),
    })
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
        parse(&mut s, &parent, &dl(), &mut b).unwrap();
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
        parse(&mut s, &parent, &dl(), &mut b).unwrap();
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
