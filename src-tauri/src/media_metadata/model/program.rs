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

/// One MPEG-TS / MPEG-PS program (a logical sub-mux).  Most files have
/// exactly one program; multi-program TS files (broadcast captures) report
/// every program here so the frontend can filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Program {
    pub program_number: u32,
    pub pmt_pid: Option<u32>,
    pub service_name: Option<String>,
    pub service_provider: Option<String>,
    /// Track IDs (matching [`super::track::Track::id`]) carried by this
    /// program.
    #[specta(type = Vec<Number>)]
    pub track_ids: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_json() {
        let p = Program {
            program_number: 1,
            pmt_pid: Some(0x100),
            service_name: Some("BBC One HD".to_owned()),
            service_provider: Some("BBC".to_owned()),
            track_ids: vec![0, 1, 2],
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"programNumber\":1"));
        assert!(s.contains("\"pmtPid\":256"));
        assert!(s.contains("\"trackIds\":[0,1,2]"));
        let back: Program = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }
}
