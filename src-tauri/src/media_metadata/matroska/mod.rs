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

//! Native Matroska (EBML / WebM / .mkv) reader.
//!
//! Header-only port of `mkvtoolnix/src/input/r_matroska.cpp` (~3,100 LOC). The
//! parser walks the EBML structure to populate
//! [`crate::media_metadata::model::MediaMetadata`] without depending on
//! libebml / libmatroska — every byte read happens through our own
//! [`crate::media_metadata::io`] helpers.
//!
//! Module layout:
//! - [`ebml`]: generic element header walker (id + size VINTs).
//! - [`ids`]: named EBML element IDs (Matroska spec + libmatroska).
//! - [`reader`]: top-level `read_headers_internal` pipeline.
//! - [`seek_head`]: deferred L1 element index.
//! - [`info`]: Segment/Info — title, muxing/writing app, duration, UIDs.
//! - [`tracks`]: TrackEntry walker plus per-domain sub-trees.
//! - [`attachments`], [`chapters`], [`tags`]: secondary structures.
//! - [`writing_app`]: app-name + version-number parser.
//! - [`identify`]: finalises the populated `MediaMetadata`.

pub mod attachments;
pub mod chapters;
pub mod ebml;
pub mod ids;
pub mod identify;
pub mod info;
pub mod reader;
pub mod seek_head;
pub mod tags;
pub mod tracks;
pub mod writing_app;

pub use reader::MatroskaReader;
