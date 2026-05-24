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

/// Per-file parse context: the soft time budget plus the caller-supplied
/// allocation ceiling. Parsers call `check(stage)` at every coarse boundary
/// (start of each L1 EBML element, MP4 atom, Ogg page, TS packet group, etc.)
/// and clamp every file-controlled payload allocation to
/// [`max_element_size`](Self::max_element_size). Both knobs are supplied
/// per-call from the user's persisted config — see [[feedback-parser-timeout]].
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    start: Instant,
    budget: Duration,
    /// Hard ceiling for any single file-controlled payload read. Defaults to
    /// `u64::MAX` (unbounded) for [`Deadline::new`] / [`Deadline::from_parts`]
    /// so existing call sites are unaffected; the public `parse()` entry point
    /// overrides it from `ParseOptions::max_element_size`.
    max_element_size: u64,
}

impl Deadline {
    /// Build a deadline from a budget in milliseconds, with an unbounded
    /// element-size ceiling. A budget of 0 produces an instantly-expired
    /// deadline (useful in tests).
    pub fn new(budget_ms: u64) -> Self {
        Self::from_parts(Instant::now(), Duration::from_millis(budget_ms))
    }

    /// Inject the baseline `Instant` explicitly. Lets tests assert behaviour
    /// without sleeping. Element-size ceiling is unbounded.
    pub fn from_parts(start: Instant, budget: Duration) -> Self {
        Self {
            start,
            budget,
            max_element_size: u64::MAX,
        }
    }

    /// Builder-style override for the element-size ceiling. Used by the public
    /// `parse()` entry point to honour `ParseOptions::max_element_size`.
    pub fn with_max_element_size(mut self, max_element_size: u64) -> Self {
        self.max_element_size = max_element_size;
        self
    }

    /// The caller-supplied ceiling for any single file-controlled payload
    /// allocation. Readers should clamp their own hard-coded safety caps with
    /// this value (`cap.min(deadline.max_element_size())`).
    pub fn max_element_size(&self) -> u64 {
        self.max_element_size
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

    /// Build an [`Instant`] that is `offset` in the past.  On platforms where
    /// `Instant` is backed by a clock with a recent epoch (Windows QPC, fresh
    /// boots) plain subtraction underflows; `checked_sub` lets us fall back
    /// to the current `Instant`, which is harmless for tests that pair the
    /// baseline with a `ZERO` budget.
    fn past(offset: Duration) -> Instant {
        Instant::now().checked_sub(offset).unwrap_or_else(Instant::now)
    }

    #[test]
    fn from_parts_lets_tests_set_a_past_baseline() {
        // Past baseline + zero budget → guaranteed expired regardless of
        // whether the platform allows a real subtraction.
        let d = Deadline::from_parts(past(Duration::from_secs(3600)), Duration::ZERO);
        assert!(d.is_expired());
        let err = d.check("synthetic").unwrap_err();
        assert!(matches!(err, ParseError::Timeout { .. }));
    }

    #[test]
    fn remaining_saturates_at_zero_when_expired() {
        let d = Deadline::from_parts(past(Duration::from_secs(10)), Duration::ZERO);
        assert_eq!(d.remaining(), Duration::ZERO);
    }

    #[test]
    fn stage_label_propagates_unchanged() {
        let d = Deadline::new(0);
        let err = d.check("matroska::seek_head").unwrap_err();
        assert_eq!(err.stage(), "matroska::seek_head");
    }

    #[test]
    fn max_element_size_defaults_unbounded() {
        assert_eq!(Deadline::new(1000).max_element_size(), u64::MAX);
        assert_eq!(
            Deadline::from_parts(Instant::now(), Duration::from_millis(1)).max_element_size(),
            u64::MAX
        );
    }

    #[test]
    fn with_max_element_size_overrides_and_preserves_budget() {
        let d = Deadline::new(1234).with_max_element_size(16 * 1024 * 1024);
        assert_eq!(d.max_element_size(), 16 * 1024 * 1024);
        assert_eq!(d.budget_ms(), 1234);
    }
}
