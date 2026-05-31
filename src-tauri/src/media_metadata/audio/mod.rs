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

//! Native audio-only readers — header-only ports of mkvtoolnix's
//! `r_mp3.cpp / r_aac.cpp / r_ac3.cpp / r_dts.cpp / r_flac.cpp / r_wav.cpp
//! / r_truehd.cpp / r_tta.cpp / r_wavpack.cpp` plus the shared ID3v2
//! header skipper.

pub mod aac;
pub mod ac3;
pub mod dts;
pub mod flac;
pub mod id3v2;
pub mod mp3;
pub mod truehd;
pub mod tta;
pub mod wav;
pub mod wavpack;

pub use aac::AacReader;
pub use ac3::Ac3Reader;
pub use dts::DtsReader;
pub use flac::FlacReader;
pub use mp3::Mp3Reader;
pub use truehd::TrueHdReader;
pub use tta::TtaReader;
pub use wav::WavReader;
pub use wavpack::WavpackReader;
