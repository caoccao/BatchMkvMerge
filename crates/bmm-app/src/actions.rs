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

//! What spans stores: enqueue, cancel, clear and remove, which touch both the
//! files and the queue.
//!
//! An enqueue lists every root that is not waiting or merging as waiting and
//! hands it to the merge queue at once, in order, so a drive's queue follows
//! the file list. The output is chosen when the merge starts
//! ([`resolve_merge_output_for`]): a merge may wait for its drive, and only
//! then is it known which files exist. The app never creates the output
//! directory: mkvmerge creates missing directories.

use bmm_core::media_track::MediaTrack;
use bmm_core::merge::MergeArgs;
use bmm_core::merge_args::{
  MergeInput, OrderedTrackRef, build_command_string, build_command_string_multi, build_merge_args,
  build_merge_args_multi, resolve_output_dir,
};
use bmm_core::services::{override_directory, resolve_merge_output_path_avoiding, resolve_named_output_path_avoiding};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gpui_kit::{App, AppContext, ClipboardItem, Task};

use crate::domain::file_tree::combine_unit_tracks;
use crate::domain::status::{QueueItem, QueueItemStatus};
use crate::state::outputs::{RunningOutputs, TakenOutputs};
use crate::state::ui::NotificationKind;
use crate::state::{FileStore, Stores};
use crate::tools::Tool;

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

/// The output a merge keyed by `file` writes, chosen as the merge starts
/// (and, to show it, by Copy Command and the output dialog). A name the app
/// picks, `<dir>/<stem>.mkv` or the input's file name in a directory the
/// card's path names, gets " (1)", " (2)", … while that file exists or is
/// `taken` by a running merge. A file path the user set is used as is; `Err`
/// names it when a file is there or a running merge writes it, as a merge
/// replaces no file. Checks the file system.
pub fn resolve_merge_output_for(target: &OutputTarget, file: &str, taken: &TakenOutputs) -> Result<String, String> {
  let is_taken = |path: &str| taken.contains(path);
  let Some(path) = target.path.as_deref().filter(|path| !path.is_empty()) else {
    let dir = resolve_output_dir(file, target.dir.as_deref(), target.global_dir.as_deref());
    return Ok(resolve_merge_output_path_avoiding(dir, file.to_string(), is_taken));
  };
  if let Some(dir) = override_directory(path) {
    let name = Path::new(file)
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("output.mkv");
    return Ok(resolve_named_output_path_avoiding(
      dir.to_string(),
      name.to_string(),
      is_taken,
    ));
  }
  if Path::new(path).exists() || is_taken(path) {
    Err(path.to_string())
  } else {
    Ok(path.to_string())
  }
}

/// What a merge whose set output path is taken tells the user.
pub fn output_exists_message(path: &str) -> String {
  tr!("merge.error.outputExists", path = path).to_string()
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
  /// The merge of one file's selected `tracks`; `None` without any.
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

  /// One output from several inputs, keyed by `root`. Inputs without a
  /// selected row are dropped; `None` when none is left.
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

  /// A card unit with the stores' current selection: one member merges as a
  /// file, several as one output in the unit's combined display order (its
  /// custom order, if any).
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

  /// One unit of a group card with the stores' current selection
  /// (`GroupCard.enqueueUnit`): one member merges as a file, several as one
  /// output in the default type-major order, as the group card passed no
  /// order.
  pub fn group_unit(files: &FileStore, unit: &[String]) -> Option<Self> {
    let root = unit.first()?;
    if unit.len() == 1 {
      return Self::file(files, root, files.selected_tracks(root));
    }
    let inputs = unit
      .iter()
      .map(|file| MergeInput {
        file: file.clone(),
        tracks: files.selected_tracks(file),
      })
      .collect();
    Self::unit(files, root, inputs, None)
  }

  /// The output file the merge writes, passing over the `taken` outputs;
  /// `Err` names a set path that is taken. Checks the file system.
  pub fn output_path(&self, taken: &TakenOutputs) -> Result<String, String> {
    resolve_merge_output_for(&self.output, &self.root, taken)
  }

  /// The `mkvmerge` arguments that write `output`.
  pub fn args(&self, output: &str) -> Vec<String> {
    if self.multi {
      build_merge_args_multi(&self.inputs, output, self.order.as_deref())
    } else {
      build_merge_args(&self.root, output, &self.inputs[0].tracks)
    }
  }

  /// The copyable shell command (`buildCommandString(Multi)`) that writes
  /// `output` with the MKVToolNix in `mkv_toolnix_path`.
  pub fn command(&self, output: &str, mkv_toolnix_path: &str) -> String {
    if self.multi {
      build_command_string_multi(&self.inputs, output, mkv_toolnix_path, self.order.as_deref())
    } else {
      build_command_string(&self.root, output, mkv_toolnix_path, &self.inputs[0].tracks)
    }
  }
}

