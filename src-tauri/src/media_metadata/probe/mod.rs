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

//! Probe cascade.  Mirrors mkvtoolnix's
//! `src/merge/reader_detection_and_creation.cpp::probe_file_format`
//! (a 6-phase cascade — unambiguous magics → extension hints → text
//! subtitles → strict elementary streams → frame-scan audio → ambiguous
//! formats).
//!
//! Phase 3 ships an active registry that only contains the Matroska reader —
//! other format readers are introduced in later phases and slot into the
//! same dispatch order.

pub mod dispatch;
pub mod extension_hint;
pub mod signatures;
pub mod unsupported;

pub use dispatch::{dispatch, registered_readers, DispatchOutcome};
pub use extension_hint::{is_supported_media_extension, FileTypeHint};
