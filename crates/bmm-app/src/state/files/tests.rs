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

//! Tests of the file store: every method ported from a `store.ts` action,
//! and the loading pipeline.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use bmm_core::config::{Config, ConfigProfile, GroupMode};
use bmm_core::media_metadata::ParseError;
use bmm_core::media_metadata::language::Language;
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::MediaTrackType;
use core::prelude::v1::test;
use gpui_kit::{AppContext, Entity, Subscription, TestAppContext};

use super::*;
use crate::domain::file_tree::FlagKind;
use crate::domain::grouping::RenderEntry;

/// A fresh directory under the system temp dir, removed on drop.
pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
  pub(crate) fn new(name: &str) -> Self {
    let path = std::env::temp_dir().join(format!("bmm-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    Self(path)
  }

  /// Creates an empty file.
  pub(crate) fn file(&self, name: &str) -> String {
    self.write(name, b"")
  }

  pub(crate) fn write(&self, name: &str, content: &[u8]) -> String {
    let path = self.0.join(name);
    std::fs::write(&path, content).unwrap();
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

/// Parsed metadata with one track per `(type, ISO 639-2 language)`, ids from
/// 0; an empty language leaves the track without one (`und`).
pub(crate) fn metadata(tracks: &[(TrackType, &str)]) -> MediaMetadata {
  let mut meta = MediaMetadata::new("fixture.mkv", 1_000);
  for (id, (track_type, language)) in tracks.iter().enumerate() {
    let mut properties = TrackProperties::default();
    if !language.is_empty() {
      properties.common.language = Some(Language::from_iso_639_2(language));
    }
    meta.tracks.push(Track {
      id: id as i64,
      track_type: *track_type,
      codec: CodecInfo {
        id: "V_TEST".to_string(),
        name: None,
        codec_private: None,
      },
      properties,
    });
  }
  meta
}

/// A movie: video, two audio tracks and two subtitles.
pub(crate) fn movie() -> MediaMetadata {
  metadata(&[
    (TrackType::Video, "eng"),
    (TrackType::Audio, "eng"),
    (TrackType::Audio, "ger"),
    (TrackType::Subtitles, "eng"),
    (TrackType::Subtitles, ""),
  ])
}

/// Serves `fixtures` by path and counts the parses; other paths are not
/// recognised.
pub(crate) fn fixture_source(
  fixtures: HashMap<String, MediaMetadata>,
) -> (MetadataSource, Arc<Mutex<HashMap<String, usize>>>) {
  let parses = Arc::new(Mutex::new(HashMap::new()));
  let counter = parses.clone();
  let source = MetadataSource::new(move |file, _| {
    *counter.lock().unwrap().entry(file.to_string()).or_insert(0) += 1;
    fixtures.get(file).cloned().ok_or(ParseError::Unrecognised)
  });
  (source, parses)
}

pub(super) fn settings(profile: ConfigProfile) -> FileSettings {
  FileSettings {
    profile: Some(profile),
    ..FileSettings::from_config(&Config::default())
  }
}

pub(super) fn default_profile() -> ConfigProfile {
  Config::default().profiles[0].clone()
}

/// A profile that selects everything and runs every automation step: `und`
/// becomes German, German subtitles are named "Deutsch".
fn automating_profile() -> ConfigProfile {
  let mut profile = default_profile();
  profile.select_video = false;
  profile.select_audio = false;
  profile.select_subtitle = false;
  profile.automation.reset_und_language.enabled = true;
  profile.automation.reset_und_language.language = "de".to_string();
  profile.automation.set_track_name.enabled = true;
  profile.automation.reset_default_track.enabled = true;
  profile.automation.reset_forced_display.enabled = true;
  profile
    .track_names_subtitle
    .insert("ger".to_string(), "Deutsch\nGerman".to_string());
  profile
}

pub(super) fn s(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

/// A store, the events it emitted and how often it notified.
pub(super) struct Harness {
  pub(super) store: Entity<FileStore>,
  events: Rc<RefCell<Vec<FileEvent>>>,
  notified: Rc<RefCell<usize>>,
  _subscriptions: Vec<Subscription>,
}

impl Harness {
  fn new(cx: &mut TestAppContext, settings: FileSettings, source: MetadataSource) -> Self {
    let store = cx.new(|_| FileStore::with_settings(settings, source));
    let events = Rc::new(RefCell::new(Vec::new()));
    let notified = Rc::new(RefCell::new(0));
    let (event_sink, notify_sink) = (events.clone(), notified.clone());
    let subscriptions = cx.update(|cx| {
      vec![
        cx.subscribe(&store, move |_, event: &FileEvent, _| {
          event_sink.borrow_mut().push(event.clone())
        }),
        cx.observe(&store, move |_, _| *notify_sink.borrow_mut() += 1),
      ]
    });
    Self {
      store,
      events,
      notified,
      _subscriptions: subscriptions,
    }
  }

  /// A store whose files are the keys of `fixtures`, added in `order` and
  /// parsed.
  pub(super) fn loaded(
    cx: &mut TestAppContext,
    settings: FileSettings,
    order: &[&str],
    fixtures: HashMap<String, MediaMetadata>,
  ) -> Self {
    let (source, _) = fixture_source(fixtures);
    let harness = Self::new(cx, settings, source);
    harness.update(cx, |store, cx| store.add_files(s(order), cx));
    cx.run_until_parked();
    harness.take_events();
    harness.take_notified();
    harness
  }

  pub(super) fn update<R>(
    &self,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut FileStore, &mut Context<FileStore>) -> R,
  ) -> R {
    self.store.update(cx, f)
  }

  pub(super) fn read<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&FileStore) -> R) -> R {
    self.store.read_with(cx, |store, _| f(store))
  }

  pub(super) fn take_events(&self) -> Vec<FileEvent> {
    std::mem::take(&mut self.events.borrow_mut())
  }

  pub(super) fn take_notified(&self) -> usize {
    std::mem::take(&mut self.notified.borrow_mut())
  }
}

pub(super) fn row<'a>(store: &'a FileStore, file: &str, key: &str) -> &'a bmm_core::media_track::MediaTrack {
  store
    .tracks(file)
    .unwrap()
    .iter()
    .find(|track| track.key() == key)
    .unwrap()
}

