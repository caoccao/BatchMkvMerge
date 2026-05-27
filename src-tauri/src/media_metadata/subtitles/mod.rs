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

//! Native subtitle readers (text + image + segment-stream formats).
//!
//! - Text: SRT, SSA/ASS, WebVTT, USF, MicroDVD.
//! - Image / segment: VobSub (.idx + .sub sibling), HDMV PGS (.sup),
//!   HDMV TextST, VobButton.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

pub mod encoding;
pub mod hdmv_textst;
pub mod microdvd;
pub mod pgs;
pub mod srt;
pub mod ssa;
pub mod usf;
pub mod vobbtn;
pub mod vobsub;
pub mod webvtt;

pub use hdmv_textst::HdmvTextStReader;
pub use microdvd::MicroDvdReader;
pub use pgs::PgsReader;
pub use srt::SrtReader;
pub use ssa::SsaReader;
pub use usf::UsfReader;
pub use vobbtn::VobButtonReader;
pub use vobsub::VobSubReader;
pub use webvtt::WebVttReader;

pub(crate) fn read_source_to_end(
  src: &mut FileSource,
  deadline: Option<&Deadline>,
  stage: &'static str,
) -> Result<Vec<u8>, ParseError> {
  const CHUNK: usize = 64 * 1024;

  src.seek_to(0)?;
  let mut out = Vec::new();
  loop {
    if let Some(deadline) = deadline {
      deadline.check(stage)?;
    }
    let mut chunk = vec![0u8; CHUNK];
    let read = src.read_at_most(&mut chunk)?;
    if read == 0 {
      break;
    }
    out.extend_from_slice(&chunk[..read]);
    if read < CHUNK {
      break;
    }
  }
  src.seek_to(0)?;
  Ok(out)
}
