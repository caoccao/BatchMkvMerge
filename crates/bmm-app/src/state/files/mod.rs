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

//! The dropped files and what is known about them: the parsed metadata and its
//! table rows, the track selection, flag, language and name edits, the output
//! paths, manual card merges and detaches.
//!
//! The card units ([`build_merge_units`]) and the file list's entries
//! ([`build_render_entries`]) are derived after every change. A change
//! notifies observers only when it is structural: files added or removed, a
//! unit or group formed or split, a file parsed or failed, the first track
//! selected or the last one cleared. Every change also emits
//! [`FileEvent::Changed`] naming the files, so a card can repaint alone.
//!
//! Parsing and the profile's automation live in [`load`]; the track tables'
//! edits, which fan out over a group card's units, in [`table`].

pub mod load;
pub mod table;

use std::collections::{HashMap, HashSet};

use bmm_core::config::{Config, ConfigAutomation, ConfigProfile, GroupMode};
use bmm_core::languages::language_code_aliases;
use bmm_core::media_metadata::ParseOptions;
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::{
  MediaTrack, MediaTrackCounts, MediaTrackKind, MediaTrackType, media_track_counts, metadata_to_media_tracks,
};
use bmm_core::protocol::MediaMetadataErrorPayload;
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Subscription};

pub use load::{LoadState, MetadataSource};
pub use table::TableScope;

use super::config::{ConfigStore, active_profile};
use crate::domain::file_tree::{FlagKind, build_merge_units};
use crate::domain::grouping::{RenderEntry, build_render_entries};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEvent {
  /// Data of these files changed: rows, selection, flags, output paths or the
  /// load state.
  Changed { files: Vec<String> },
  /// The drop-target or the active-card highlight moved between these cards
  /// (unit roots or group keys).
  Highlight { ids: Vec<String> },
}

/// What the store takes from the config: how files form units and group
/// cards, the active profile (selection and automation) and the parser
/// options.
#[derive(Clone, Debug)]
pub struct FileSettings {
  pub group_by_file_name: bool,
  pub group_mode: GroupMode,
  pub profile: Option<ConfigProfile>,
  pub parse_options: ParseOptions,
}

impl FileSettings {
  pub fn from_config(config: &Config) -> Self {
    Self {
      group_by_file_name: config.group_by_file_name,
      group_mode: config.group_mode.clone(),
      profile: active_profile(config).cloned(),
      parse_options: bmm_core::services::parser_options_from_config(config),
    }
  }
}

pub struct FileStore {
  /// The files in the order they were added.
  files: Vec<String>,
  /// Raw parser output, for the card summaries.
  file_metadata: HashMap<String, MediaMetadata>,
  /// The table rows, edited in place by the track table.
  file_tracks: HashMap<String, Vec<MediaTrack>>,
  file_track_counts: HashMap<String, MediaTrackCounts>,
  /// Selected row keys per file; absent until the profile's selection ran.
  file_selected_ids: HashMap<String, Vec<String>>,
  /// Output directory overrides (group cards).
  file_output_dirs: HashMap<String, String>,
  /// Full output path overrides of single-root cards; they win over the
  /// directories and are used as given.
  file_output_paths: HashMap<String, String>,
  /// Manual card merges: absorbed root → the root it was dropped on.
  merged_roots: HashMap<String, String>,
  /// Files pulled out of their unit into a card of their own.
  detached_files: HashSet<String>,
  /// Custom combined track order (combined ids) of a multi-member unit, by
  /// its root.
  combined_track_orders: HashMap<String, Vec<String>>,
  /// The single-root card hovered as a valid drop target of a card drag.
  drop_target_root: Option<String>,
  /// Session-only default output directory (not persisted).
  global_output_dir: Option<String>,
  /// The card the keyboard acts on (a unit root or a group key).
  active_card: Option<String>,
  /// Where BetterMediaInfo was detected; `None` hides the cards' button.
  better_media_info: Option<String>,
  /// Why a file could not be parsed; it is not tried again until removed.
  load_errors: HashMap<String, MediaMetadataErrorPayload>,
  /// Files being parsed on the background executor.
  in_flight: HashSet<String>,
  /// Units whose automation ran, by their member list.
  automated_units: HashSet<Vec<String>>,
  /// How many times a unit's automation ran (tests).
  automation_runs: usize,
  units: Vec<Vec<String>>,
  entries: Vec<RenderEntry>,
  has_selection: bool,
  settings: FileSettings,
  source: MetadataSource,
  _config: Option<Subscription>,
}

impl EventEmitter<FileEvent> for FileStore {}

