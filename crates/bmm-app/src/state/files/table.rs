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

//! What a track table shows and where its edits land, in a file card or a
//! group card.
//!
//! A table shows one unit: a single card's, or the first unit of a group
//! card, whose edits fan out to the same member position of every unit (the
//! units of a group are structurally identical). Rows are keyed by their
//! combined key, `"{member index}:{type}:{id}"`.

use std::collections::HashSet;

use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::MediaTrack;
use gpui_kit::Context;

use super::{FileStore, set_language};
use crate::domain::file_tree::{
  CombinedTrack, FlagKind, apply_unit_flag_automation, combine_unit_tracks, next_track_flag, parse_row_key,
};
use crate::domain::grouping::RenderEntry;

/// The units a track table shows and edits: one unit for a single card,
/// two or more for a group card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableScope {
  units: Vec<Vec<String>>,
}

impl TableScope {
  /// A single card's unit, root first.
  pub fn unit(members: Vec<String>) -> Self {
    assert!(!members.is_empty(), "a unit has a root");
    Self { units: vec![members] }
  }

  /// A group card's units.
  pub fn group(units: Vec<Vec<String>>) -> Self {
    assert!(
      units.iter().all(|unit| !unit.is_empty()) && !units.is_empty(),
      "every unit has a root"
    );
    Self { units }
  }

  pub fn from_entry(entry: &RenderEntry) -> Self {
    match entry {
      RenderEntry::Single { members } => Self::unit(members.clone()),
      RenderEntry::Group { units, .. } => Self::group(units.clone()),
    }
  }

  /// The first unit's root: the table's identity while files are added,
  /// grouped or edited, and its card's id as the active card.
  pub fn id(&self) -> &str {
    &self.units[0][0]
  }

  pub fn units(&self) -> &[Vec<String>] {
    &self.units
  }

  /// The unit the table shows (a group card's first).
  pub fn members(&self) -> &[String] {
    &self.units[0]
  }

  pub fn is_group(&self) -> bool {
    self.units.len() > 1
  }

  /// Each unit's root, the files the queue knows.
  pub fn roots(&self) -> impl Iterator<Item = &str> {
    self.units.iter().map(|unit| unit[0].as_str())
  }

  /// Every file of every unit.
  pub fn files(&self) -> impl Iterator<Item = &String> {
    self.units.iter().flatten()
  }

  /// Whether the ID column shows `member:id`: a multi-member unit, or a
  /// group with one.
  pub fn qualified_ids(&self) -> bool {
    self.units.iter().any(|unit| unit.len() > 1)
  }

  /// A group of multi-member units cannot reorder: the units would need the
  /// same combined order, which nothing stores.
  pub fn can_reorder(&self) -> bool {
    !self.is_group() || self.units.iter().all(|unit| unit.len() == 1)
  }

  /// The custom order the rows follow: a single card's multi-member unit
  /// only (a single-member unit reorders its file's own rows, a group shows
  /// the default order).
  pub fn order<'a>(&self, store: &'a FileStore) -> Option<&'a [String]> {
    (!self.is_group() && self.members().len() > 1)
      .then(|| store.combined_track_order(self.id()))
      .flatten()
  }

  /// The table's rows, in display order.
  pub fn rows(&self, store: &FileStore) -> Vec<CombinedTrack> {
    combine_unit_tracks(self.members(), store.file_tracks(), self.order(store))
  }

  /// The row keys whose merge checkbox is ticked: the shown unit's selection
  /// in combined keys.
  pub fn checked_rows(&self, store: &FileStore) -> HashSet<String> {
    let mut checked = HashSet::new();
    for (member_index, file) in self.members().iter().enumerate() {
      for bare_key in store.selected_ids(file).into_iter().flatten() {
        checked.insert(format!("{member_index}:{bare_key}"));
      }
    }
    checked
  }

  /// The `(file, row key)` pairs an edit of `row_keys` touches: the member
  /// at the row's position in every unit.
  fn targets(&self, row_keys: &[String]) -> Vec<(String, String)> {
    let mut targets = Vec::new();
    for row_key in row_keys {
      let Some((member_index, bare_key)) = parse_row_key(row_key) else {
        continue;
      };
      for unit in &self.units {
        if let Some(file) = unit.get(member_index) {
          targets.push((file.clone(), bare_key.to_string()));
        }
      }
    }
    targets
  }
}

impl FileStore {
  /// The select-all checkbox: every row of every parsed file of the scope, or
  /// none.
  pub fn set_all_rows_checked(&mut self, scope: &TableScope, checked: bool, cx: &mut Context<Self>) {
    let mut changed = Vec::new();
    for file in scope.files() {
      let Some(tracks) = self.file_tracks.get(file) else {
        continue;
      };
      let ids = if checked {
        tracks.iter().map(MediaTrack::key).collect()
      } else {
        Vec::new()
      };
      self.file_selected_ids.insert(file.clone(), ids);
      changed.push(file.clone());
    }
    self.refresh(changed, false, cx);
  }

  /// Ticks or clears the merge checkbox of `row_keys`.
  pub fn set_rows_checked(&mut self, scope: &TableScope, row_keys: &[String], checked: bool, cx: &mut Context<Self>) {
    let changed = self.edit_selection(scope, row_keys, |ids, key| {
      let present = ids.iter().any(|id| id == key);
      if checked && !present {
        ids.push(key.to_string());
      } else if !checked && present {
        ids.retain(|id| id != key);
      }
    });
    self.refresh(changed, false, cx);
  }

