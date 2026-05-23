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

use std::time::{Duration, Instant};

use super::error::ParseError;

/// Soft per-file parse budget. Parsers call `check(stage)` at every coarse
/// boundary (start of each L1 EBML element, MP4 atom, Ogg page, TS packet
/// group, etc.). The budget is supplied per-call from the user's persisted
/// config — see [[feedback-parser-timeout]].
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    start: Instant,
    budget: Duration,
}

impl Deadline {
    /// Build a deadline from an `Instant` baseline and a budget in milliseconds.
    /// A budget of 0 produces an instantly-expired deadline (useful in tests).
    pub fn new(budget_ms: u64) -> Self {
        Self::from_parts(Instant::now(), Duration::from_millis(budget_ms))
    }

    /// Inject the baseline `Instant` explicitly. Lets tests assert behaviour
    /// without sleeping.
    pub fn from_parts(start: Instant, budget: Duration) -> Self {
        Self { start, budget }
    }

    /// `Err(Timeout)` once the elapsed time crosses the budget. Cost per call
    /// is roughly the cost of one `Instant::now()` (~5 ns on modern hardware).
    pub fn check(&self, stage: &'static str) -> Result<(), ParseError> {
        if self.start.elapsed() >= self.budget {
            Err(ParseError::Timeout {
                budget_ms: self.budget_ms(),
                stage,
            })
        } else {
            Ok(())
        }
    }

    /// Time remaining; saturates at zero rather than panicking on overflow.
    pub fn remaining(&self) -> Duration {
        self.budget.saturating_sub(self.start.elapsed())
    }

    /// Whether the budget is exhausted. Cheaper than calling `check` if the
    /// caller only needs a bool.
    pub fn is_expired(&self) -> bool {
        self.start.elapsed() >= self.budget
    }

    /// Budget in milliseconds (clamped to `u64::MAX`).
    pub fn budget_ms(&self) -> u64 {
        let m = self.budget.as_millis();
        if m > u64::MAX as u128 {
            u64::MAX
        } else {
            m as u64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn zero_budget_is_immediately_expired() {
        let d = Deadline::new(0);
        assert!(d.is_expired());
        let err = d.check("test").unwrap_err();
        match err {
            ParseError::Timeout { budget_ms, stage } => {
                assert_eq!(budget_ms, 0);
                assert_eq!(stage, "test");
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn generous_budget_does_not_fire() {
        let d = Deadline::new(60_000);
        assert!(!d.is_expired());
        assert!(d.check("anywhere").is_ok());
        assert!(d.remaining() > Duration::from_secs(30));
    }

    #[test]
    fn budget_ms_roundtrips() {
        assert_eq!(Deadline::new(1234).budget_ms(), 1234);
        assert_eq!(Deadline::new(0).budget_ms(), 0);
    }

    #[test]
    fn small_budget_expires_after_sleeping_past_it() {
        let d = Deadline::new(10);
        thread::sleep(Duration::from_millis(30));
        assert!(d.is_expired());
        assert!(d.check("late").is_err());
    }

    #[test]
    fn from_parts_lets_tests_set_a_past_baseline() {
        // 1 hour in the past with a 1-second budget — instantly expired.
        let past = Instant::now() - Duration::from_secs(3600);
        let d = Deadline::from_parts(past, Duration::from_secs(1));
        assert!(d.is_expired());
        let err = d.check("synthetic").unwrap_err();
        assert!(matches!(err, ParseError::Timeout { .. }));
    }

    #[test]
    fn remaining_saturates_at_zero_when_expired() {
        let past = Instant::now() - Duration::from_secs(10);
        let d = Deadline::from_parts(past, Duration::from_millis(1));
        assert_eq!(d.remaining(), Duration::ZERO);
    }

    #[test]
    fn stage_label_propagates_unchanged() {
        let d = Deadline::new(0);
        let err = d.check("matroska::seek_head").unwrap_err();
        assert_eq!(err.stage(), "matroska::seek_head");
    }
}
