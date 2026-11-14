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

//! The dropped files. Only the file list itself so far; parsed metadata,
//! selections, merges and output paths join with the metadata pipeline.

use gpui_kit::{App, AppContext, Context, Entity};

/// The files shown in the file list, in the order they were added.
#[derive(Default)]
pub struct FileStore {
  files: Vec<String>,
  /// Session-only default output directory (not persisted).
  global_output_dir: Option<String>,
}

impl FileStore {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn files(&self) -> &[String] {
    &self.files
  }

  pub fn has_files(&self) -> bool {
    !self.files.is_empty()
  }

  pub fn global_output_dir(&self) -> Option<&str> {
    self.global_output_dir.as_deref()
  }

  /// Appends the paths that are not listed yet, in order (`addFiles`).
  pub fn add_files(&mut self, paths: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
    let before = self.files.len();
    for path in paths {
      if !self.files.contains(&path) {
        self.files.push(path);
      }
    }
    if self.files.len() != before {
      cx.notify();
    }
  }

  /// Removes every file (`clearFiles`).
  pub fn clear_files(&mut self, cx: &mut Context<Self>) {
    if !self.files.is_empty() {
      self.files.clear();
      cx.notify();
    }
  }
}

/// Resolves dropped, chosen or launch-argument paths to media files on the
/// background executor (directories list their supported files, one level,
/// sorted) and adds them, as `getMediaFiles` + `addFiles` did.
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
pub(crate) mod tests {
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::TestAppContext;
  use std::path::{Path, PathBuf};

  /// A fresh directory under the system temp dir, removed on drop.
  pub(crate) struct TempDir(pub PathBuf);

  impl TempDir {
    pub(crate) fn new(name: &str) -> Self {
      let path = std::env::temp_dir().join(format!("bmm-app-{name}-{}", std::process::id()));
      let _ = std::fs::remove_dir_all(&path);
      std::fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    pub(crate) fn file(&self, name: &str) -> String {
      let path = self.0.join(name);
      std::fs::write(&path, b"").unwrap();
      path.to_string_lossy().into_owned()
    }

    pub(crate) fn path(&self) -> &Path {
      &self.0
    }
  }

  impl Drop for TempDir {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  #[gpui_kit::test]
  fn add_files_appends_new_paths_in_order(cx: &mut TestAppContext) {
    let store = cx.new(|_| FileStore::new());
    store.update(cx, |store, cx| {
      store.add_files(["b".to_owned(), "a".to_owned()], cx);
      store.add_files(["a".to_owned(), "c".to_owned(), "c".to_owned()], cx);
    });
    store.read_with(cx, |store, _| assert_eq!(store.files(), ["b", "a", "c"]));
    store.update(cx, |store, cx| store.clear_files(cx));
    store.read_with(cx, |store, _| assert!(!store.has_files()));
  }

  #[gpui_kit::test]
  fn add_paths_lists_the_media_files_of_files_and_folders(cx: &mut TestAppContext) {
    let dir = TempDir::new("add-paths");
    let movie = dir.file("b.mkv");
    let subtitle = dir.file("a.srt");
    dir.file("notes.txt");
    let other = TempDir::new("add-paths-other");
    let single = other.file("single.mka");
    let ignored = other.file("ignored.doc");
    let store = cx.new(|_| FileStore::new());
    let paths = vec![
      dir.path().to_string_lossy().into_owned(),
      single.clone(),
      ignored,
      "/does/not/exist.mkv".to_owned(),
    ];
    cx.update(|cx| add_paths(&store, paths, cx));
    cx.run_until_parked();
    store.read_with(cx, |store, _| assert_eq!(store.files(), [subtitle, movie, single]));
  }
}
