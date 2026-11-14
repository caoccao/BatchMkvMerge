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

//! The persisted configuration (`bmm_core::config::Config`).

use std::sync::{Arc, Mutex};

use bmm_core::config::Config;
use gpui_kit::{AppContext, Context, Subscription};

/// Where configuration writes go: the config file in the app, a recorder in
/// tests. Writes carry a revision and run one at a time, and a write older
/// than the last one written is dropped, so background writes that finish out
/// of order never put an older config on disk.
#[derive(Clone)]
pub struct Persistence {
  write: Arc<dyn Fn(Config) + Send + Sync>,
  last_written: Arc<Mutex<u64>>,
}

impl Persistence {
  pub fn new(write: impl Fn(Config) + Send + Sync + 'static) -> Self {
    Self {
      write: Arc::new(write),
      last_written: Arc::new(Mutex::new(0)),
    }
  }

  /// Writes through `bmm_core::config::set_config`, which also updates the
  /// in-memory config the core reads.
  pub fn disk() -> Self {
    Self::new(|config| {
      if let Err(error) = bmm_core::config::set_config(config) {
        log::error!("Failed to save the config: {error}");
      }
    })
  }

  /// Writes `config` unless a newer revision has been written already.
  fn write(&self, revision: u64, config: Config) {
    let mut last_written = self
      .last_written
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    if revision > *last_written {
      (self.write)(config);
      *last_written = revision;
    }
  }
}

/// Holds the configuration the UI shows and edits.
pub struct ConfigStore {
  config: Config,
  revision: u64,
  persistence: Persistence,
  _flush_on_quit: Subscription,
}

impl ConfigStore {
  pub fn new(config: Config, persistence: Persistence, cx: &mut Context<Self>) -> Self {
    Self {
      config,
      revision: 0,
      persistence,
      // A write still pending at quit is finished synchronously.
      _flush_on_quit: cx.on_app_quit(|this, _| {
        this.flush();
        async {}
      }),
    }
  }

  pub fn config(&self) -> &Config {
    &self.config
  }

  /// Applies `edit` at once and persists the result in the background. The
  /// result is never read back: an older write finishing late must not revert
  /// newer edits (the React store's optimistic `updateConfig` rule).
  pub fn update(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Config)) {
    edit(&mut self.config);
    self.revision += 1;
    let (revision, config, persistence) = (self.revision, self.config.clone(), self.persistence.clone());
    cx.background_spawn(async move { persistence.write(revision, config) })
      .detach();
    cx.notify();
  }

  /// Writes the current config now unless that revision is on disk already.
  pub fn flush(&self) {
    self.persistence.write(self.revision, self.config.clone());
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use bmm_core::config::{GroupMode, Language};
  use core::prelude::v1::test;
  use gpui_kit::TestAppContext;

  fn recorder() -> (Persistence, Arc<Mutex<Vec<Config>>>) {
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = written.clone();
    (
      Persistence::new(move |config| sink.lock().unwrap().push(config)),
      written,
    )
  }

  #[gpui_kit::test]
  fn update_applies_at_once_and_persists_in_the_background(cx: &mut TestAppContext) {
    let (persistence, written) = recorder();
    let store = cx.new(|cx| ConfigStore::new(Config::default(), persistence, cx));
    store.update(cx, |store, cx| {
      store.update(cx, |config| config.group_by_file_name = false)
    });
    assert!(!store.read_with(cx, |store, _| store.config().group_by_file_name));
    cx.run_until_parked();
    let written = written.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert!(!written[0].group_by_file_name);
  }

  #[gpui_kit::test]
  fn rapid_updates_keep_the_newest_config(cx: &mut TestAppContext) {
    let (persistence, written) = recorder();
    let store = cx.new(|cx| ConfigStore::new(Config::default(), persistence, cx));
    store.update(cx, |store, cx| {
      store.update(cx, |config| config.language = Language::De);
      store.update(cx, |config| config.language = Language::Fr);
      store.update(cx, |config| config.group_mode = GroupMode::None);
    });
    cx.run_until_parked();
    let written = written.lock().unwrap();
    let last = written.last().expect("a write");
    assert_eq!(last.language, Language::Fr);
    assert!(matches!(last.group_mode, GroupMode::None));
    assert_eq!(store.read_with(cx, |store, _| store.config().language), Language::Fr);
  }

  #[test]
  fn a_write_older_than_the_last_one_is_dropped() {
    let (persistence, written) = recorder();
    let newer = Config {
      language: Language::Ja,
      ..Config::default()
    };
    persistence.write(2, newer);
    persistence.write(1, Config::default());
    persistence.write(2, Config::default());
    let written = written.lock().unwrap();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].language, Language::Ja);
  }

  #[gpui_kit::test]
  fn flush_writes_only_what_is_not_on_disk(cx: &mut TestAppContext) {
    let (persistence, written) = recorder();
    let store = cx.new(|cx| ConfigStore::new(Config::default(), persistence, cx));
    store.update(cx, |store, cx| {
      store.update(cx, |config| config.language = Language::It)
    });
    store.read_with(cx, |store, _| store.flush());
    assert_eq!(written.lock().unwrap().len(), 1);
    cx.run_until_parked();
    store.read_with(cx, |store, _| store.flush());
    assert_eq!(
      written.lock().unwrap().len(),
      1,
      "the background write of the same revision is dropped"
    );
  }
}
