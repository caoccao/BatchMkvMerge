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

//! Flash Video (FLV) reader — port of `mkvtoolnix/src/input/r_flv.cpp`.
//!
//! The reader walks the first 1 MiB of the file looking for tags so we have
//! the codec FOURCC, width, height, channels and sample rate ready when
//! `read_headers` returns.  Cluster-equivalent payloads (the body of each
//! tag) are never decoded beyond what's needed for identification.

pub mod header;
pub mod reader;
pub mod script_data;
pub mod tag;

pub use reader::FlvReader;
