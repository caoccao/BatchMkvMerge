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

//! The output files of running merges, and the partial files of stopped
//! ones.
//!
//! A merge chooses its output just before `mkvmerge` starts, not when it is
//! enqueued: it may wait for its drive, and only then is it known which files
//! exist. mkvmerge overwrites a target that exists, and a merge that has just
//! started may not have created its output yet, so the outputs of running
//! merges are claimed until they finish, and merges on different drives
//! choose one at a time. When a merge that started `mkvmerge` fails or is
//! cancelled, its output is kept as the partial file it left, which Resume
//! replaces; a merge that never started `mkvmerge` wrote nothing, so a file
//! at its output is not its own. The partial file belongs to one job, kept
//! across its Resumes: a fresh merge of the root is a new job and forgets it.
//!
//! Paths are compared by the file they name ([`output_key`]): `out` and
//! `out\..\out`, a link and its target, or two spellings of one directory
//! are the same output.

use bmm_core::merge::MergeOutcome;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// The file `path` names, for comparing outputs: the deepest directory of it
/// that exists as the file system resolves it (links, `..`, short names and
/// the letter case on disk), the rest folded lexically, as directories that
/// do not exist yet cannot be links. Windows and macOS file systems ignore
/// case, so the key does too there. Reads the file system.
pub fn output_key(path: &str) -> String {
  let path = Path::new(path);
  let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
  let components: Vec<Component> = absolute.components().collect();
  let (mut key, rest) = (1..=components.len())
    .rev()
    .find_map(|split| {
      let existing: PathBuf = components[..split].iter().collect();
      // `C:` alone is the current directory of drive C.
      if !existing.has_root() {
        return None;
      }
      std::fs::canonicalize(&existing)
        .ok()
        .map(|real| (real, &components[split..]))
    })
    .unwrap_or((PathBuf::new(), &components[..]));
  for component in rest {
    match component {
      Component::CurDir => {}
      Component::ParentDir => {
        key.pop();
      }
      other => key.push(other),
    }
  }
  let key = key.to_string_lossy();
  if cfg!(any(windows, target_os = "macos")) {
    key.to_lowercase()
  } else {
    key.into_owned()
  }
}

/// Where a merge writes: a card's full output path wins, else its directory
/// override, else the session's global directory, else the input's own
/// directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputTarget {
  pub path: Option<String>,
  pub dir: Option<String>,
  pub global_dir: Option<String>,
}

/// Output paths a new output must not take.
#[derive(Clone, Debug, Default)]
pub struct TakenOutputs(HashSet<String>);

impl TakenOutputs {
  /// Reads the file system.
  pub fn contains(&self, path: &str) -> bool {
    !self.0.is_empty() && self.0.contains(&output_key(path))
  }

  /// Reads the file system.
  pub fn insert(&mut self, path: &str) {
    self.0.insert(output_key(path));
  }
}

/// The outputs claimed by running merges, by the root file of each merge,
/// and the partial files that stopped merges left. Shared with the merge
/// workers, which choose outputs. Choosing reads the file system and takes
/// one lock for its whole length, so that two workers never pick the same
/// file; the foreground takes only the other, short one.
#[derive(Clone, Default)]
pub struct RunningOutputs {
  choosing: Arc<Mutex<()>>,
  outputs: Arc<Mutex<Outputs>>,
}

/// Under one lock, so that a worker never sees a merge's claim gone and the
/// partial files it replaced still there.
#[derive(Default)]
struct Outputs {
  /// By root, the output of its running merge.
  claimed: HashMap<String, Chosen>,
  /// By root, the output its latest merge that started `mkvmerge` left
  /// unfinished: that merge failed or was cancelled, and the root's job has
  /// only been resumed since.
  partial: HashMap<String, Chosen>,
}

/// An output a merge chose, and the card's output settings it was chosen
/// for.
#[derive(Clone)]
struct Chosen {
  target: OutputTarget,
  output: String,
  key: String,
}

