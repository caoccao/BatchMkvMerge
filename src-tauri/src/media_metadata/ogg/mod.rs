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

//! Native Ogg / OGM reader.
//!
//! Header-only port of `mkvtoolnix/src/input/r_ogm.cpp`.  Demultiplexes pages
//! (per RFC 3533), reassembles packets across pages, and dispatches the
//! first packet of each bitstream into the codec-specific sniffers under
//! [`codecs`].  VorbisComment blocks are decoded by [`comments`].
//!
//! No dependency on the `ogg` crate — the page format is small enough to
//! roll our own.

pub mod codecs;
pub mod comments;
pub mod identify;
pub mod page;
pub mod reader;

pub use reader::OggReader;