#[gpui_kit::test]
fn add_files_appends_new_paths_in_order(cx: &mut TestAppContext) {
  let (source, _) = fixture_source(HashMap::new());
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| store.add_files(s(&["b", "a"]), cx));
  harness.update(cx, |store, cx| store.add_files(s(&["a", "c", "c"]), cx));
  assert_eq!(harness.take_notified(), 2);
  harness.update(cx, |store, cx| store.add_files(s(&["a"]), cx));
  assert_eq!(harness.take_notified(), 0, "adding nothing does not notify");
  harness.read(cx, |store| assert_eq!(store.files(), ["b", "a", "c"]));
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
  let (source, _) = fixture_source(HashMap::new());
  let store = cx.new(|_| FileStore::with_settings(settings(default_profile()), source));
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

#[gpui_kit::test]
fn a_parsed_file_gets_rows_counts_and_the_profile_selection(cx: &mut TestAppContext) {
  let harness = Harness::loaded(
    cx,
    settings(default_profile()),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), movie())]),
  );
  harness.read(cx, |store| {
    assert!(matches!(store.load_state("/m/a.mkv"), LoadState::Loaded));
    assert_eq!(store.tracks("/m/a.mkv").unwrap().len(), 5);
    let counts = store.track_counts("/m/a.mkv").unwrap();
    assert_eq!((counts.video, counts.audio, counts.subtitles), (1, 2, 2));
    assert_eq!(store.metadata("/m/a.mkv").unwrap().file_size, 1_000);
    // The default profile: every video, English and German audio and
    // subtitles; not the subtitle without a language.
    assert_eq!(
      store.selected_ids("/m/a.mkv").unwrap(),
      ["video:0", "audio:1", "audio:2", "subtitles:3"]
    );
    assert_eq!(store.selected_tracks("/m/a.mkv").len(), 4);
    assert!(store.has_selection());
  });
}