  /// Flips the merge checkbox of each of `row_keys` (Space on the selected
  /// rows).
  pub fn flip_rows_checked(&mut self, scope: &TableScope, row_keys: &[String], cx: &mut Context<Self>) {
    let changed = self.edit_selection(scope, row_keys, |ids, key| {
      if ids.iter().any(|id| id == key) {
        ids.retain(|id| id != key);
      } else {
        ids.push(key.to_string());
      }
    });
    self.refresh(changed, false, cx);
  }

  /// Sets the language of `row_keys`.
  pub fn set_rows_language(&mut self, scope: &TableScope, row_keys: &[String], value: &str, cx: &mut Context<Self>) {
    let changed = self.edit_targets(scope, row_keys, |track| set_language(track, value));
    self.refresh_languages(changed, cx);
  }

  /// Sets the track name of `row_keys`.
  pub fn set_rows_name(&mut self, scope: &TableScope, row_keys: &[String], value: &str, cx: &mut Context<Self>) {
    let changed = self.edit_targets(scope, row_keys, |track| track.track_name = value.to_string());
    self.refresh_rows(changed, cx);
  }

  /// A click on a flag button: the clicked row's next value, set on
  /// `row_keys`.
  pub fn cycle_rows_flag(
    &mut self,
    scope: &TableScope,
    clicked: &str,
    row_keys: &[String],
    kind: FlagKind,
    cx: &mut Context<Self>,
  ) {
    let Some(row) = scope.rows(self).into_iter().find(|row| row.row_key() == clicked) else {
      return;
    };
    let value = next_track_flag(flag_of(&row.track, kind));
    let changed = self.edit_targets(scope, row_keys, |track| match kind {
      FlagKind::Default => track.default_track = value,
      FlagKind::Forced => track.forced = value,
    });
    self.refresh_rows(changed, cx);
  }

  /// The Default Track / Forced Display header buttons: the reset automation
  /// over each unit's own checked rows.
  pub fn reset_rows_flags(
    &mut self,
    scope: &TableScope,
    reset_default: bool,
    reset_forced: bool,
    cx: &mut Context<Self>,
  ) {
    let order = scope.order(self).map(<[String]>::to_vec);
    let edits: Vec<_> = scope
      .units()
      .iter()
      .flat_map(|unit| {
        apply_unit_flag_automation(
          unit,
          &self.file_tracks,
          &self.file_selected_ids,
          reset_default,
          reset_forced,
          order.as_deref(),
        )
      })
      .collect();
    let mut changed = Vec::new();
    for edit in edits {
      let set = |track: &mut MediaTrack| match edit.kind {
        FlagKind::Default => track.default_track = edit.value,
        FlagKind::Forced => track.forced = edit.value,
      };
      changed.extend(self.edit_rows(std::slice::from_ref(&edit.file), &edit.keys, set));
    }
    self.refresh_rows(changed, cx);
  }

  /// Drag-reorder: moves the `from` row to the `to` row's position. A
  /// single-member unit reorders its file's rows (every root's, in lock-step,
  /// for a group); a multi-member unit stores a combined order, so a row can
  /// move across members and types.
  pub fn reorder_rows(&mut self, scope: &TableScope, from: &str, to: &str, cx: &mut Context<Self>) {
    if from == to || !scope.can_reorder() {
      return;
    }
    if scope.members().len() > 1 {
      let rows = scope.rows(self);
      let mut ids: Vec<String> = rows.iter().map(CombinedTrack::combined_id).collect();
      let position = |key: &str| rows.iter().position(|row| row.row_key() == key);
      let (Some(from_index), Some(to_index)) = (position(from), position(to)) else {
        return;
      };
      let moved = ids.remove(from_index);
      ids.insert(to_index, moved);
      self.set_combined_track_order(scope.id(), ids, cx);
      return;
    }
    let (Some((_, from_key)), Some((_, to_key))) = (parse_row_key(from), parse_row_key(to)) else {
      return;
    };
    let roots: Vec<String> = scope.roots().map(str::to_string).collect();
    self.reorder_tracks(&roots, from_key, to_key, cx);
  }

  /// Runs `edit` on each target's selection; the files edited.
  fn edit_selection(
    &mut self,
    scope: &TableScope,
    row_keys: &[String],
    mut edit: impl FnMut(&mut Vec<String>, &str),
  ) -> Vec<String> {
    let mut changed = Vec::new();
    for (file, key) in scope.targets(row_keys) {
      if !self.file_tracks.contains_key(&file) {
        continue;
      }
      edit(self.file_selected_ids.entry(file.clone()).or_default(), &key);
      changed.push(file);
    }
    changed
  }

  /// Runs `edit` on each target row; the files edited.
  fn edit_targets(
    &mut self,
    scope: &TableScope,
    row_keys: &[String],
    mut edit: impl FnMut(&mut MediaTrack),
  ) -> Vec<String> {
    let mut changed = Vec::new();
    for (file, key) in scope.targets(row_keys) {
      changed.extend(self.edit_rows(std::slice::from_ref(&file), std::slice::from_ref(&key), &mut edit));
    }
    changed
  }
}

/// The flag a button shows, as the table reads it.
pub fn flag_of(track: &MediaTrack, kind: FlagKind) -> TrackFlag {
  match kind {
    FlagKind::Default => track.default_track,
    FlagKind::Forced => track.forced,
  }
}
