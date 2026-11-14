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

//! What spans stores: enqueue, cancel, clear and remove (`mergeActions.ts`
//! and the store actions that touch both the files and the queue).
//!
//! A merge is enqueued in three steps: a [`MergeRequest`] is read from the
//! stores on the foreground; the output path is resolved and the `mkvmerge`
//! arguments built on the background executor (resolving checks the file
//! system); then, on the foreground again, every root that is not waiting or
//! merging is listed as waiting and handed to the merge queue. Requests keep
//! their order, so a drive's queue follows the file list. The app never
//! creates the output directory: mkvmerge creates missing directories.

use bmm_core::media_track::MediaTrack;
use bmm_core::merge_args::{MergeInput, OrderedTrackRef, build_merge_args, build_merge_args_multi, resolve_output_dir};
use bmm_core::services::{resolve_merge_output_path, resolve_overridden_output_path};
use gpui_kit::{App, AppContext, Task};

use crate::domain::file_tree::combine_unit_tracks;
use crate::state::{FileStore, Stores};

/// Where a merge writes: a card's full output path wins, else its directory
/// override, else the session's global directory, else the input's own
/// directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputTarget {
  pub path: Option<String>,
  pub dir: Option<String>,
  pub global_dir: Option<String>,
}

impl OutputTarget {
  /// The target the stores hold for `root`.
  pub fn of(files: &FileStore, root: &str) -> Self {
    Self {
      path: files.output_path(root).map(str::to_string),
      dir: files.output_dir(root).map(str::to_string),
      global_dir: files.global_output_dir().map(str::to_string),
    }
  }
}

/// The output file of a merge keyed by `file` (`resolveMergeOutputFor`). A
/// full-path override naming a file is used as is (the save dialog already
/// confirmed an overwrite); one naming a directory gets the input's file name
/// with no " (1)" suffix. Otherwise `<dir>/<stem>.mkv`, with " (1)", " (2)", …
/// while that exists. Checks the file system.
pub fn resolve_merge_output_for(target: &OutputTarget, file: &str) -> String {
  if let Some(path) = target.path.as_deref().filter(|path| !path.is_empty()) {
    return resolve_overridden_output_path(path.to_string(), file.to_string());
  }
  let dir = resolve_output_dir(file, target.dir.as_deref(), target.global_dir.as_deref());
  resolve_merge_output_path(dir, file.to_string())
}

/// One merge, read from the stores: the queue key (a unit root, the output's
/// base name), the inputs with their selected rows (root first), and for a
/// multi-input merge an explicit track order.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeRequest {
  pub root: String,
  pub inputs: Vec<MergeInput>,
  /// `None` for a single input; for several, `None` is the default
  /// type-major order.
  pub order: Option<Vec<OrderedTrackRef>>,
  pub multi: bool,
  pub output: OutputTarget,
}

impl MergeRequest {
  /// The merge of one file's selected `tracks` (`enqueueSelectedTracksForFile`);
  /// `None` without any.
  pub fn file(files: &FileStore, file: &str, tracks: Vec<MediaTrack>) -> Option<Self> {
    (!tracks.is_empty()).then(|| Self {
      root: file.to_string(),
      inputs: vec![MergeInput {
        file: file.to_string(),
        tracks,
      }],
      order: None,
      multi: false,
      output: OutputTarget::of(files, file),
    })
  }

  /// One output from several inputs, keyed by `root`
  /// (`enqueueSelectedTracksForUnit`). Inputs without a selected row are
  /// dropped; `None` when none is left.
  pub fn unit(
    files: &FileStore,
    root: &str,
    inputs: Vec<MergeInput>,
    order: Option<Vec<OrderedTrackRef>>,
  ) -> Option<Self> {
    let inputs: Vec<MergeInput> = inputs.into_iter().filter(|input| !input.tracks.is_empty()).collect();
    (!inputs.is_empty()).then(|| Self {
      root: root.to_string(),
      inputs,
      order,
      multi: true,
      output: OutputTarget::of(files, root),
    })
  }

  /// A card unit with the stores' current selection (`enqueueUnitSelection`):
  /// one member merges as a file, several as one output in the unit's
  /// combined display order (its custom order, if any).
  pub fn unit_selection(files: &FileStore, unit: &[String]) -> Option<Self> {
    let root = unit.first()?;
    if unit.len() == 1 {
      return Self::file(files, root, files.selected_tracks(root));
    }
    let inputs: Vec<MergeInput> = unit
      .iter()
      .map(|file| MergeInput {
        file: file.clone(),
        tracks: files.selected_tracks(file),
      })
      .collect();
    let order: Vec<OrderedTrackRef> = combine_unit_tracks(unit, files.file_tracks(), files.combined_track_order(root))
      .into_iter()
      .filter(|row| {
        files
          .selected_ids(&row.source_file)
          .is_some_and(|ids| ids.contains(&row.track.key()))
      })
      .map(|row| row.ordered_ref())
      .collect();
    Self::unit(files, root, inputs, Some(order))
  }

