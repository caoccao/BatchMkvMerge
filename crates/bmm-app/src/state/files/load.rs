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

//! Background parsing and the profile's automation (`useFilesLoad.ts`).
//!
//! Every listed file that is not parsed, not being parsed and has not failed
//! is parsed once on the background executor. Once all members of a unit are
//! parsed, its automation runs in the React order: language and track name per
//! new file, then the unit's default/forced flags scoped to the tracks the
//! profile will select, then the selection is committed. It runs once per unit
//! (its member list): again only for files without a selection, or when the
//! unit's membership changes. Existing selections are never replaced.

use std::collections::HashMap;
use std::sync::Arc;

use bmm_core::languages::build_track_name_options;
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::{ParseError, ParseOptions};
use bmm_core::merge_args::TrackSelector;
use bmm_core::protocol::MediaMetadataErrorPayload;
use gpui_kit::{AppContext, Context};

use super::FileStore;
use crate::domain::file_tree::{FlagKind, apply_unit_flag_automation};
use crate::domain::grouping::RenderEntry;

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
  pub(super) fn load_pending(&mut self, cx: &mut Context<Self>) {
    let pending: Vec<String> = self
      .files
      .iter()
      .filter(|file| {
        !self.file_tracks.contains_key(*file)
          && !self.in_flight.contains(*file)
          && !self.load_errors.contains_key(*file)
      })
      .cloned()
      .collect();
    for file in pending {
      self.in_flight.insert(file.clone());
      let (source, options) = (self.source.clone(), self.settings.parse_options.clone());
      cx.spawn(async move |this, cx| {
        let parsed = {
          let file = file.clone();
          cx.background_spawn(async move { source.read(&file, &options) }).await
        };
        let _ = this.update(cx, |store, cx| store.finish_load(file, parsed, cx));
      })
      .detach();
    }
  }

  /// Stores a finished parse. A file removed meanwhile is dropped; one removed
  /// and added again keeps this result.
  fn finish_load(&mut self, file: String, parsed: Result<MediaMetadata, ParseError>, cx: &mut Context<Self>) {
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
    self.refresh(vec![file], true, cx);
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
