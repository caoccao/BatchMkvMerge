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

//! GitHub release check and the startup update policy.
//!
//! The app version is passed in by the shell (`env!("CARGO_PKG_VERSION")` of
//! the binary crate), so this crate's own version never leaks into the check.

use anyhow::Result;

use crate::config::{self, Config, ConfigUpdate, UpdateCheckInterval};
use crate::constants::APP_NAME;
use crate::protocol::UpdateCheckResult;

const RELEASES_URL: &str = "https://api.github.com/repos/caoccao/BatchMkvMerge/releases";

/// What the shell does about updates at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupUpdateCheck {
  /// No check yet, or the last one is older than the configured interval:
  /// run [`run_check`] on a background thread.
  CheckNow,
  /// A recent check exists; its cached answer is the result.
  Cached(UpdateCheckResult),
}

/// Ask GitHub for the newest release and compare it with `app_version`.
/// Blocking network call.
pub fn check_for_updates(app_version: &str) -> Result<UpdateCheckResult> {
  log::info!("Checking for updates. Current version: {}", app_version);
  let resp = ureq::get(RELEASES_URL)
    .set("User-Agent", APP_NAME)
    .call()
    .map_err(|e| anyhow::anyhow!("Failed to fetch releases: {}", e))?;
  let json: serde_json::Value = resp
    .into_json()
    .map_err(|e| anyhow::anyhow!("Failed to parse releases: {}", e))?;
  if let Some(first) = json.as_array().and_then(|arr| arr.first()) {
    let tag = first["tag_name"].as_str().unwrap_or("");
    log::info!("Latest release tag: {}", tag);
    if is_newer_version(tag, app_version) {
      let version = tag.trim_start_matches('v').to_owned();
      return Ok(UpdateCheckResult {
        has_update: true,
        latest_version: Some(version),
      });
    }
  }
  Ok(no_update())
}

/// Seconds since the Unix epoch, or 0 if the clock is before it.
pub fn current_timestamp() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|duration| duration.as_secs() as i64)
    .unwrap_or(0)
}

/// Length of a check interval in seconds.
pub fn interval_seconds(interval: &UpdateCheckInterval) -> i64 {
  match interval {
    UpdateCheckInterval::Daily => 86_400,
    UpdateCheckInterval::Weekly => 604_800,
    UpdateCheckInterval::Monthly => 2_592_000,
  }
}

/// Numeric dotted-version comparison; a leading `v` is ignored and missing
/// parts count as 0.
pub fn is_newer_version(latest: &str, current: &str) -> bool {
  let latest = latest.trim_start_matches('v');
  let current = current.trim_start_matches('v');
  let latest_parts: Vec<u32> = latest.split('.').filter_map(|s| s.parse().ok()).collect();
  let current_parts: Vec<u32> = current.split('.').filter_map(|s| s.parse().ok()).collect();
  let len = latest_parts.len().max(current_parts.len());
  for i in 0..len {
    let l = latest_parts.get(i).copied().unwrap_or(0);
    let c = current_parts.get(i).copied().unwrap_or(0);
    if l > c {
      return true;
    }
    if l < c {
      return false;
    }
  }
  false
}

/// Run one check and record it in the config (`lastChecked`, `lastVersion`).
/// Never fails: a network error or a panic yields "no update". A found update
/// the user chose to skip is reported as "no update". Blocking network call.
pub fn run_check(app_version: &str) -> UpdateCheckResult {
  let check_result = std::panic::catch_unwind(|| check_for_updates(app_version)).unwrap_or_else(|_| {
    log::error!("Update check panicked");
    Err(anyhow::anyhow!("Update check panicked"))
  });
  match check_result {
    Ok(update_result) => {
      let mut updated_config = config::get_config();
      let result = record_check(&mut updated_config.update, update_result, current_timestamp());
      let _ = config::set_config(updated_config);
      result
    }
    Err(error) => {
      log::warn!("Update check failed: {}", error);
      no_update()
    }
  }
}

/// Records a finished check in `update` (`lastChecked` = `now`, and
/// `lastVersion` when a newer release was found) and returns what to show:
/// the update, unless it is the version the user skipped.
pub fn record_check(update: &mut ConfigUpdate, result: UpdateCheckResult, now: i64) -> UpdateCheckResult {
  update.last_checked = now;
  if let Some(ref version) = result.latest_version {
    update.last_version = version.clone();
  }
  without_ignored_version(result, &update.ignore_version)
}

/// Remember `version` as the one the user does not want to be told about.
pub fn skip_version(version: String) -> Result<()> {
  let mut config = config::get_config();
  config.update.ignore_version = version;
  config::set_config(config)
}

/// Startup policy: check now when the interval has elapsed, otherwise reuse
/// the version found by the last check. `now` is [`current_timestamp`].
pub fn startup_check(config: &Config, now: i64, app_version: &str) -> StartupUpdateCheck {
  let update = &config.update;
  if update.last_checked == 0 || now - update.last_checked > interval_seconds(&update.check_interval) {
    StartupUpdateCheck::CheckNow
  } else if !update.last_version.is_empty()
    && is_newer_version(&update.last_version, app_version)
    && update.last_version != update.ignore_version
  {
    StartupUpdateCheck::Cached(UpdateCheckResult {
      has_update: true,
      latest_version: Some(update.last_version.clone()),
    })
  } else {
    StartupUpdateCheck::Cached(no_update())
  }
}