#[gpui_kit::test]
fn a_parse_failure_is_kept_and_not_retried(cx: &mut TestAppContext) {
  let (source, parses) = fixture_source(HashMap::from([("/m/good.mkv".to_string(), movie())]));
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| store.add_files(s(&["/m/good.mkv", "/m/bad.mkv"]), cx));
  harness.read(cx, |store| {
    assert!(matches!(store.load_state("/m/bad.mkv"), LoadState::Loading));
    assert!(matches!(
      store.unit_load_state(&s(&["/m/good.mkv", "/m/bad.mkv"])),
      LoadState::Loading
    ));
  });
  cx.run_until_parked();
  harness.update(cx, |store, cx| store.set_settings(settings(default_profile()), cx));
  cx.run_until_parked();
  harness.read(cx, |store| {
    assert!(matches!(
      store.load_state("/m/bad.mkv"),
      LoadState::Failed(MediaMetadataErrorPayload::Unrecognised { .. })
    ));
    assert!(matches!(
      store.unit_load_state(&s(&["/m/good.mkv", "/m/bad.mkv"])),
      LoadState::Failed(_)
    ));
    assert!(matches!(store.unit_load_state(&s(&["/m/good.mkv"])), LoadState::Loaded));
  });
  assert_eq!(parses.lock().unwrap()["/m/bad.mkv"], 1);
}

#[gpui_kit::test]
fn a_file_removed_while_parsing_drops_its_result(cx: &mut TestAppContext) {
  let (source, parses) = fixture_source(HashMap::from([("/m/a.mkv".to_string(), movie())]));
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| {
    store.add_files(s(&["/m/a.mkv"]), cx);
    store.remove_file("/m/a.mkv", cx);
  });
  cx.run_until_parked();
  harness.read(cx, |store| assert!(store.tracks("/m/a.mkv").is_none()));
  // Removed and added again while the parse runs: the result is kept, and
  // the file is not parsed twice.
  harness.update(cx, |store, cx| {
    store.add_files(s(&["/m/a.mkv"]), cx);
    store.remove_file("/m/a.mkv", cx);
    store.add_files(s(&["/m/a.mkv"]), cx);
  });
  cx.run_until_parked();
  harness.read(cx, |store| assert!(store.tracks("/m/a.mkv").is_some()));
  assert_eq!(parses.lock().unwrap()["/m/a.mkv"], 2);
}