impl RunningOutputs {
  /// Runs `choose` and claims the output it returns for the merge of `root`
  /// with the output settings `target`, one worker at a time. `choose` gets
  /// the outputs of the running merges and the partial file the stopped
  /// merge of `root` left, if any: the latest merge of the root's job that
  /// started `mkvmerge`, when it did not complete and `target` is still the
  /// one it was chosen for. An `Err` from `choose` claims nothing.
  pub fn claim<E>(
    &self,
    root: &str,
    target: &OutputTarget,
    choose: impl FnOnce(&TakenOutputs, Option<&str>) -> Result<String, E>,
  ) -> Result<String, E> {
    let _choosing = self.choosing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (taken, partial) = {
      let outputs = self.outputs();
      let partial = outputs
        .partial
        .get(root)
        .filter(|partial| partial.target == *target)
        .map(|partial| partial.output.clone());
      (outputs.taken(), partial)
    };
    let output = choose(&taken, partial.as_deref())?;
    let key = output_key(&output);
    let chosen = Chosen {
      target: target.clone(),
      output: output.clone(),
      key,
    };
    self.outputs().claimed.insert(root.to_string(), chosen);
    Ok(output)
  }

  /// The outputs claimed now, for an output that is shown but not merged:
  /// Copy Command and the output dialog show what Merge would write.
  pub fn taken(&self) -> TakenOutputs {
    self.outputs().taken()
  }

  /// Frees the output of the merge of `root`, which ended as `outcome`.
  /// When the merge started `mkvmerge` (`launched`), the file is its own
  /// now: a partial file of any merge there is gone, and unless it
  /// completed, the file is the one Resume replaces. A merge that did not
  /// start `mkvmerge` wrote nothing and changes no partial file.
  pub fn release(&self, root: &str, launched: bool, outcome: MergeOutcome) {
    let mut outputs = self.outputs();
    let Some(chosen) = outputs.claimed.remove(root) else {
      return;
    };
    if !launched {
      return;
    }
    outputs
      .partial
      .retain(|other, partial| other != root && partial.key != chosen.key);
    if outcome != MergeOutcome::Completed {
      outputs.partial.insert(root.to_string(), chosen);
    }
  }

  /// A fresh merge of `root` starts a new job, which has left no partial
  /// file: one that an earlier job left is not this job's to replace, also
  /// when this job fails before it starts `mkvmerge` and is then resumed.
  pub fn start_job(&self, root: &str) {
    self.outputs().partial.remove(root);
  }

