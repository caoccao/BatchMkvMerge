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

//! Native MP4 / QuickTime reader.
//!
//! Header-only port of `mkvtoolnix/src/input/r_qtmp4.cpp` — walks the ISO
//! BMFF / QuickTime box hierarchy via our own `media_metadata::io` helpers,
//! never depending on `mp4parse-rust` or any third-party muxer crate.
//!
//! Module layout:
//! - [`atom`]: generic box header walker (32-bit + 64-bit sizes, size-0 ⇒ EOF).
//! - [`ftyp`]: major + compatible brands; `Mp4` vs `QuickTime` disambiguation.
//! - [`moov`]: movie header tree (mvhd, trak, tkhd, mdia, mdhd, hdlr, stbl, edts).
//! - [`meta`]: iTunes metadata (ilst, udta).
//! - [`codec_specific`]: AVCC / HVCC / ESDS / colr / pasp / dvcC decoders.
//! - [`fragments`]: mvex/trex + moof/traf/tfhd/trun for fragmented MP4.
//! - [`reader`]: top-level `Reader` impl + read_headers pipeline.
//! - [`identify`]: defensive finalise step.

pub mod atom;
pub mod codec_specific;
pub mod fragments;
pub mod ftyp;
pub mod identify;
pub mod meta;
pub mod moov;
pub mod reader;
pub mod verify;

pub use reader::Mp4Reader;