/// Plan acceptance: 200 files load through the background executor, each is
/// parsed once although loading is requested again while parses run, and the
/// automation runs exactly once per unit. The files are real subtitle files
/// read by the native parser.
#[gpui_kit::test]
fn two_hundred_files_are_parsed_once_and_each_unit_is_automated_once(cx: &mut TestAppContext) {
  let dir = TempDir::new("two-hundred");
  let subtitle = b"1\n00:00:01,000 --> 00:00:02,000\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\nWorld\n";
  let mut paths = Vec::new();
  for index in 0..100 {
    paths.push(dir.write(&format!("ep{index:03}.srt"), subtitle));
    paths.push(dir.write(&format!("ep{index:03}.forced.srt"), subtitle));
  }
  let parses = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
  let counter = parses.clone();
  let parser = MetadataSource::parser();
  let source = MetadataSource::new(move |file, options| {
    *counter.lock().unwrap().entry(file.to_string()).or_insert(0) += 1;
    parser.read(file, options)
  });
  let harness = Harness::new(cx, settings(automating_profile()), source);
  harness.update(cx, |store, cx| {
    store.add_files(paths.clone(), cx);
    // Asking again while the parses run starts none twice.
    store.add_files(paths.clone(), cx);
    store.set_settings(settings(automating_profile()), cx);
  });
  harness.read(cx, |store| assert_eq!(store.in_flight.len(), 200));
  cx.run_until_parked();
  {
    let parses = parses.lock().unwrap();
    assert_eq!(parses.len(), 200);
    assert!(parses.values().all(|count| *count == 1), "every file parsed once");
  }
  harness.read(cx, |store| {
    assert_eq!(store.units().len(), 100, "a root and its forced subtitle per unit");
    assert_eq!(store.automation_runs(), 100, "one automation per unit");
    // Alike units share one group card.
    assert!(matches!(store.entries(), [RenderEntry::Group { units, .. }] if units.len() == 100));
    for unit in store.units() {
      let (root, child) = (&unit[0], &unit[1]);
      let track = &store.tracks(root).unwrap()[0];
      assert_eq!(track.track_type, MediaTrackType::Subtitles);
      assert_eq!(track.language, "de", "und became the reset language");
      assert_eq!(track.track_name, "Deutsch", "the first preset of the new language");
      assert_eq!(store.selected_ids(root).unwrap(), ["subtitles:0"]);
      // The unit's first subtitle is the default, the other is not.
      assert_eq!(track.default_track, TrackFlag::True);
      assert_eq!(store.tracks(child).unwrap()[0].default_track, TrackFlag::False);
    }
  });
  // A membership change runs the flag steps of the new units; going back to
  // units seen before does not run them again.
  harness.update(cx, |store, cx| {
    let mut settings = settings(automating_profile());
    settings.group_by_file_name = false;
    store.set_settings(settings, cx);
  });
  harness.read(cx, |store| assert_eq!(store.automation_runs(), 300));
  harness.update(cx, |store, cx| store.set_settings(settings(automating_profile()), cx));
  harness.read(cx, |store| assert_eq!(store.automation_runs(), 300));
}

#[gpui_kit::test]
fn the_language_is_reset_before_the_selection_filters_by_it(cx: &mut TestAppContext) {
  let mut profile = automating_profile();
  profile.select_subtitle = true;
  profile.subtitle_languages_for_track_selection = "de".to_string();
  let harness = Harness::loaded(
    cx,
    settings(profile),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), movie())]),
  );
  harness.read(cx, |store| {
    let selected = store.selected_ids("/m/a.mkv").unwrap();
    assert!(
      selected.contains(&"subtitles:4".to_string()),
      "the und subtitle, now German"
    );
    assert!(!selected.contains(&"subtitles:3".to_string()), "the English one");
    // Only selected tracks take part in the flag steps.
    assert_eq!(row(store, "/m/a.mkv", "subtitles:4").default_track, TrackFlag::True);
    assert_eq!(
      row(store, "/m/a.mkv", "subtitles:3").default_track,
      TrackFlag::Unspecified
    );
  });
}

#[gpui_kit::test]
fn automation_keeps_an_existing_selection(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([("/m/a.mkv".to_string(), movie()), ("/m/b.mkv".to_string(), movie())]);
  let mut settings = settings(automating_profile());
  settings.group_mode = GroupMode::None;
  let harness = Harness::loaded(cx, settings, &["/m/a.mkv", "/m/b.mkv"], fixtures);
  harness.update(cx, |store, cx| {
    store.set_file_selected_ids("/m/a.mkv", s(&["audio:2"]), cx);
    store.set_track_flag(
      &s(&["/m/a.mkv"]),
      &s(&["audio:2"]),
      FlagKind::Default,
      TrackFlag::False,
      cx,
    );
    assert_eq!(store.automation_runs(), 2);
    store.merge_card_into("/m/b.mkv", "/m/a.mkv", cx);
    // The merged unit is new: its flag steps run over the kept selection.
    assert_eq!(store.automation_runs(), 3);
    assert_eq!(store.selected_ids("/m/a.mkv").unwrap(), ["audio:2"]);
    assert_eq!(row(store, "/m/a.mkv", "audio:2").default_track, TrackFlag::True);
  });
}

