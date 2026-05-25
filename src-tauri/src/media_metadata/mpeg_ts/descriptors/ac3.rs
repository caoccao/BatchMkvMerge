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

//! AC-3 audio descriptor (tag 0x6A) — DVB and ATSC use slightly different
//! shapes, but for identification the bare presence of this descriptor is
//! enough to flag the stream as AC-3.  The body is allowed to be empty.

pub fn decode(_body: &[u8]) -> bool {
  true
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn presence_alone_is_sufficient_to_flag_ac3() {
    assert!(decode(&[]));
    assert!(decode(&[0xAA, 0xBB]));
  }
}