impl FileStore {
  /// A store that follows `config` and parses with `source`.
  pub fn new(config: &Entity<ConfigStore>, source: MetadataSource, cx: &mut Context<Self>) -> Self {
    let settings = FileSettings::from_config(config.read(cx).config());
    let mut store = Self::with_settings(settings, source);
    store._config = Some(cx.observe(config, |store, config, cx| {
      let settings = FileSettings::from_config(config.read(cx).config());
      store.set_settings(settings, cx);
    }));
    store
  }

  /// A store with fixed settings, not following a config.
  pub fn with_settings(settings: FileSettings, source: MetadataSource) -> Self {
    Self {
      files: Vec::new(),
      file_metadata: HashMap::new(),
      file_tracks: HashMap::new(),
      file_track_counts: HashMap::new(),
      file_selected_ids: HashMap::new(),
      file_output_dirs: HashMap::new(),
      file_output_paths: HashMap::new(),
      merged_roots: HashMap::new(),
      detached_files: HashSet::new(),
      combined_track_orders: HashMap::new(),
      drop_target_root: None,
      global_output_dir: None,
      active_card: None,
      better_media_info: None,
      load_errors: HashMap::new(),
      in_flight: HashSet::new(),
      automated_units: HashSet::new(),
      automation_runs: 0,
      units: Vec::new(),
      entries: Vec::new(),
      has_selection: false,
      settings,
      source,
      _config: None,
    }
  }

  pub fn files(&self) -> &[String] {
    &self.files
  }

  pub fn has_files(&self) -> bool {
    !self.files.is_empty()
  }

  pub fn settings(&self) -> &FileSettings {
    &self.settings
  }

  pub fn metadata(&self, file: &str) -> Option<&MediaMetadata> {
    self.file_metadata.get(file)
  }

  pub fn tracks(&self, file: &str) -> Option<&[MediaTrack]> {
    self.file_tracks.get(file).map(Vec::as_slice)
  }

  /// Every parsed file's rows, as the domain functions take them.
  pub fn file_tracks(&self) -> &HashMap<String, Vec<MediaTrack>> {
    &self.file_tracks
  }

  pub fn track_counts(&self, file: &str) -> Option<MediaTrackCounts> {
    self.file_track_counts.get(file).copied()
  }

  pub fn selected_ids(&self, file: &str) -> Option<&[String]> {
    self.file_selected_ids.get(file).map(Vec::as_slice)
  }

  pub fn file_selected_ids(&self) -> &HashMap<String, Vec<String>> {
    &self.file_selected_ids
  }

  /// The selected rows of `file` in table order.
  pub fn selected_tracks(&self, file: &str) -> Vec<MediaTrack> {
    let (Some(tracks), Some(selected)) = (self.file_tracks.get(file), self.file_selected_ids.get(file)) else {
      return Vec::new();
    };
    let selected: HashSet<&str> = selected.iter().map(String::as_str).collect();
    tracks
      .iter()
      .filter(|track| selected.contains(track.key().as_str()))
      .cloned()
      .collect()
  }

  /// Whether any listed file has a selected row (Merge All's enabled state).
  pub fn has_selection(&self) -> bool {
    self.has_selection
  }

  pub fn output_dir(&self, file: &str) -> Option<&str> {
    self.file_output_dirs.get(file).map(String::as_str)
  }

  pub fn output_path(&self, file: &str) -> Option<&str> {
    self.file_output_paths.get(file).map(String::as_str)
  }

  pub fn global_output_dir(&self) -> Option<&str> {
    self.global_output_dir.as_deref()
  }

  pub fn combined_track_order(&self, root: &str) -> Option<&[String]> {
    self.combined_track_orders.get(root).map(Vec::as_slice)
  }

  pub fn merged_root(&self, file: &str) -> Option<&str> {
    self.merged_roots.get(file).map(String::as_str)
  }

  pub fn is_detached(&self, file: &str) -> bool {
    self.detached_files.contains(file)
  }

  pub fn drop_target_root(&self) -> Option<&str> {
    self.drop_target_root.as_deref()
  }

  pub fn active_card(&self) -> Option<&str> {
    self.active_card.as_deref()
  }

  pub fn better_media_info_available(&self) -> bool {
    self.better_media_info.is_some()
  }

  /// The directory BetterMediaInfo was detected in.
  pub fn better_media_info_dir(&self) -> Option<&str> {
    self.better_media_info.as_deref()
  }

  /// The card units, root first (one card and one merge output each).
  pub fn units(&self) -> &[Vec<String>] {
    &self.units
  }

  /// The file list's cards: single units and group cards.
  pub fn entries(&self) -> &[RenderEntry] {
    &self.entries
  }