#[gpui_kit::test]
fn remove_file_drops_everything_about_it(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([
    ("/m/a.mkv".to_string(), movie()),
    ("/m/b.mkv".to_string(), movie()),
    ("/m/c.mkv".to_string(), movie()),
  ]);
  let harness = Harness::loaded(
    cx,
    settings(default_profile()),
    &["/m/a.mkv", "/m/b.mkv", "/m/c.mkv"],
    fixtures,
  );
  harness.update(cx, |store, cx| {
    store.set_file_output_path("/m/a.mkv", "/out/a.mkv".to_string(), cx);
    store.set_group_output_dir(&s(&["/m/b.mkv"]), "/out", cx);
    store.set_combined_track_order("/m/a.mkv", s(&["x"]), cx);
    store.merge_card_into("/m/b.mkv", "/m/a.mkv", cx);
    store.merge_card_into("/m/a.mkv", "/m/c.mkv", cx);
    store.set_active_card(Some("/m/a.mkv".to_string()), cx);
    store.remove_file("/m/a.mkv", cx);
    store.remove_file("/m/missing.mkv", cx);
    assert_eq!(store.files(), ["/m/b.mkv", "/m/c.mkv"]);
    for gone in [
      store.metadata("/m/a.mkv").is_some(),
      store.tracks("/m/a.mkv").is_some(),
      store.track_counts("/m/a.mkv").is_some(),
      store.selected_ids("/m/a.mkv").is_some(),
      store.output_path("/m/a.mkv").is_some(),
      store.combined_track_order("/m/a.mkv").is_some(),
      store.merged_root("/m/a.mkv").is_some(),
      store.merged_root("/m/b.mkv").is_some(),
      store.active_card().is_some(),
    ] {
      assert!(!gone);
    }
    assert_eq!(store.output_dir("/m/b.mkv"), Some("/out"));
  });
}

#[gpui_kit::test]
fn clear_files_keeps_the_global_output_dir(cx: &mut TestAppContext) {
  let harness = Harness::loaded(
    cx,
    settings(default_profile()),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), movie())]),
  );
  harness.update(cx, |store, cx| {
    store.set_global_output_dir("/out".to_string(), cx);
    store.set_file_output_path("/m/a.mkv", "/x.mkv".to_string(), cx);
    store.clear_files(cx);
    assert!(!store.has_files());
    assert!(store.tracks("/m/a.mkv").is_none());
    assert!(store.output_path("/m/a.mkv").is_none());
    assert!(!store.has_selection());
    assert!(store.units().is_empty());
    assert_eq!(store.global_output_dir(), Some("/out"));
    store.set_global_output_dir(String::new(), cx);
    assert_eq!(store.global_output_dir(), None);
  });
}

#[gpui_kit::test]
fn merging_and_detaching_cards_change_the_units(cx: &mut TestAppContext) {
  let mut settings = settings(default_profile());
  settings.group_mode = GroupMode::None;
  let files = ["/m/a.mkv", "/m/a.en.srt", "/m/b.mkv"];
  let fixtures = files.iter().map(|file| (file.to_string(), movie())).collect();
  let harness = Harness::loaded(cx, settings, &files, fixtures);
  harness.update(cx, |store, cx| {
    assert_eq!(store.units(), [s(&["/m/a.mkv", "/m/a.en.srt"]), s(&["/m/b.mkv"])]);
    store.merge_card_into("/m/b.mkv", "/m/b.mkv", cx);
    store.merge_card_into("/m/b.mkv", "/m/a.mkv", cx);
    assert_eq!(store.units(), [s(&["/m/a.mkv", "/m/a.en.srt", "/m/b.mkv"])]);
    // A drag-merged root un-merges with its own unit.
    store.detach_file_from_root("/m/b.mkv", cx);
    assert_eq!(store.units().len(), 2);
    // Any other member becomes a card alone, and re-merging undoes that.
    store.detach_file_from_root("/m/a.en.srt", cx);
    assert!(store.is_detached("/m/a.en.srt"));
    assert_eq!(store.units(), [s(&["/m/a.mkv"]), s(&["/m/b.mkv"]), s(&["/m/a.en.srt"])]);
    store.merge_card_into("/m/a.en.srt", "/m/b.mkv", cx);
    assert!(!store.is_detached("/m/a.en.srt"));
    assert_eq!(store.merged_root("/m/a.en.srt"), Some("/m/b.mkv"));
  });
  assert!(harness.take_notified() > 0, "unit changes are structural");
}

