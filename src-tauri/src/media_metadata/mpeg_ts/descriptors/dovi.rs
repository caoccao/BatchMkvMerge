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

//! Dolby Vision video stream descriptor (tag 0xB0).  Body:
//!
//! ```text
//! u8 dv_version_major
//! u8 dv_version_minor
//! 7  dv_profile | 1  dv_level_msb (7 high bits of level)
//! ... more (level, flags, BL/EL/RPU presence)
//! ```
//!
//! Identification only needs the profile.

pub fn decode(body: &[u8]) -> Option<u32> {
    if body.len() < 3 {
        return None;
    }
    let profile = (body[2] >> 1) & 0x7F;
    Some(profile as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_profile_5() {
        let body = [1u8, 0, (5 << 1) & 0xFE];
        assert_eq!(decode(&body), Some(5));
    }

    #[test]
    fn extracts_profile_8() {
        let body = [1u8, 0, (8 << 1) & 0xFE];
        assert_eq!(decode(&body), Some(8));
    }

    #[test]
    fn rejects_truncated_body() {
        assert!(decode(&[1u8, 0]).is_none());
    }
}
