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

//! Background parsing and the profile's automation.
//!
//! Every listed file that is not parsed, not being parsed and has not failed
//! is parsed once on the background executor. Once all members of a unit are
//! parsed, its automation runs in this order: language and track name per new
//! file, then the unit's default/forced flags scoped to the tracks the profile
//! will select, then the selection is committed. It runs once per unit (its
//! member list): again only for files without a selection, or when the unit's
//! membership changes. Existing selections are never replaced.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bmm_core::languages::build_track_name_options;
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::{ParseError, ParseOptions};
use bmm_core::merge_args::TrackSelector;
use bmm_core::protocol::MediaMetadataErrorPayload;
use gpui_kit::{AppContext, BackgroundExecutor, Context};

use super::FileStore;
use crate::domain::file_tree::{FlagKind, apply_unit_flag_automation};
use crate::domain::grouping::RenderEntry;

/// How long past the parse budget a read may take before its watchdog fails
/// the file: the budget is checked between reads, so a parse can overrun it
/// a little; one that overruns by this much is stuck in a read.
pub const WATCHDOG_MARGIN: Duration = Duration::from_secs(5);

/// Waits until `deadline` on the scheduler's own timer, which fires from the
/// dispatcher's timer thread and wakes the waiting task directly.
/// `BackgroundExecutor::timer` runs that timer inside a background task, so
/// it also needs a free background thread to finish, and blocked reads can
/// hold every one of them (Linux has a fixed pool).
pub(super) async fn sleep_until(executor: &BackgroundExecutor, deadline: Instant) {
  let left = deadline.saturating_duration_since(executor.now());
  if !left.is_zero() {
    executor.scheduler_executor().timer(left).await;
  }
}

/// Reads a file's metadata: the native parser in the app, a stand-in in
/// tests. Called on the background executor.
#[derive(Clone)]
pub struct MetadataSource(Arc<ReadFn>);

type ReadFn = dyn Fn(&str, &ParseOptions) -> Result<MediaMetadata, ParseError> + Send + Sync;

impl MetadataSource {
  pub fn new(read: impl Fn(&str, &ParseOptions) -> Result<MediaMetadata, ParseError> + Send + Sync + 'static) -> Self {
    Self(Arc::new(read))
  }

  /// The native header parser (`get_media_metadata`).
  pub fn parser() -> Self {
    Self::new(|file, options| bmm_core::services::read_media_metadata(file.to_string(), options.clone()))
  }

  pub(crate) fn read(&self, file: &str, options: &ParseOptions) -> Result<MediaMetadata, ParseError> {
    (self.0)(file, options)
  }
}

/// Where a file, or a unit, stands in loading.
#[derive(Clone, Copy, Debug)]
pub enum LoadState<'a> {
  Loading,
  Loaded,
  Failed(&'a MediaMetadataErrorPayload),
}