#[gpui_kit::test]
fn track_edits_apply_to_matching_rows_of_the_given_files(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([("/m/a.mkv".to_string(), movie()), ("/m/b.mkv".to_string(), movie())]);
  let harness = Harness::loaded(cx, settings(default_profile()), &["/m/a.mkv", "/m/b.mkv"], fixtures);
  let both = s(&["/m/a.mkv", "/m/b.mkv", "/m/missing.mkv"]);
  harness.update(cx, |store, cx| {
    store.set_track_flag(&both, &s(&["audio:1"]), FlagKind::Default, TrackFlag::True, cx);
    store.set_track_flag(
      &s(&["/m/a.mkv"]),
      &s(&["audio:2"]),
      FlagKind::Forced,
      TrackFlag::False,
      cx,
    );
    store.set_track_flag(&both, &[], FlagKind::Forced, TrackFlag::True, cx);
    store.set_track_language(&both, &s(&["subtitles:4"]), "ja", cx);
    store.set_track_name(&s(&["/m/b.mkv"]), &s(&["video:0"]), "Main", cx);
    for file in ["/m/a.mkv", "/m/b.mkv"] {
      assert_eq!(row(store, file, "audio:1").default_track, TrackFlag::True);
      let subtitle = row(store, file, "subtitles:4");
      assert_eq!(subtitle.language, "ja");
      assert_eq!(subtitle.language_codes, ["jpn", "ja"]);
    }
    assert_eq!(row(store, "/m/a.mkv", "audio:2").forced, TrackFlag::False);
    assert_eq!(row(store, "/m/b.mkv", "audio:2").forced, TrackFlag::Unspecified);
    assert_eq!(row(store, "/m/b.mkv", "video:0").track_name, "Main");
    assert_eq!(row(store, "/m/a.mkv", "video:0").track_name, "");
  });
  let events = harness.take_events();
  assert_eq!(
    events.first(),
    Some(&FileEvent::Changed {
      files: s(&["/m/a.mkv", "/m/b.mkv"])
    })
  );
  assert_eq!(harness.take_notified(), 0, "row edits are not structural");
}

#[gpui_kit::test]
fn a_language_edit_regroups_under_the_language_mode(cx: &mut TestAppContext) {
  let mut settings = settings(default_profile());
  settings.group_mode = GroupMode::TrackCountAndLanguage;
  let fixtures = HashMap::from([("/m/a.mkv".to_string(), movie()), ("/m/b.mkv".to_string(), movie())]);
  let harness = Harness::loaded(cx, settings, &["/m/a.mkv", "/m/b.mkv"], fixtures);
  harness.update(cx, |store, cx| {
    assert!(matches!(store.entries(), [RenderEntry::Group { .. }]));
    store.set_track_language(&s(&["/m/a.mkv"]), &s(&["audio:2"]), "fr", cx);
    assert!(matches!(
      store.entries(),
      [RenderEntry::Single { .. }, RenderEntry::Single { .. }]
    ));
  });
  assert_eq!(harness.take_notified(), 1);
}

