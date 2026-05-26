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

//! Native AVI reader.
//!
//! Header-only port of `mkvtoolnix/src/input/r_avi.cpp`.  Walks the RIFF
//! container directly through our own [`crate::media_metadata::io`] helpers
//! — we deliberately do not depend on `avilib` (mkvtoolnix's C dependency)
//! since identification needs only a handful of chunks.

pub mod avih;
pub mod identify;
pub mod mpeg4_par;
pub mod odml;
pub mod reader;
pub mod riff;
pub mod strl;
pub mod subtitles;

pub use reader::AviReader;
