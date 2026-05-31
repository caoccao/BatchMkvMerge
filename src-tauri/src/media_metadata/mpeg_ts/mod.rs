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

//! Native MPEG-TS (ISO/IEC 13818-1) reader.
//!
//! Header-only port of `mkvtoolnix/src/input/r_mpeg_ts.cpp`.  Demultiplexes
//! transport packets (188 / 192-byte BD M2TS / 204-byte FEC variants),
//! reassembles PAT + PMT sections to learn which PIDs carry which
//! elementary streams, and dispatches descriptors to enrich the per-PID
//! stream entry with codec details and language.

pub mod descriptors;
pub mod identify;
pub mod packet;
pub mod pat;
pub mod pes;
pub mod pmt;
pub mod reader;
pub mod stream_table;

pub use reader::MpegTsReader;