#[gpui_kit::test]
fn reorder_moves_a_row_in_files_that_have_both_keys(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([
    ("/m/a.mkv".to_string(), movie()),
    ("/m/b.mkv".to_string(), metadata(&[(TrackType::Video, "eng")])),
  ]);
  let harness = Harness::loaded(cx, settings(default_profile()), &["/m/a.mkv", "/m/b.mkv"], fixtures);
  harness.update(cx, |store, cx| {
    let files = s(&["/m/a.mkv", "/m/b.mkv"]);
    store.reorder_tracks(&files, "subtitles:4", "audio:1", cx);
    store.reorder_tracks(&files, "audio:1", "audio:1", cx);
    let keys: Vec<String> = store
      .tracks("/m/a.mkv")
      .unwrap()
      .iter()
      .map(|track| track.key())
      .collect();
    assert_eq!(keys, ["video:0", "subtitles:4", "audio:1", "audio:2", "subtitles:3"]);
    assert_eq!(store.tracks("/m/b.mkv").unwrap().len(), 1);
    assert_eq!(store.tracks("/m/a.mkv").unwrap()[1].id, 4, "ids are unchanged");
  });
}

#[gpui_kit::test]
fn output_overrides(cx: &mut TestAppContext) {
  let (source, _) = fixture_source(HashMap::new());
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| {
    store.add_files(s(&["/m/a.mkv", "/m/b.mkv"]), cx);
    store.set_group_output_dir(&s(&["/m/a.mkv", "/m/b.mkv"]), "/out", cx);
    store.set_file_output_path("/m/a.mkv", "/elsewhere/a.mkv".to_string(), cx);
    assert_eq!(store.output_dir("/m/a.mkv"), None, "a full path replaces the directory");
    assert_eq!(store.output_path("/m/a.mkv"), Some("/elsewhere/a.mkv"));
    store.clear_file_output_path("/m/a.mkv", cx);
    assert_eq!(store.output_path("/m/a.mkv"), None);
    store.clear_group_output_dir(&s(&["/m/b.mkv"]), cx);
    assert_eq!(store.output_dir("/m/b.mkv"), None);
    store.set_better_media_info(Some("/opt/bmi".into()), cx);
    assert!(store.better_media_info_available());
    assert_eq!(store.better_media_info_dir(), Some("/opt/bmi"));
  });
}

#[gpui_kit::test]
fn apply_automation_to_file_resets_und_then_names_by_the_new_language(cx: &mut TestAppContext) {
  let mut meta = movie();
  meta.chapters.num_entries = 3;
  let mut profile = default_profile();
  profile.select_subtitle = false;
  let harness = Harness::loaded(
    cx,
    settings(profile),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), meta)]),
  );
  let automation = automating_profile().automation;
  harness.update(cx, |store, cx| {
    store.apply_automation_to_file(
      "/m/a.mkv",
      &automation,
      |track_type, language| (track_type == MediaTrackType::Subtitles).then(|| format!("{language} subs")),
      cx,
    );
    assert_eq!(row(store, "/m/a.mkv", "subtitles:4").language, "de");
    assert_eq!(row(store, "/m/a.mkv", "subtitles:4").track_name, "de subs");
    assert_eq!(row(store, "/m/a.mkv", "subtitles:3").track_name, "en subs");
    assert_eq!(row(store, "/m/a.mkv", "audio:1").track_name, "");
    assert_eq!(
      row(store, "/m/a.mkv", "chapters:0").track_name,
      "",
      "chapters are left alone"
    );
    // Nothing enabled: nothing changes.
    store.apply_automation_to_file(
      "/m/a.mkv",
      &ConfigAutomation::default(),
      |_, _| Some("x".to_string()),
      cx,
    );
    assert_eq!(row(store, "/m/a.mkv", "audio:1").track_name, "");
  });
}

