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

//! RealMedia (`.rm`, `.rmvb`) reader — port of mkvtoolnix's
//! `r_real.cpp` + `lib/librmff/rmff.c`.  We walk the top-level chunk
//! hierarchy (`.RMF`, `PROP`, `MDPR`, `CONT`, `DATA`) and decode the
//! type-specific data inside each `MDPR` chunk to surface codec / sample
//! rate / dimensions for identification.

pub mod chunks;
pub mod reader;
pub mod stream_props;

pub use reader::RealMediaReader;
