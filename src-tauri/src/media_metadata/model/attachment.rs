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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// One attached file (Matroska Attachments, MP4 mdat-by-handler, ...).  The
/// payload itself is **not** read — see plan §15.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// 1-based sequence number in the file (matches `mkvmerge -J` ordering).
    pub id: u32,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub description: Option<String>,
    /// Bytes of the payload.  We seek past, not read.
    #[specta(type = Number)]
    pub size: u64,
    /// Hex-encoded UID when the source format provides one (Matroska FileUID).
    pub uid_hex: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let a = Attachment {
            id: 1,
            file_name: "cover.jpg".to_owned(),
            mime_type: Some("image/jpeg".to_owned()),
            description: Some("Front cover".to_owned()),
            size: 12_345,
            uid_hex: Some("deadbeef".to_owned()),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"fileName\":\"cover.jpg\""));
        assert!(s.contains("\"mimeType\":\"image/jpeg\""));
        assert!(s.contains("\"uidHex\":\"deadbeef\""));
        let back: Attachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn nulls_omittable_via_option() {
        let a = Attachment {
            id: 2,
            file_name: "data.bin".to_owned(),
            mime_type: None,
            description: None,
            size: 0,
            uid_hex: None,
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"mimeType\":null"));
        let back: Attachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back, a);
    }
}
