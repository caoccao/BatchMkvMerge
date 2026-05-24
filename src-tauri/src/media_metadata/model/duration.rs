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

/// A duration carried as both nanoseconds and a pre-formatted string so the
/// frontend never has to format integers itself.  The formatted form is
/// `HH:MM:SS.fffffffff` (always nine fractional digits, no rounding).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DurationValue {
    #[specta(type = Number)]
    pub ns: u64,
    pub formatted: String,
}

impl DurationValue {
    pub fn from_ns(ns: u64) -> Self {
        Self {
            formatted: format_ns(ns),
            ns,
        }
    }
}

fn format_ns(ns: u64) -> String {
    const NS_PER_SEC: u64 = 1_000_000_000;
    let total_seconds = ns / NS_PER_SEC;
    let sub = ns % NS_PER_SEC;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}.{:09}", hours, minutes, seconds, sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_renders_as_zero() {
        let d = DurationValue::from_ns(0);
        assert_eq!(d.ns, 0);
        assert_eq!(d.formatted, "00:00:00.000000000");
    }

    #[test]
    fn one_hour_two_minutes_three_seconds() {
        let ns = ((1 * 3600 + 2 * 60 + 3) * 1_000_000_000) + 456_789_012;
        let d = DurationValue::from_ns(ns);
        assert_eq!(d.formatted, "01:02:03.456789012");
    }

    #[test]
    fn nanosecond_precision_preserved() {
        let d = DurationValue::from_ns(1);
        assert_eq!(d.formatted, "00:00:00.000000001");
    }

    #[test]
    fn hours_can_exceed_24() {
        let d = DurationValue::from_ns(99 * 3_600 * 1_000_000_000);
        assert_eq!(d.formatted, "99:00:00.000000000");
    }

    #[test]
    fn round_trip_via_serde_json() {
        let d = DurationValue::from_ns(1_234_567_890);
        let s = serde_json::to_string(&d).unwrap();
        assert!(s.contains("\"ns\":1234567890"));
        assert!(s.contains("\"formatted\":\"00:00:01.234567890\""));
        let back: DurationValue = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }
}
