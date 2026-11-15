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

//! The output files of running merges.
//!
//! A merge chooses its output just before `mkvmerge` starts, not when it is
//! enqueued: it may wait for its drive, and only then is it known which files
//! exist. mkvmerge overwrites a target that exists, and a merge that has just
//! started may not have created its output yet, so the outputs of running
//! merges are claimed until they finish, and merges on different drives
//! choose one at a time.
//!
//! Paths are compared by the file they name ([`output_key`]): `out` and
//! `out\..\out`, a link and its target, or two spellings of one directory
//! are the same output.

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

/// The outputs claimed by running merges, by the root file of each merge.
/// Shared with the merge workers, which choose outputs. Choosing reads the
/// file system and takes one lock for its whole length, so that two workers
/// never pick the same file; the foreground takes only the other, short one.
#[derive(Clone, Default)]
pub struct RunningOutputs {
  choosing: Arc<Mutex<()>>,
  claimed: Arc<Mutex<HashMap<String, String>>>,
}

impl RunningOutputs {
  /// Runs `choose` with the outputs of the running merges and claims the
  /// output it returns for the merge of `root`, one worker at a time. An
  /// `Err` from `choose` claims nothing.
  pub fn claim<E>(&self, root: &str, choose: impl FnOnce(&TakenOutputs) -> Result<String, E>) -> Result<String, E> {
    let _choosing = self.choosing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let output = choose(&self.taken())?;
    let key = output_key(&output);
    self.claimed().insert(root.to_string(), key);
    Ok(output)
  }

  /// The outputs claimed now, for an output that is shown but not merged:
  /// Copy Command and the output dialog show what Merge would write.
  pub fn taken(&self) -> TakenOutputs {
    TakenOutputs(self.claimed().values().cloned().collect())
  }

  /// Frees the output of the merge of `root`, which ended.
  pub fn release(&self, root: &str) {
    self.claimed().remove(root);
  }

  /// A claim is a map insert or removal, so a poisoned lock still holds a
  /// consistent map.
  fn claimed(&self) -> MutexGuard<'_, HashMap<String, String>> {
    self.claimed.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::state::files::tests::TempDir;

  #[test]
  fn a_claimed_output_is_taken_until_its_merge_ends() {
    let outputs = RunningOutputs::default();
    let first = outputs.claim("/in/a.srt", |taken| {
      assert!(!taken.contains("/out/Episode.mkv"));
      Ok::<_, ()>("/out/Episode.mkv".to_string())
    });
    assert_eq!(first, Ok("/out/Episode.mkv".to_string()));
    outputs
      .claim("/in/b.srt", |taken| {
        assert!(taken.contains("/out/Episode.mkv"));
        Ok::<_, ()>("/out/Episode (1).mkv".to_string())
      })
      .unwrap();
    assert_eq!(outputs.claim("/in/c.srt", |_| Err("taken")), Err("taken"));
    outputs.release("/in/a.srt");
    outputs.release("/in/unknown.srt");
    let taken = outputs.taken();
    assert!(!taken.contains("/out/Episode.mkv") && taken.contains("/out/Episode (1).mkv"));
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
              .claim(&format!("/in/{index}/Episode.srt"), |taken| {
                Ok::<_, ()>(bmm_core::services::resolve_merge_output_path_avoiding(
                  out,
                  "Episode.srt".to_string(),
                  |path| taken.contains(path),
                ))
              })
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
