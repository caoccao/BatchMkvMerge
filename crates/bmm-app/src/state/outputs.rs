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

//! The output files of merges that are not written yet.
//!
//! An automatic output name is `<stem>.mkv`, numbered while that file exists,
//! and mkvmerge overwrites a target that exists. Two merges resolved before
//! either writes (two `Episode.srt` of one Merge All into one directory, or a
//! second Merge while the first still waits) would pick the same name, so
//! every enqueue reserves its output until its merge ends, and an automatic
//! name passes over the reserved ones too.
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

/// Output paths an automatic name must not take.
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

/// What an enqueue resolved: its output, whether the app chose that name
/// (and so may choose another), and what it built for it.
pub struct Resolution<T> {
  pub output: String,
  pub automatic: bool,
  pub value: T,
}

/// The reserved outputs, by the ticket of the enqueue that chose each. Shared
/// with the background executor, where outputs are resolved. The lock is
/// held only to read or change the map, never while the file system is read:
/// the foreground takes it too, and an output may be on a slow share.
#[derive(Clone, Default)]
pub struct OutputReservations(Arc<Mutex<HashMap<u64, String>>>);

impl OutputReservations {
  /// Runs `resolve` with the outputs reserved now and reserves the output it
  /// returns for `ticket`; returns what `resolve` built. An automatic output
  /// that another enqueue reserved meanwhile is resolved again past it, so
  /// two enqueues resolving at once still pick different names. An output
  /// the user named is reserved as it is.
  pub fn reserve<T>(&self, ticket: u64, mut resolve: impl FnMut(&TakenOutputs) -> Resolution<T>) -> T {
    let mut taken = self.taken();
    loop {
      let Resolution {
        output,
        automatic,
        value,
      } = resolve(&taken);
      let key = output_key(&output);
      let mut reserved = self.lock();
      if !automatic || !reserved.values().any(|other| *other == key) {
        reserved.insert(ticket, key);
        return value;
      }
      taken = TakenOutputs(reserved.values().cloned().collect());
    }
  }

  /// The outputs reserved now, for an output that is shown but not merged:
  /// Copy Command and the output dialog show what Merge would write.
  pub fn taken(&self) -> TakenOutputs {
    TakenOutputs(self.lock().values().cloned().collect())
  }

  /// Frees the output of `ticket`, if it reserved one.
  pub fn release(&self, ticket: u64) {
    self.lock().remove(&ticket);
  }

  /// A reservation is a map insert or removal, so a poisoned lock still holds
  /// a consistent map.
  fn lock(&self) -> MutexGuard<'_, HashMap<u64, String>> {
    self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::state::files::tests::TempDir;

  fn automatic(output: &str) -> Resolution<()> {
    Resolution {
      output: output.to_string(),
      automatic: true,
      value: (),
    }
  }

  #[test]
  fn a_reserved_output_is_taken_until_it_is_released() {
    let reservations = OutputReservations::default();
    let first = reservations.reserve(1, |taken| {
      assert!(!taken.contains("/out/Episode.mkv"));
      Resolution {
        output: "/out/Episode.mkv".to_string(),
        automatic: true,
        value: 'a',
      }
    });
    assert_eq!(first, 'a');
    reservations.reserve(2, |taken| {
      assert!(taken.contains("/out/Episode.mkv"));
      automatic("/out/Episode (1).mkv")
    });
    let taken = reservations.taken();
    assert!(taken.contains("/out/Episode.mkv") && taken.contains("/out/Episode (1).mkv"));
    reservations.release(1);
    reservations.release(7);
    let taken = reservations.taken();
    assert!(!taken.contains("/out/Episode.mkv") && taken.contains("/out/Episode (1).mkv"));
  }

  #[test]
  fn an_automatic_output_reserved_meanwhile_is_resolved_again() {
    let reservations = OutputReservations::default();
    let mut attempts = Vec::new();
    reservations.reserve(1, |taken| {
      if attempts.is_empty() {
        // Another enqueue reserves the name while this one resolves.
        reservations.reserve(2, |_| automatic("/out/Episode.mkv"));
      }
      let output = if taken.contains("/out/Episode.mkv") {
        "/out/Episode (1).mkv"
      } else {
        "/out/Episode.mkv"
      };
      attempts.push(output);
      automatic(output)
    });
    assert_eq!(attempts, ["/out/Episode.mkv", "/out/Episode (1).mkv"]);
    // A name the user chose is kept as it is.
    reservations.reserve(3, |_| Resolution {
      output: "/out/Episode.mkv".to_string(),
      automatic: false,
      value: (),
    });
    reservations.release(2);
    assert!(reservations.taken().contains("/out/Episode.mkv"), "still reserved by 3");
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