  /// The `mkvmerge` arguments, with the output path resolved. Checks the
  /// file system.
  pub fn resolve(&self) -> Vec<String> {
    let output = resolve_merge_output_for(&self.output, &self.root);
    if self.multi {
      build_merge_args_multi(&self.inputs, &output, self.order.as_deref())
    } else {
      build_merge_args(&self.root, &output, &self.inputs[0].tracks)
    }
  }
}

/// Resolves `requests` on the background executor, then enqueues them in
/// order, skipping a root that is waiting or merging.
pub fn enqueue(stores: &Stores, requests: Vec<MergeRequest>, cx: &mut App) -> Task<()> {
  let queue = stores.queue.clone();
  let backend = stores.merges.clone();
  let requests: Vec<MergeRequest> = requests
    .into_iter()
    .filter(|request| queue.read(cx).can_enqueue(&request.root))
    .collect();
  cx.spawn(async move |cx| {
    if requests.is_empty() {
      return;
    }
    let resolved: Vec<(String, Vec<String>)> = cx
      .background_spawn(async move {
        requests
          .into_iter()
          .map(|request| {
            let args = request.resolve();
            (request.root, args)
          })
          .collect()
      })
      .await;
    queue.update(cx, |queue, cx| {
      for (root, args) in resolved {
        if !queue.add_to_queue(&root, cx) {
          continue;
        }
        if let Err(error) = backend.enqueue(root.clone(), args) {
          log::error!("Failed to enqueue {root}: {error}");
          queue.remove_from_queue(&root, cx);
        }
      }
    });
  })
}

/// Whether Merge All (toolbar, F3) is available: a listed file has selected
/// tracks.
pub fn can_merge_all(stores: &Stores, cx: &App) -> bool {
  stores.files.read(cx).has_selection()
}

/// Merge All: one merge per card unit, not per file, so a multi-member card
/// is one queue task as with its own Merge button.
pub fn merge_all(stores: &Stores, cx: &mut App) -> Task<()> {
  let files = stores.files.read(cx);
  let requests = files
    .units()
    .iter()
    .filter_map(|unit| MergeRequest::unit_selection(files, unit))
    .collect();
  enqueue(stores, requests, cx)
}

/// Asks the queue to cancel `file` (`cancelMerge`); the outcome arrives as a
/// merge event.
pub fn cancel_merge(stores: &Stores, file: &str, cx: &mut App) {
  stores
    .queue
    .update(cx, |queue, cx| queue.mark_cancel_requested(file, cx));
  if let Err(error) = stores.merges.cancel(file) {
    log::error!("Failed to cancel {file}: {error}");
  }
}

/// `cancelMerges`.
pub fn cancel_merges(stores: &Stores, files: &[String], cx: &mut App) {
  for file in files {
    cancel_merge(stores, file, cx);
  }
}

/// Cancel All (toolbar, F4, the close guard's Yes): every waiting or merging
/// file.
pub fn cancel_all(stores: &Stores, cx: &mut App) {
  let files = stores.queue.read(cx).active_files();
  cancel_merges(stores, &files, cx);
}

/// Whether Clear (toolbar, Ctrl+Q) is available: files listed and no merge
/// waiting or running.
pub fn can_clear(stores: &Stores, cx: &App) -> bool {
  stores.files.read(cx).has_files() && !stores.has_active_merges(cx)
}

/// Clears the file list and the queue if [`can_clear`] (`clearFiles`).
pub fn clear_files(stores: &Stores, cx: &mut App) {
  if can_clear(stores, cx) {
    stores.files.update(cx, |files, cx| files.clear_files(cx));
    stores.queue.update(cx, |queue, cx| queue.clear(cx));
  }
}

/// Removes one file and its queue item (`removeFile`).
pub fn remove_file(stores: &Stores, file: &str, cx: &mut App) {
  stores.files.update(cx, |files, cx| files.remove_file(file, cx));
  stores.queue.update(cx, |queue, cx| queue.remove_from_queue(file, cx));
}

#[cfg(test)]
mod tests;