  /// Every change is a map insert or removal, so a poisoned lock still
  /// holds consistent maps.
  fn outputs(&self) -> MutexGuard<'_, Outputs> {
    self.outputs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

impl Outputs {
  fn taken(&self) -> TakenOutputs {
    TakenOutputs(self.claimed.values().map(|chosen| chosen.key.clone()).collect())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::state::files::tests::TempDir;

  #[test]
  fn a_claimed_output_is_taken_until_its_merge_ends() {
    let outputs = RunningOutputs::default();
    let target = OutputTarget::default();
    let first = outputs.claim("/in/a.srt", &target, |taken, _| {
      assert!(!taken.contains("/out/Episode.mkv"));
      Ok::<_, ()>("/out/Episode.mkv".to_string())
    });
    assert_eq!(first, Ok("/out/Episode.mkv".to_string()));
    outputs
      .claim("/in/b.srt", &target, |taken, _| {
        assert!(taken.contains("/out/Episode.mkv"));
        Ok::<_, ()>("/out/Episode (1).mkv".to_string())
      })
      .unwrap();
    assert_eq!(outputs.claim("/in/c.srt", &target, |_, _| Err("taken")), Err("taken"));
    outputs.release("/in/a.srt", true, MergeOutcome::Completed);
    outputs.release("/in/unknown.srt", false, MergeOutcome::Cancelled);
    let taken = outputs.taken();
    assert!(!taken.contains("/out/Episode.mkv") && taken.contains("/out/Episode (1).mkv"));
  }

  /// The partial file that Resume replaces is the output of the latest merge
  /// of a root that started `mkvmerge`, when that merge failed or was
  /// cancelled, while the card's output settings are unchanged and no other
  /// merge has written the file since.
  #[test]
  fn only_a_stopped_merge_that_started_mkvmerge_leaves_a_partial_file() {
    let (a, b) = ("/in/a.mkv", "/in/b.mkv");
    let outputs = RunningOutputs::default();
    let target = OutputTarget {
      dir: Some("/out".to_string()),
      ..OutputTarget::default()
    };
    let run = |root: &str, output: &str, launched: bool, outcome: MergeOutcome| {
      outputs
        .claim(root, &target, |_, _| Ok::<_, ()>(output.to_string()))
        .unwrap();
      outputs.release(root, launched, outcome);
    };
    let partial = |root: &str, target: &OutputTarget| {
      let mut seen = None;
      let looked = outputs.claim(root, target, |_, partial| {
        seen = partial.map(str::to_string);
        Err::<String, ()>(())
      });
      assert_eq!(looked, Err(()), "claims nothing");
      seen
    };
    let left = |output: &str| Some(output.to_string());
    assert_eq!(partial(a, &target), None);
    run(a, "/out/a.mkv", true, MergeOutcome::Failed);
    assert_eq!(partial(a, &target), left("/out/a.mkv"));
    assert_eq!(partial(b, &target), None, "another root's");
    let moved = OutputTarget {
      dir: Some("/elsewhere".to_string()),
      ..OutputTarget::default()
    };
    assert_eq!(partial(a, &moved), None, "the settings changed");
    run(a, "/out/a (1).mkv", false, MergeOutcome::Failed);
    outputs.release(a, false, MergeOutcome::Cancelled);
    assert_eq!(
      partial(a, &target),
      left("/out/a.mkv"),
      "a merge that did not start mkvmerge, or was cancelled while waiting, wrote nothing"
    );
    run(a, "/out/a (1).mkv", true, MergeOutcome::Cancelled);
    assert_eq!(partial(a, &target), left("/out/a (1).mkv"), "the latest one");
    run(a, "/out/a (1).mkv", true, MergeOutcome::Completed);
    assert_eq!(partial(a, &target), None, "completed");
    run(a, "/out/a.mkv", true, MergeOutcome::Failed);
    run(b, "/out/./a.mkv", true, MergeOutcome::Completed);
    assert_eq!(partial(a, &target), None, "another merge wrote the file since");
    run(a, "/out/a.mkv", true, MergeOutcome::Failed);
    run(b, "/out/b.mkv", true, MergeOutcome::Cancelled);
    outputs.start_job(a);
    assert_eq!(partial(a, &target), None, "a fresh merge is a new job");
    assert_eq!(partial(b, &target), left("/out/b.mkv"), "only its root's");
  }

  /// Workers on different drives choosing at once still pick different files.
  #[test]
  fn workers_choose_one_at_a_time() {
    let dir = TempDir::new("outputs-at-once");
    let out = dir.path().to_string_lossy().into_owned();
    let outputs = RunningOutputs::default();
    let chosen: Vec<String> = std::thread::scope(|scope| {
      let workers: Vec<_> = (0..8)
        .map(|index| {
          let (outputs, out) = (outputs.clone(), out.clone());
          scope.spawn(move || {
            outputs
              .claim(
                &format!("/in/{index}/Episode.srt"),
                &OutputTarget::default(),
                |taken, _| {
                  Ok::<_, ()>(bmm_core::services::resolve_merge_output_path_avoiding(
                    out,
                    "Episode.srt".to_string(),
                    |path| taken.contains(path),
                  ))
                },
              )
              .unwrap()
          })
        })
        .collect();
      workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });
    let distinct: HashSet<&String> = chosen.iter().collect();
    assert_eq!(distinct.len(), 8, "{chosen:?}");
  }

  #[test]
  fn paths_compare_by_the_file_they_name() {
    let dir = TempDir::new("output-keys");
    std::fs::create_dir(dir.path().join("out")).unwrap();
    let path = |relative: &str| dir.path().join(relative).to_string_lossy().into_owned();
    let mut taken = TakenOutputs::default();
    taken.insert(&path("out/Episode.mkv"));
    assert!(taken.contains(&path("out/../out/Episode.mkv")));
    assert!(taken.contains(&path("out/./Episode.mkv")));
    assert!(
      taken.contains(&path("out/new/../Episode.mkv")),
      "through a directory not made yet"
    );
    assert!(!taken.contains(&path("out/Episode (1).mkv")));
    assert_eq!(
      taken.contains(&path("OUT/episode.mkv")),
      cfg!(any(windows, target_os = "macos")),
      "case folds where the file system ignores it"
    );
    assert_eq!(taken.contains(&path("out\\Episode.mkv")), cfg!(windows));
    // A directory that does not exist yet (mkvmerge creates it).
    taken.insert(&path("later/sub/Episode.mkv"));
    assert!(taken.contains(&path("later/x/../sub/Episode.mkv")));
  }
}