#[gpui_kit::test]
fn the_selection_flag_notifies_only_when_it_flips(cx: &mut TestAppContext) {
  let mut profile = default_profile();
  profile.select_video = true;
  profile.video_languages_for_track_selection = "xx".to_string();
  profile.audio_languages_for_track_selection = "xx".to_string();
  profile.subtitle_languages_for_track_selection = "xx".to_string();
  let harness = Harness::loaded(
    cx,
    settings(profile),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), movie())]),
  );
  harness.update(cx, |store, cx| {
    assert!(!store.has_selection(), "the filter matches nothing");
    store.set_file_selected_ids("/m/a.mkv", s(&["video:0"]), cx);
  });
  assert_eq!(harness.take_notified(), 1);
  harness.update(cx, |store, cx| {
    store.set_file_selected_ids("/m/a.mkv", s(&["video:0", "audio:1"]), cx)
  });
  assert_eq!(harness.take_notified(), 0);
  harness.update(cx, |store, cx| store.set_file_selected_ids("/m/a.mkv", Vec::new(), cx));
  assert_eq!(harness.take_notified(), 1);
}

#[gpui_kit::test]
fn highlights_name_the_cards_they_leave_and_enter(cx: &mut TestAppContext) {
  let (source, _) = fixture_source(HashMap::new());
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| {
    store.set_drop_target(Some("/m/a.mkv".to_string()), cx);
    store.set_drop_target(Some("/m/b.mkv".to_string()), cx);
    store.set_drop_target(Some("/m/b.mkv".to_string()), cx);
    store.set_active_card(Some("group".to_string()), cx);
    store.set_drop_target(None, cx);
    assert_eq!(store.drop_target_root(), None);
    assert_eq!(store.active_card(), Some("group"));
  });
  assert_eq!(
    harness.take_events(),
    [
      FileEvent::Highlight { ids: s(&["/m/a.mkv"]) },
      FileEvent::Highlight {
        ids: s(&["/m/a.mkv", "/m/b.mkv"])
      },
      FileEvent::Highlight { ids: s(&["group"]) },
      FileEvent::Highlight { ids: s(&["/m/b.mkv"]) },
    ]
  );
  assert_eq!(harness.take_notified(), 0);
}

#[gpui_kit::test]
fn the_store_follows_the_config(cx: &mut TestAppContext) {
  let config = cx.new(|cx| ConfigStore::new(Config::default(), crate::state::Persistence::new(|_| {}), cx));
  let (source, _) = fixture_source(HashMap::new());
  let store = cx.new(|cx| FileStore::new(&config, source, cx));
  store.update(cx, |store, cx| store.add_files(s(&["/m/a.mkv", "/m/a.en.srt"]), cx));
  assert_eq!(store.read_with(cx, |store, _| store.units().len()), 1);
  config.update(cx, |config, cx| {
    config.update(cx, |config| config.group_by_file_name = false)
  });
  cx.run_until_parked();
  assert_eq!(store.read_with(cx, |store, _| store.units().len()), 2);
}

#[gpui_kit::test]
fn set_file_metadata_stores_rows_for_listed_files_only(cx: &mut TestAppContext) {
  let (source, _) = fixture_source(HashMap::new());
  let harness = Harness::new(cx, settings(default_profile()), source);
  harness.update(cx, |store, cx| store.add_files(s(&["/m/a.mkv"]), cx));
  cx.run_until_parked();
  harness.take_notified();
  harness.update(cx, |store, cx| {
    assert!(matches!(store.load_state("/m/a.mkv"), LoadState::Failed(_)));
    store.set_file_metadata("/m/a.mkv", movie(), cx);
    store.set_file_metadata("/m/unlisted.mkv", movie(), cx);
    assert!(
      matches!(store.load_state("/m/a.mkv"), LoadState::Loaded),
      "replaces the failure"
    );
    assert_eq!(store.tracks("/m/a.mkv").unwrap().len(), 5);
    assert!(
      store.selected_ids("/m/a.mkv").is_some(),
      "the profile's selection followed"
    );
    assert!(store.tracks("/m/unlisted.mkv").is_none());
  });
  assert_eq!(harness.take_notified(), 1);
}