fn no_update() -> UpdateCheckResult {
  UpdateCheckResult {
    has_update: false,
    latest_version: None,
  }
}

fn without_ignored_version(result: UpdateCheckResult, ignore_version: &str) -> UpdateCheckResult {
  if result.has_update && !ignore_version.is_empty() && result.latest_version.as_deref() == Some(ignore_version) {
    no_update()
  } else {
    result
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const NOW: i64 = 1_800_000_000;

  fn config_checked(seconds_ago: i64, last_version: &str, ignore_version: &str) -> Config {
    let mut config = Config::default();
    config.update.check_interval = UpdateCheckInterval::Weekly;
    config.update.last_checked = NOW - seconds_ago;
    config.update.last_version = last_version.to_owned();
    config.update.ignore_version = ignore_version.to_owned();
    config
  }

  fn found(version: &str) -> UpdateCheckResult {
    UpdateCheckResult {
      has_update: true,
      latest_version: Some(version.to_owned()),
    }
  }

  #[test]
  fn newer_version_compares_numerically_and_ignores_v_prefix() {
    assert!(is_newer_version("v0.3.0", "0.2.0"));
    assert!(is_newer_version("0.10.0", "0.9.9"));
    assert!(is_newer_version("1.0", "0.9.9"));
    assert!(is_newer_version("0.2.0.1", "0.2.0"));
    assert!(!is_newer_version("0.2.0", "v0.2.0"));
    assert!(!is_newer_version("0.2", "0.2.0"));
    assert!(!is_newer_version("0.1.9", "0.2.0"));
    assert!(!is_newer_version("", "0.2.0"));
  }

  #[test]
  fn interval_lengths_are_one_day_week_and_thirty_days() {
    assert_eq!(interval_seconds(&UpdateCheckInterval::Daily), 86_400);
    assert_eq!(interval_seconds(&UpdateCheckInterval::Weekly), 7 * 86_400);
    assert_eq!(interval_seconds(&UpdateCheckInterval::Monthly), 30 * 86_400);
  }

  #[test]
  fn startup_checks_when_never_checked() {
    let config = Config::default();
    assert_eq!(config.update.last_checked, 0);
    assert_eq!(startup_check(&config, NOW, "0.2.0"), StartupUpdateCheck::CheckNow);
  }

  #[test]
  fn startup_checks_when_interval_elapsed() {
    let config = config_checked(604_801, "0.3.0", "");
    assert_eq!(startup_check(&config, NOW, "0.2.0"), StartupUpdateCheck::CheckNow);
  }

  #[test]
  fn startup_reuses_newer_cached_version_within_interval() {
    let config = config_checked(604_800, "0.3.0", "");
    assert_eq!(
      startup_check(&config, NOW, "0.2.0"),
      StartupUpdateCheck::Cached(found("0.3.0"))
    );
  }

  #[test]
  fn startup_hides_cached_version_the_user_skipped() {
    let config = config_checked(60, "0.3.0", "0.3.0");
    assert_eq!(
      startup_check(&config, NOW, "0.2.0"),
      StartupUpdateCheck::Cached(no_update())
    );
  }

  #[test]
  fn startup_reports_no_update_when_cached_version_is_not_newer() {
    assert_eq!(
      startup_check(&config_checked(60, "0.2.0", ""), NOW, "0.2.0"),
      StartupUpdateCheck::Cached(no_update())
    );
    assert_eq!(
      startup_check(&config_checked(60, "", ""), NOW, "0.2.0"),
      StartupUpdateCheck::Cached(no_update())
    );
  }

  #[test]
  fn skipped_version_is_filtered_from_a_fresh_result() {
    assert_eq!(without_ignored_version(found("0.3.0"), "0.3.0"), no_update());
    assert_eq!(without_ignored_version(found("0.3.0"), "0.2.5"), found("0.3.0"));
    assert_eq!(without_ignored_version(found("0.3.0"), ""), found("0.3.0"));
    assert_eq!(without_ignored_version(no_update(), "0.3.0"), no_update());
  }

  #[test]
  fn current_timestamp_is_after_2020() {
    assert!(current_timestamp() > 1_577_836_800);
  }

  #[test]
  fn a_recorded_check_stores_the_time_and_version_and_hides_a_skipped_one() {
    let mut update = ConfigUpdate::default();
    assert_eq!(record_check(&mut update, found("0.3.0"), NOW), found("0.3.0"));
    assert_eq!((update.last_checked, update.last_version.as_str()), (NOW, "0.3.0"));
    update.ignore_version = "0.3.1".to_owned();
    assert_eq!(record_check(&mut update, found("0.3.1"), NOW + 1), no_update());
    assert_eq!((update.last_checked, update.last_version.as_str()), (NOW + 1, "0.3.1"));
    assert_eq!(record_check(&mut update, no_update(), NOW + 2), no_update());
    assert_eq!((update.last_checked, update.last_version.as_str()), (NOW + 2, "0.3.1"));
  }
}