/// The arguments of `request`'s merge, built as its worker starts it: the
/// output is chosen then ([`resolve_merge_output_for`]) and claimed in
/// `outputs` until the merge finishes. A set path that is taken fails the
/// merge.
fn launch_args(request: MergeRequest, outputs: RunningOutputs) -> MergeArgs {
  MergeArgs::AtLaunch(Box::new(move || {
    let output = outputs.claim(&request.root, |taken| {
      request.output_path(taken).map_err(|path| output_exists_message(&path))
    })?;
    Ok(request.args(&output))
  }))
}

/// Lists the roots of `requests` that are not waiting or merging as waiting
/// and hands them to the merge queue, in order; each chooses its output when
/// it starts ([`launch_args`]).
pub fn enqueue(stores: &Stores, requests: Vec<MergeRequest>, cx: &mut App) {
  let backend = stores.merges.clone();
  stores.queue.update(cx, |queue, cx| {
    for request in requests {
      let root = request.root.clone();
      if !queue.add_to_queue(&root, cx) {
        continue;
      }
      let args = launch_args(request, queue.outputs().clone());
      if let Err(error) = backend.enqueue(root.clone(), args) {
        log::error!("Failed to enqueue {root}: {error}");
        queue.abandon(&root, cx);
      }
    }
  });
}

/// Whether Merge All (toolbar, F3) is available: a listed file has selected
/// tracks.
pub fn can_merge_all(stores: &Stores, cx: &App) -> bool {
  stores.files.read(cx).has_selection()
}

/// Merge All: one merge per card unit, not per file, so a multi-member card
/// is one queue task as with its own Merge button.
pub fn merge_all(stores: &Stores, cx: &mut App) {
  let files = stores.files.read(cx);
  let requests = files
    .units()
    .iter()
    .filter_map(|unit| MergeRequest::unit_selection(files, unit))
    .collect();
  enqueue(stores, requests, cx)
}

/// Asks the queue to cancel `file`; the outcome arrives as a merge event.
pub fn cancel_merge(stores: &Stores, file: &str, cx: &mut App) {
  stores
    .queue
    .update(cx, |queue, cx| queue.mark_cancel_requested(file, cx));
  if let Err(error) = stores.merges.cancel(file) {
    log::error!("Failed to cancel {file}: {error}");
  }
}

/// [`cancel_merge`] for each of `files`.
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

/// A root merges its whole card unit with the current selection. A listed file
/// that is no longer a unit root merges alone, and a removed file is skipped.
fn resume_request(files: &FileStore, item: &QueueItem) -> Option<MergeRequest> {
  let root = &item.file;
  match files.units().iter().find(|unit| unit.first() == Some(root)) {
    Some(unit) => MergeRequest::unit_selection(files, unit),
    None if files.files().contains(root) => MergeRequest::unit_selection(files, std::slice::from_ref(root)),
    None => None,
  }
}

/// Resume one cancelled or failed job from its queue row.
pub fn resume_merge(stores: &Stores, file: &str, cx: &mut App) {
  let request = stores
    .queue
    .read(cx)
    .item(file)
    .filter(|item| item.status.is_resumable())
    .and_then(|item| resume_request(stores.files.read(cx), item));
  if let Some(request) = request {
    enqueue(stores, vec![request], cx);
  }
}

/// Resume All requeues cancelled and failed roots in queue order.
pub fn resume_all(stores: &Stores, cx: &mut App) {
  let files = stores.files.read(cx);
  let requests = stores
    .queue
    .read(cx)
    .resumable_items()
    .filter_map(|item| resume_request(files, item))
    .collect();
  enqueue(stores, requests, cx)
}

/// Whether Clear (toolbar, Ctrl+Q) is available: files listed and no merge
/// waiting or running.
pub fn can_clear(stores: &Stores, cx: &App) -> bool {
  stores.files.read(cx).has_files() && !stores.has_active_merges(cx)
}

/// Clears the file list and the queue if [`can_clear`].
pub fn clear_files(stores: &Stores, cx: &mut App) {
  if can_clear(stores, cx) {
    stores.files.update(cx, |files, cx| files.clear_files(cx));
    stores.queue.update(cx, |queue, cx| queue.clear(cx));
  }
}

/// Removes one file and its queue item. A merge of the file that waits or
/// runs is cancelled first, so none goes on without its card; its item stays,
/// hidden, until the merge queue reports the end.
pub fn remove_file(stores: &Stores, file: &str, cx: &mut App) {
  let uncancelled = stores
    .queue
    .read(cx)
    .item(file)
    .is_some_and(|item| item.status.is_active() && !item.cancel_requested);
  if uncancelled {
    cancel_merge(stores, file, cx);
  }
  stores.files.update(cx, |files, cx| files.remove_file(file, cx));
  stores.queue.update(cx, |queue, cx| queue.remove_from_queue(file, cx));
}

