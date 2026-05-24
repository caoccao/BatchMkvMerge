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

//! Native elementary video stream readers (AVC, HEVC, MPEG-1/2 video, VC-1,
//! Dirac, DV, AV1 OBU).  Each reader sniffs raw byte streams without any
//! container, decoding sequence / SPS / VPS headers via the shared
//! [`crate::media_metadata::io::BitReader`] (exp-Golomb included).

pub mod avc;
pub mod dirac;
pub mod dv;
pub mod hevc;
pub mod mpeg_video;
pub mod obu;
pub mod vc1;

pub use avc::AvcReader;
pub use dirac::DiracReader;
pub use dv::DvReader;
pub use hevc::HevcReader;
pub use mpeg_video::MpegVideoReader;
pub use obu::ObuReader;
pub use vc1::Vc1Reader;