  /// Appends the paths that are not listed yet, in order, and starts parsing
  /// them.
  pub fn add_files(&mut self, paths: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
    let mut added = Vec::new();
    for path in paths {
      if !self.files.contains(&path) && !added.contains(&path) {
        added.push(path);
      }
    }
    if added.is_empty() {
      return;
    }
    self.files.extend(added.iter().cloned());
    self.refresh(added, true, cx);
  }

  /// Removes one file and everything about it, including manual merges naming
  /// it on either side. Its queue item goes with
  /// [`crate::actions::remove_file`].
  pub fn remove_file(&mut self, path: &str, cx: &mut Context<Self>) {
    let Some(index) = self.files.iter().position(|file| file == path) else {
      return;
    };
    self.files.remove(index);
    self.file_metadata.remove(path);
    self.file_tracks.remove(path);
    self.file_track_counts.remove(path);
    self.file_selected_ids.remove(path);
    self.combined_track_orders.remove(path);
    self.file_output_dirs.remove(path);
    self.file_output_paths.remove(path);
    self.merged_roots.retain(|source, dest| source != path && dest != path);
    self.detached_files.remove(path);
    self.load_errors.remove(path);
    self
      .automated_units
      .retain(|unit| !unit.iter().any(|file| file == path));
    if self.active_card.as_deref() == Some(path) {
      self.active_card = None;
    }
    self.refresh(vec![path.to_string()], true, cx);
  }

  /// Removes every file; the global output directory stays. The queue is
  /// cleared by [`crate::actions::clear_files`].
  pub fn clear_files(&mut self, cx: &mut Context<Self>) {
    if self.files.is_empty() {
      return;
    }
    let removed = std::mem::take(&mut self.files);
    self.file_metadata.clear();
    self.file_tracks.clear();
    self.file_track_counts.clear();
    self.file_selected_ids.clear();
    self.combined_track_orders.clear();
    self.file_output_dirs.clear();
    self.file_output_paths.clear();
    self.merged_roots.clear();
    self.detached_files.clear();
    self.load_errors.clear();
    self.automated_units.clear();
    self.active_card = None;
    self.refresh(removed, true, cx);
  }

  /// Drag-merges `source_root`'s card (its whole unit) into `dest_root`'s
  /// card. Re-merging undoes an earlier detach of the source.
  pub fn merge_card_into(&mut self, source_root: &str, dest_root: &str, cx: &mut Context<Self>) {
    if source_root == dest_root {
      return;
    }
    self.detached_files.remove(source_root);
    self.merged_roots.insert(source_root.to_string(), dest_root.to_string());
    self.refresh(vec![source_root.to_string(), dest_root.to_string()], false, cx);
  }

  /// Takes `file` out of its card: a drag-merged root is un-merged with its
  /// own unit, any other member becomes a card alone.
  pub fn detach_file_from_root(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.merged_roots.remove(file).is_none() {
      self.detached_files.insert(file.to_string());
    }
    self.refresh(vec![file.to_string()], false, cx);
  }

  /// Sets or clears the card hovered as a drop target.
  pub fn set_drop_target(&mut self, root: Option<String>, cx: &mut Context<Self>) {
    if self.drop_target_root != root {
      let previous = std::mem::replace(&mut self.drop_target_root, root.clone());
      cx.emit(FileEvent::Highlight {
        ids: previous.into_iter().chain(root).collect(),
      });
    }
  }

  /// Sets the card the keyboard acts on.
  pub fn set_active_card(&mut self, id: Option<String>, cx: &mut Context<Self>) {
    if self.active_card != id {
      let previous = std::mem::replace(&mut self.active_card, id.clone());
      cx.emit(FileEvent::Highlight {
        ids: previous.into_iter().chain(id).collect(),
      });
    }
  }

  /// Stores parsed `metadata` with its rows and counts. The profile's
  /// selection and automation follow once the whole unit is parsed.
  pub fn set_file_metadata(&mut self, file: &str, metadata: MediaMetadata, cx: &mut Context<Self>) {
    if !self.files.iter().any(|listed| listed == file) {
      return;
    }
    self.store_metadata(file, metadata);
    self.refresh(vec![file.to_string()], true, cx);
  }

  fn store_metadata(&mut self, file: &str, metadata: MediaMetadata) {
    let tracks = metadata_to_media_tracks(&metadata);
    self
      .file_track_counts
      .insert(file.to_string(), media_track_counts(&tracks));
    self.file_tracks.insert(file.to_string(), tracks);
    self.file_metadata.insert(file.to_string(), metadata);
    self.load_errors.remove(file);
  }

