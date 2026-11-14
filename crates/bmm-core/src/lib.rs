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

//! UI-free core of BatchMkvMerge.
//!
//! Everything here is shared by the Tauri shell (`src-tauri`) and, from the
//! gpui-kit migration on, the native application. Nothing in this crate
//! depends on a GUI toolkit or an async runtime: long-running work (merges,
//! update checks) runs on plain threads, and callers that need it off their
//! UI thread move the call there themselves.

pub mod config;
pub mod constants;
pub mod js;
pub mod languages;
pub mod media_metadata;
pub mod media_track;
pub mod merge;
pub mod merge_args;
pub mod mkvtoolnix;
pub mod protocol;
pub mod services;
pub mod update;
