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

//! The persisted configuration (`bmm_core::config::Config`) and the profile
//! edits of the React store (`addProfile`, `deleteActiveProfile`,
//! `setActiveProfile`, `resetActiveProfileTemplates`,
//! `updateActiveProfile`).

use std::sync::{Arc, Mutex};

use bmm_core::config::{Config, ConfigProfile};
use gpui_kit::{AppContext, Context, Subscription};

/// The active profile, else the first one (`getActiveProfile`).
pub fn active_profile(config: &Config) -> Option<&ConfigProfile> {
  active_profile_index(config).map(|index| &config.profiles[index])
}

/// The index of [`active_profile`].
fn active_profile_index(config: &Config) -> Option<usize> {
  config
    .profiles
    .iter()
    .position(|profile| profile.name == config.active_profile)
    .or_else(|| (!config.profiles.is_empty()).then_some(0))
}

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

  /// Edits the profile the Settings show (`updateActiveProfile`): the active
  /// one, else the first (React edited nothing when the name matched none).
  pub fn update_active_profile(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut ConfigProfile)) {
    if let Some(index) = active_profile_index(&self.config) {
      self.update(cx, |config| edit(&mut config.profiles[index]));
    }
  }

  /// Whether `name` (trimmed) can become a new profile: not empty and not
  /// taken.
  pub fn can_add_profile(&self, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && !self.config.profiles.iter().any(|profile| profile.name == name)
  }

  /// Adds a fresh profile called `name` (trimmed) and makes it active
  /// (`addProfile`); `false` when [`Self::can_add_profile`] refuses it.
  pub fn add_profile(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
    if !self.can_add_profile(name) {
      return false;
    }
    let name = name.trim().to_string();
    self.update(cx, |config| {
      config.profiles.push(ConfigProfile::named(&name));
      config.active_profile = name;
    });
    true
  }

  /// Whether the active profile can be deleted: every one but Default.
  pub fn can_delete_active_profile(&self) -> bool {
    self.config.active_profile != ConfigProfile::DEFAULT_NAME
  }

  /// Deletes the active profile; Default becomes active
  /// (`deleteActiveProfile`). The Default profile stays.
  pub fn delete_active_profile(&mut self, cx: &mut Context<Self>) {
    if !self.can_delete_active_profile() {
      return;
    }
    self.update(cx, |config| {
      let name = std::mem::replace(&mut config.active_profile, ConfigProfile::DEFAULT_NAME.to_string());
      config.profiles.retain(|profile| profile.name != name);
    });
  }

  /// Makes the profile called `name` active, if there is one
  /// (`setActiveProfile`).
  pub fn set_active_profile(&mut self, name: &str, cx: &mut Context<Self>) {
    if self.config.active_profile != name && self.config.profiles.iter().any(|profile| profile.name == name) {
      let name = name.to_string();
      self.update(cx, |config| config.active_profile = name);
    }
  }

  /// Puts the active profile back to a fresh profile of the same name
  /// (`resetActiveProfileTemplates`).
  pub fn reset_active_profile(&mut self, cx: &mut Context<Self>) {
    self.update_active_profile(cx, |profile| *profile = ConfigProfile::named(&profile.name));
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
  use gpui_kit::{Entity, TestAppContext};

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
  fn the_active_profile_falls_back_to_the_first() {
    let mut config = Config::default();
    let mut other = config.profiles[0].clone();
    other.name = "Other".to_string();
    config.profiles.push(other);
    config.active_profile = "Other".to_string();
    assert_eq!(
      active_profile(&config).map(|profile| profile.name.as_str()),
      Some("Other")
    );
    config.active_profile = "Missing".to_string();
    assert_eq!(
      active_profile(&config).map(|profile| profile.name.as_str()),
      Some(config.profiles[0].name.as_str())
    );
    config.profiles.clear();
    assert!(active_profile(&config).is_none());
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

  fn store(cx: &mut TestAppContext) -> (Entity<ConfigStore>, Arc<Mutex<Vec<Config>>>) {
    let (persistence, written) = recorder();
    (
      cx.new(|cx| ConfigStore::new(Config::default(), persistence, cx)),
      written,
    )
  }

  fn names(store: &Entity<ConfigStore>, cx: &mut TestAppContext) -> (Vec<String>, String) {
    store.read_with(cx, |store, _| {
      let config = store.config();
      (
        config.profiles.iter().map(|profile| profile.name.clone()).collect(),
        config.active_profile.clone(),
      )
    })
  }

  #[gpui_kit::test]
  fn a_new_profile_is_fresh_and_becomes_active(cx: &mut TestAppContext) {
    let (store, written) = store(cx);
    assert!(store.update(cx, |store, cx| store.add_profile("  Anime ", cx)));
    assert_eq!(
      names(&store, cx),
      (vec!["Default".into(), "Anime".into()], "Anime".into())
    );
    let anime = store.read_with(cx, |store, _| store.config().profiles[1].clone());
    assert!(!anime.select_audio && !anime.select_subtitle && !anime.select_chapters);
    // Empty and taken names are refused.
    for name in ["", "   ", "Anime", "Default"] {
      assert!(!store.read_with(cx, |store, _| store.can_add_profile(name)), "{name:?}");
      assert!(!store.update(cx, |store, cx| store.add_profile(name, cx)));
    }
    cx.run_until_parked();
    assert_eq!(written.lock().unwrap().last().unwrap().active_profile, "Anime");
  }

  #[gpui_kit::test]
  fn deleting_the_active_profile_activates_default_and_default_stays(cx: &mut TestAppContext) {
    let (store, _) = store(cx);
    store.update(cx, |store, cx| {
      store.add_profile("Anime", cx);
      store.add_profile("Movies", cx);
      store.set_active_profile("Anime", cx);
    });
    assert!(store.read_with(cx, |store, _| store.can_delete_active_profile()));
    store.update(cx, |store, cx| store.delete_active_profile(cx));
    assert_eq!(
      names(&store, cx),
      (vec!["Default".into(), "Movies".into()], "Default".into())
    );
    assert!(!store.read_with(cx, |store, _| store.can_delete_active_profile()));
    store.update(cx, |store, cx| store.delete_active_profile(cx));
    assert_eq!(names(&store, cx).0, vec!["Default".to_string(), "Movies".to_string()]);
    // An unknown name is not activated.
    store.update(cx, |store, cx| store.set_active_profile("Missing", cx));
    assert_eq!(names(&store, cx).1, "Default");
  }

  #[gpui_kit::test]
  fn reset_puts_the_active_profile_back_to_its_template(cx: &mut TestAppContext) {
    let (store, _) = store(cx);
    store.update(cx, |store, cx| {
      store.add_profile("Anime", cx);
      store.update_active_profile(cx, |profile| {
        profile.select_video = true;
        profile.preferred_audio_languages = "jpn".into();
      });
    });
    let edited = store.read_with(cx, |store, _| store.config().profiles[1].clone());
    assert!(edited.select_video);
    assert_eq!(edited.preferred_audio_languages, "jpn");
    store.update(cx, |store, cx| store.reset_active_profile(cx));
    let reset = store.read_with(cx, |store, _| store.config().profiles[1].clone());
    assert_eq!(
      serde_json::to_value(&reset).unwrap(),
      serde_json::to_value(ConfigProfile::named("Anime")).unwrap()
    );
    // The Default profile was not touched.
    let default = store.read_with(cx, |store, _| store.config().profiles[0].clone());
    assert_eq!(
      serde_json::to_value(&default).unwrap(),
      serde_json::to_value(ConfigProfile::default()).unwrap()
    );
  }

  #[gpui_kit::test]
  fn edits_go_to_the_first_profile_when_the_active_name_matches_none(cx: &mut TestAppContext) {
    let (store, _) = store(cx);
    store.update(cx, |store, cx| {
      store.update(cx, |config| config.active_profile = "Missing".into());
      store.update_active_profile(cx, |profile| profile.select_video = true);
    });
    assert!(store.read_with(cx, |store, _| store.config().profiles[0].select_video));
  }
}