  /// Replaces the selected row keys of `file`.
  pub fn set_file_selected_ids(&mut self, file: &str, ids: Vec<String>, cx: &mut Context<Self>) {
    self.file_selected_ids.insert(file.to_string(), ids);
    self.refresh(vec![file.to_string()], false, cx);
  }

  /// Sets a flag to `value` on the rows `keys` of every file in `files`; the
  /// card computes the value of a click.
  pub fn set_track_flag(
    &mut self,
    files: &[String],
    keys: &[String],
    kind: FlagKind,
    value: TrackFlag,
    cx: &mut Context<Self>,
  ) {
    if keys.is_empty() {
      return;
    }
    let changed = self.edit_rows(files, keys, |track| match kind {
      FlagKind::Default => track.default_track = value,
      FlagKind::Forced => track.forced = value,
    });
    self.refresh(changed, false, cx);
  }

  /// Moves the `from_key` row to `to_key`'s position in every file of `files`
  /// that has both. Track ids are unchanged.
  pub fn reorder_tracks(&mut self, files: &[String], from_key: &str, to_key: &str, cx: &mut Context<Self>) {
    if from_key == to_key {
      return;
    }
    let mut changed = Vec::new();
    for file in files {
      let Some(tracks) = self.file_tracks.get_mut(file) else {
        continue;
      };
      let from = tracks.iter().position(|track| track.key() == from_key);
      let to = tracks.iter().position(|track| track.key() == to_key);
      if let (Some(from), Some(to)) = (from, to)
        && from != to
      {
        let moved = tracks.remove(from);
        tracks.insert(to, moved);
        changed.push(file.clone());
      }
    }
    self.refresh(changed, false, cx);
  }

  /// Sets the combined track order of a multi-member unit by its root, as
  /// combined ids.
  pub fn set_combined_track_order(&mut self, root: &str, order: Vec<String>, cx: &mut Context<Self>) {
    self.combined_track_orders.insert(root.to_string(), order);
    self.refresh(vec![root.to_string()], false, cx);
  }

  /// Sets the language of the rows `keys` in `files`.
  pub fn set_track_language(&mut self, files: &[String], keys: &[String], value: &str, cx: &mut Context<Self>) {
    let changed = self.edit_rows(files, keys, |track| set_language(track, value));
    self.refresh(changed, false, cx);
  }

  /// Sets the track name of the rows `keys` in `files`.
  pub fn set_track_name(&mut self, files: &[String], keys: &[String], value: &str, cx: &mut Context<Self>) {
    let changed = self.edit_rows(files, keys, |track| track.track_name = value.to_string());
    self.refresh(changed, false, cx);
  }

  /// The profile's per-track automation of one file: an `und` language becomes
  /// the reset language, then each track takes the first preset name of its
  /// (new) language. Chapters and attachments are left alone.
  pub fn apply_automation_to_file(
    &mut self,
    file: &str,
    automation: &ConfigAutomation,
    preset_for: impl Fn(MediaTrackType, &str) -> Option<String>,
    cx: &mut Context<Self>,
  ) {
    if self.automate_file(file, automation, preset_for) {
      self.refresh(vec![file.to_string()], false, cx);
    }
  }

  /// [`Self::apply_automation_to_file`] without the refresh; whether a row
  /// could have changed.
  fn automate_file(
    &mut self,
    file: &str,
    automation: &ConfigAutomation,
    preset_for: impl Fn(MediaTrackType, &str) -> Option<String>,
  ) -> bool {
    let reset_und = &automation.reset_und_language;
    let set_name = automation.set_track_name.enabled;
    if !reset_und.enabled && !set_name {
      return false;
    }
    let Some(tracks) = self.file_tracks.get_mut(file) else {
      return false;
    };
    for track in tracks.iter_mut().filter(|track| track.kind == MediaTrackKind::Track) {
      if reset_und.enabled && !reset_und.language.is_empty() && track.language == "und" {
        set_language(track, &reset_und.language);
      }
      if set_name && let Some(preset) = preset_for(track.track_type, &track.language) {
        track.track_name = preset;
      }
    }
    true
  }

  /// Sets a single-root card's full output path; it replaces a directory
  /// override of the file.
  pub fn set_file_output_path(&mut self, file: &str, path: String, cx: &mut Context<Self>) {
    self.file_output_dirs.remove(file);
    self.file_output_paths.insert(file.to_string(), path);
    self.refresh(vec![file.to_string()], false, cx);
  }

  /// Clears a single-root card's full output path.
  pub fn clear_file_output_path(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.file_output_paths.remove(file).is_some() {
      self.refresh(vec![file.to_string()], false, cx);
    }
  }