/// Whether a member of `unit` merges. Not only the root: a card regrouped
/// while a file's own merge runs holds that file as a member.
pub fn unit_merging(stores: &Stores, unit: &[String], cx: &App) -> bool {
  let queue = stores.queue.read(cx);
  unit.iter().any(|file| {
    queue
      .item(file)
      .is_some_and(|item| item.status == QueueItemStatus::Merging)
  })
}

/// A file card's Delete (`MkvFileCard.handleDelete`): nothing while a member
/// merges; otherwise every member goes, with its queue item, a waiting one
/// cancelled first. `false` when nothing was removed.
pub fn delete_unit(stores: &Stores, unit: &[String], cx: &mut App) -> bool {
  if unit.is_empty() || unit_merging(stores, unit, cx) {
    return false;
  }
  for file in unit {
    remove_file(stores, file, cx);
  }
  true
}

/// A group card's Clear All (`GroupCard.handleClearAll`): every unit without
/// a merging member goes, as with [`delete_unit`].
pub fn clear_units(stores: &Stores, units: &[Vec<String>], cx: &mut App) {
  for unit in units {
    delete_unit(stores, unit, cx);
  }
}

/// Copies the commands of `requests`, one per line, to the clipboard and says
/// so. Outputs are chosen on the background executor as Merge would choose
/// them now: past the outputs of the running merges, and past each other's.
/// A set path that is taken copies nothing and says so.
pub fn copy_commands(stores: &Stores, requests: Vec<MergeRequest>, cx: &mut App) -> Task<()> {
  if requests.is_empty() {
    return Task::ready(());
  }
  let toolnix = stores.config.read(cx).config().external_tools.mkv_toolnix_path.clone();
  let mut taken = stores.queue.read(cx).outputs().taken();
  let ui = stores.ui.clone();
  cx.spawn(async move |cx| {
    let commands = cx
      .background_spawn(async move {
        requests
          .iter()
          .map(|request| {
            let output = request.output_path(&taken)?;
            taken.insert(&output);
            Ok(request.command(&output, &toolnix))
          })
          .collect::<Result<Vec<_>, String>>()
      })
      .await;
    cx.update(|cx| match commands {
      Ok(commands) => {
        cx.write_to_clipboard(ClipboardItem::new_string(commands.join("\n")));
        ui.update(cx, |ui, cx| {
          ui.show_snackbar(NotificationKind::Success, tr!("merge.commandCopied").to_string(), cx)
        });
      }
      Err(path) => {
        ui.update(cx, |ui, cx| {
          ui.show_snackbar(NotificationKind::Error, output_exists_message(&path), cx)
        });
      }
    });
  })
}

/// Opens `paths` in the detected BetterMediaInfo; a failure is shown as a card
/// message.
pub fn open_in_better_media_info(stores: &Stores, paths: Vec<String>, cx: &mut App) -> Task<()> {
  let Some(dir) = stores.files.read(cx).better_media_info_dir().map(str::to_string) else {
    return Task::ready(());
  };
  let ui = stores.ui.clone();
  cx.spawn(async move |cx| {
    let launched = cx
      .background_spawn(async move { bmm_core::services::launch_better_media_info_from(&dir, paths) })
      .await;
    if let Err(error) = launched {
      cx.update(|cx| {
        ui.update(cx, |ui, cx| {
          ui.show_snackbar(NotificationKind::Error, error.to_string(), cx)
        });
      });
    }
  })
}

/// Detects BetterMediaInfo now and whenever its configured path changes: the
/// cards show Open in BetterMediaInfo while it is found. A detection that a
/// newer one overtook is dropped.
pub fn watch_better_media_info(stores: &Stores, cx: &mut App) {
  let detected_path: Rc<RefCell<Option<String>>> = Rc::default();
  let generation = Rc::new(Cell::new(0_u64));
  let detect = {
    let (files, tools) = (stores.files.clone(), stores.tools.clone());
    move |path: String, cx: &mut App| {
      if detected_path.borrow().as_ref() == Some(&path) {
        return;
      }
      *detected_path.borrow_mut() = Some(path.clone());
      generation.set(generation.get() + 1);
      let (files, tools, generation, current) = (files.clone(), tools.clone(), generation.clone(), generation.get());
      cx.spawn(async move |cx| {
        let status = cx
          .background_spawn(async move { tools.detect(Tool::BetterMediaInfo, path.trim(), false) })
          .await;
        if generation.get() != current {
          return;
        }
        let dir = status.found.then_some(status.path);
        files.update(cx, |files, cx| files.set_better_media_info(dir, cx));
      })
      .detach();
    }
  };
  let path = |stores: &Stores, cx: &App| {
    stores
      .config
      .read(cx)
      .config()
      .external_tools
      .better_media_info_path
      .clone()
  };
  detect(path(stores, cx), cx);
  let watched = stores.clone();
  cx.observe(&stores.config, move |_, cx| detect(path(&watched, cx), cx))
    .detach();
}

#[cfg(test)]
mod tests;