impl FileStore {
  pub fn load_state(&self, file: &str) -> LoadState<'_> {
    if self.file_tracks.contains_key(file) {
      LoadState::Loaded
    } else if let Some(error) = self.load_errors.get(file) {
      LoadState::Failed(error)
    } else {
      LoadState::Loading
    }
  }

  /// A unit's state as its card shows it: loading while a member loads, else
  /// the first member's error, else loaded.
  pub fn unit_load_state(&self, members: &[String]) -> LoadState<'_> {
    let states: Vec<LoadState<'_>> = members.iter().map(|file| self.load_state(file)).collect();
    if states.iter().any(|state| matches!(state, LoadState::Loading)) {
      return LoadState::Loading;
    }
    states
      .into_iter()
      .find(|state| matches!(state, LoadState::Failed(_)))
      .unwrap_or(LoadState::Loaded)
  }

  /// How many times a unit's automation ran.
  #[cfg(test)]
  pub(crate) fn automation_runs(&self) -> usize {
    self.automation_runs
  }

  /// Starts parsing every listed file that is not parsed, in flight or failed.
  /// Each parse has a watchdog ([`Self::give_up_load`]): the parser's own
  /// budget is checked between reads, so a read that blocks (a network share
  /// that stopped answering) would otherwise keep the file loading for as
  /// long as the OS takes. The watchdog's time runs from the moment a
  /// background thread begins the read: a large drop queues more parses than
  /// there are threads, and the time a parse waits for one is not the file's.
  /// The watchdog itself needs no background thread ([`sleep_until`]), so it
  /// fires also when blocked reads hold all of them.
  pub(super) fn load_pending(&mut self, cx: &mut Context<Self>) {
    let pending: Vec<String> = self
      .files
      .iter()
      .filter(|file| {
        !self.file_tracks.contains_key(*file)
          && !self.in_flight.contains_key(*file)
          && !self.load_errors.contains_key(*file)
      })
      .cloned()
      .collect();
    let limit = Duration::from_millis(self.settings.parse_options.timeout_ms) + WATCHDOG_MARGIN;
    for file in pending {
      self.next_load_attempt += 1;
      let attempt = self.next_load_attempt;
      self.in_flight.insert(file.clone(), attempt);
      let (source, options) = (self.source.clone(), self.settings.parse_options.clone());
      // The read sends the instant it begins, and sets `returned` once it
      // has: from then on its result is on its way.
      let (began, read_began) = async_channel::bounded(1);
      let returned = Arc::new(AtomicBool::new(false));
      let read_returned = Arc::clone(&returned);
      let parsed_file = file.clone();
      cx.spawn(async move |this, cx| {
        let executor = cx.background_executor().clone();
        let parsed = {
          let file = parsed_file.clone();
          cx.background_spawn(async move {
            let _ = began.try_send(executor.now());
            let parsed = source.read(&file, &options);
            read_returned.store(true, Ordering::Release);
            parsed
          })
          .await
        };
        let _ = this.update(cx, |store, cx| store.finish_load(parsed_file, attempt, parsed, cx));
      })
      .detach();
      cx.spawn(async move |this, cx| {
        // A parse dropped before its read began has nothing to watch.
        let Ok(began) = read_began.recv().await else {
          return;
        };
        sleep_until(cx.background_executor(), began + limit).await;
        if returned.load(Ordering::Acquire) {
          return;
        }
        let _ = this.update(cx, |store, cx| store.give_up_load(&file, attempt, cx));
      })
      .detach();
    }
  }

  /// The watchdog of parse `attempt` of `file`: a read still running after
  /// the budget and [`WATCHDOG_MARGIN`] fails the file as timed out, and its
  /// result is ignored if it ever arrives. The read itself cannot be
  /// interrupted; it ends when the OS gives up.
  pub(super) fn give_up_load(&mut self, file: &str, attempt: u64, cx: &mut Context<Self>) {
    if self.in_flight.get(file) != Some(&attempt) {
      return;
    }
    self.in_flight.remove(file);
    let budget_ms = self.settings.parse_options.timeout_ms;
    log::warn!("Gave up parsing {file}: no result after {budget_ms} ms and the margin");
    self.load_errors.insert(
      file.to_string(),
      MediaMetadataErrorPayload::Timeout {
        budget_ms,
        stage: "read".to_string(),
        detail: "the file did not answer in time".to_string(),
      },
    );
    self.refresh(vec![file.to_string()], true, cx);
  }

  /// Stores a finished parse. A file removed meanwhile is dropped; one removed
  /// and added again keeps this result; the result of an attempt its watchdog
  /// gave up on is ignored. The refresh comes in a task of its own, so every
  /// result that arrived before it runs is refreshed at once: a folder of
  /// files is derived again once per batch of results, not once per file.
  pub(super) fn finish_load(
    &mut self,
    file: String,
    attempt: u64,
    parsed: Result<MediaMetadata, ParseError>,
    cx: &mut Context<Self>,
  ) {
    if self.in_flight.get(&file) != Some(&attempt) {
      return;
    }
    self.in_flight.remove(&file);
    if !self.files.contains(&file) {
      return;
    }
    match parsed {
      Ok(metadata) => self.store_metadata(&file, metadata),
      Err(error) => {
        log::warn!("Failed to parse {file}: {error}");
        self
          .load_errors
          .insert(file.clone(), MediaMetadataErrorPayload::from_parse_error(&error));
      }
    }
    self.loaded.push(file);
    if self.loaded.len() == 1 {
      cx.spawn(async move |this, cx| {
        let _ = this.update(cx, |store, cx| store.refresh_loaded(cx));
      })
      .detach();
    }
  }

  /// The refresh of the parses that finished since the last one.
  fn refresh_loaded(&mut self, cx: &mut Context<Self>) {
    let mut loaded = std::mem::take(&mut self.loaded);
    if loaded.is_empty() {
      return;
    }
    let listed: HashSet<&String> = self.files.iter().collect();
    loaded.retain(|file| listed.contains(file));
    self.refresh(loaded, true, cx);
  }

  /// Runs the automation of every unit that is completely parsed and not
  /// automated yet; the files it changed. A unit of a group card takes no
  /// custom order (group cards keep none), a single card's unit its own.
  pub(super) fn run_automation(&mut self) -> Vec<String> {
    let Some(profile) = self.settings.profile.clone() else {
      return Vec::new();
    };
    let selector = TrackSelector::new(&profile);
    let automation = profile.automation.clone();
    let (reset_default, reset_forced) = (
      automation.reset_default_track.enabled,
      automation.reset_forced_display.enabled,
    );
    let mut jobs: Vec<(Vec<String>, Option<Vec<String>>)> = Vec::new();
    for entry in &self.entries {
      match entry {
        RenderEntry::Single { members } => {
          let order = (members.len() > 1)
            .then(|| self.combined_track_orders.get(&members[0]).cloned())
            .flatten();
          jobs.push((members.clone(), order));
        }
        RenderEntry::Group { units, .. } => jobs.extend(units.iter().map(|unit| (unit.clone(), None))),
      }
    }
    let mut changed = Vec::new();
    for (unit, order) in jobs {
      if !unit.iter().all(|file| self.file_tracks.contains_key(file)) {
        continue;
      }
      let pending: Vec<String> = unit
        .iter()
        .filter(|file| !self.file_selected_ids.contains_key(*file))
        .cloned()
        .collect();
      if pending.is_empty() && self.automated_units.contains(&unit) {
        continue;
      }
      self.automated_units.insert(unit.clone());
      self.automation_runs += 1;

      for file in &pending {
        self.automate_file(file, &automation, |track_type, language| {
          build_track_name_options(Some(&profile), track_type, language)
            .into_iter()
            .next()
        });
      }
      // The selection the profile will make, so the flag steps only touch
      // tracks that will be checked; committed after them.
      let mut prospective: HashMap<String, Vec<String>> = HashMap::new();
      for file in &unit {
        let selected = if pending.contains(file) {
          self.file_tracks[file]
            .iter()
            .filter(|track| selector.selects(track))
            .map(|track| track.key())
            .collect()
        } else {
          self.file_selected_ids[file].clone()
        };
        prospective.insert(file.clone(), selected);
      }
      let edits = apply_unit_flag_automation(
        &unit,
        &self.file_tracks,
        &prospective,
        reset_default,
        reset_forced,
        order.as_deref(),
      );
      for edit in edits {
        self.edit_rows(std::slice::from_ref(&edit.file), &edit.keys, |track| match edit.kind {
          FlagKind::Default => track.default_track = edit.value,
          FlagKind::Forced => track.forced = edit.value,
        });
      }
      for file in pending {
        if let Some(selected) = prospective.remove(&file) {
          self.file_selected_ids.insert(file, selected);
        }
      }
      changed.extend(unit);
    }
    changed
  }
}
