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

//! iTunes-style metadata.  `udta` → `meta` → `ilst` is the canonical path
//! used by `mp4tags` and friends.  Each `ilst` child is a 4-byte tag (often
//! prefixed with the © sentinel = 0xA9) wrapping a `data` box that carries
//! the actual value plus a type code.

pub mod ilst;
pub mod udta;