  /// Sets the output directory of every file of a group card.
  pub fn set_group_output_dir(&mut self, files: &[String], dir: &str, cx: &mut Context<Self>) {
    for file in files {
      self.file_output_dirs.insert(file.clone(), dir.to_string());
    }
    self.refresh(files.to_vec(), false, cx);
  }

  /// Clears the output directory of every file of a group card.
  pub fn clear_group_output_dir(&mut self, files: &[String], cx: &mut Context<Self>) {
    for file in files {
      self.file_output_dirs.remove(file);
    }
    self.refresh(files.to_vec(), false, cx);
  }

  /// Sets the session's default output directory; an empty one clears it.
  pub fn set_global_output_dir(&mut self, dir: String, cx: &mut Context<Self>) {
    let dir = (!dir.is_empty()).then_some(dir);
    if self.global_output_dir != dir {
      self.global_output_dir = dir;
      cx.notify();
    }
  }

  /// Where BetterMediaInfo was detected (the directory the launch needs), or
  /// `None` when it was not.
  pub fn set_better_media_info(&mut self, dir: Option<String>, cx: &mut Context<Self>) {
    if self.better_media_info != dir {
      self.better_media_info = dir;
      cx.notify();
    }
  }

  /// Applies changed settings: units and groups are derived again.
  pub fn set_settings(&mut self, settings: FileSettings, cx: &mut Context<Self>) {
    self.settings = settings;
    self.refresh(Vec::new(), false, cx);
  }

  /// Runs `edit` on the rows `keys` of each file in `files`; the files that
  /// have such a row.
  fn edit_rows(&mut self, files: &[String], keys: &[String], mut edit: impl FnMut(&mut MediaTrack)) -> Vec<String> {
    let keys: HashSet<&str> = keys.iter().map(String::as_str).collect();
    let mut changed = Vec::new();
    for file in files {
      let Some(tracks) = self.file_tracks.get_mut(file) else {
        continue;
      };
      let mut hit = false;
      for track in tracks.iter_mut().filter(|track| keys.contains(track.key().as_str())) {
        edit(track);
        hit = true;
      }
      if hit {
        changed.push(file.clone());
      }
    }
    changed
  }

  /// Derives units and entries again, runs the automation of newly complete
  /// units, starts parsing unparsed files, then notifies (when `structural`
  /// or a derived value changed) and emits the changed files.
  fn refresh(&mut self, mut changed: Vec<String>, structural: bool, cx: &mut Context<Self>) {
    let before = (self.units.clone(), self.entries.clone(), self.has_selection);
    self.derive();
    let automated = self.run_automation();
    if !automated.is_empty() {
      // Automation edits languages, which the language group mode keys on.
      self.derive();
      changed.extend(automated);
    }
    self.has_selection = self
      .files
      .iter()
      .any(|file| self.file_selected_ids.get(file).is_some_and(|ids| !ids.is_empty()));
    let after = (&self.units, &self.entries, self.has_selection);
    let structural = structural || (&before.0, &before.1, before.2) != after;
    self.load_pending(cx);
    if !changed.is_empty() {
      let mut seen = HashSet::new();
      changed.retain(|file| seen.insert(file.clone()));
      cx.emit(FileEvent::Changed { files: changed });
    }
    if structural {
      cx.notify();
    }
  }

  fn derive(&mut self) {
    self.units = build_merge_units(
      &self.files,
      self.settings.group_by_file_name,
      &self.merged_roots,
      &self.detached_files,
    );
    self.entries = build_render_entries(self.units.clone(), &self.settings.group_mode, &self.file_tracks);
  }
}

/// Sets a row's language and the codes its filters match.
fn set_language(track: &mut MediaTrack, value: &str) {
  track.language = value.to_string();
  track.language_codes = language_code_aliases(value);
}

/// Resolves dropped, chosen or launch-argument paths to media files on the
/// background executor (directories list their supported files, one level,
/// sorted) and adds them.
pub fn add_paths(files: &Entity<FileStore>, paths: Vec<String>, cx: &mut App) {
  if paths.is_empty() {
    return;
  }
  let files = files.clone();
  cx.spawn(async move |cx| {
    let resolved = cx
      .background_spawn(async move { bmm_core::services::get_media_files(paths) })
      .await;
    match resolved {
      Ok(media_files) => {
        if !media_files.is_empty() {
          files.update(cx, |files, cx| files.add_files(media_files, cx));
        }
      }
      Err(error) => log::error!("Failed to resolve the paths: {error}"),
    }
  })
  .detach();
}

#[cfg(test)]
mod table_tests;
#[cfg(test)]
pub(crate) mod tests;
