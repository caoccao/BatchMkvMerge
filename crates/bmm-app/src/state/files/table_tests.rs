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

//! Tests of the track tables' edits: a single card's unit, a multi-member
//! unit with its combined order, and a group card fanning out.

use std::collections::{HashMap, HashSet};

use bmm_core::config::GroupMode;
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track::TrackType;
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

use super::tests::{Harness, default_profile, metadata, movie, row, s, settings};
use super::*;
use crate::domain::file_tree::FlagKind;
use crate::domain::grouping::RenderEntry;

fn keys(store: &FileStore, scope: &TableScope) -> Vec<String> {
  scope.rows(store).iter().map(|row| row.row_key()).collect()
}

fn checked(store: &FileStore, scope: &TableScope) -> Vec<String> {
  let mut checked: Vec<String> = scope.checked_rows(store).into_iter().collect();
  checked.sort();
  checked
}

fn subtitle(language: &str) -> MediaMetadata {
  metadata(&[(TrackType::Subtitles, language)])
}

/// One movie on a card of its own; the default profile selected its video,
/// the English and German audio and the English subtitle.
fn single(cx: &mut TestAppContext) -> (Harness, TableScope) {
  let harness = Harness::loaded(
    cx,
    settings(default_profile()),
    &["/m/a.mkv"],
    HashMap::from([("/m/a.mkv".to_string(), movie())]),
  );
  (harness, TableScope::unit(s(&["/m/a.mkv"])))
}

/// Two structurally identical movies in one group card.
fn group(cx: &mut TestAppContext, files: &[&str], fixtures: HashMap<String, MediaMetadata>) -> (Harness, TableScope) {
  let mut settings = settings(default_profile());
  settings.group_mode = GroupMode::TrackCount;
  let harness = Harness::loaded(cx, settings, files, fixtures);
  let scope = harness.read(cx, |store| {
    let [entry @ RenderEntry::Group { .. }] = store.entries() else {
      panic!("one group card: {:?}", store.entries());
    };
    TableScope::from_entry(entry)
  });
  (harness, scope)
}

#[gpui_kit::test]
fn a_single_cards_rows_and_checkboxes(cx: &mut TestAppContext) {
  let (harness, scope) = single(cx);
  assert_eq!(scope.id(), "/m/a.mkv");
  assert!(!scope.is_group() && !scope.qualified_ids() && scope.can_reorder());
  harness.update(cx, |store, cx| {
    assert_eq!(
      keys(store, &scope),
      ["0:video:0", "0:audio:1", "0:audio:2", "0:subtitles:3", "0:subtitles:4"]
    );
    assert_eq!(
      checked(store, &scope),
      ["0:audio:1", "0:audio:2", "0:subtitles:3", "0:video:0"]
    );
    store.set_all_rows_checked(&scope, false, cx);
    assert!(checked(store, &scope).is_empty());
    assert_eq!(store.selected_ids("/m/a.mkv"), Some(&[][..]));
    store.set_all_rows_checked(&scope, true, cx);
    assert_eq!(checked(store, &scope).len(), 5);

    store.set_rows_checked(&scope, &s(&["0:audio:2", "0:subtitles:4"]), false, cx);
    assert_eq!(checked(store, &scope), ["0:audio:1", "0:subtitles:3", "0:video:0"]);
    // Ticking a ticked row keeps one entry; unknown rows are ignored.
    store.set_rows_checked(&scope, &s(&["0:audio:1", "0:audio:2", "7:audio:1", "bad"]), true, cx);
    assert_eq!(
      store.selected_ids("/m/a.mkv").unwrap(),
      ["video:0", "audio:1", "subtitles:3", "audio:2"]
    );
    store.flip_rows_checked(&scope, &s(&["0:video:0", "0:subtitles:4"]), cx);
    assert_eq!(
      checked(store, &scope),
      ["0:audio:1", "0:audio:2", "0:subtitles:3", "0:subtitles:4"]
    );
  });
}

#[gpui_kit::test]
fn a_single_cards_cell_and_flag_edits(cx: &mut TestAppContext) {
  let (harness, scope) = single(cx);
  harness.take_events();
  harness.update(cx, |store, cx| {
    store.set_rows_language(&scope, &s(&["0:audio:2", "0:subtitles:4"]), "fre", cx);
    store.set_rows_name(&scope, &s(&["0:audio:1"]), "Commentary", cx);
    assert_eq!(row(store, "/m/a.mkv", "audio:2").language, "fre");
    assert_eq!(row(store, "/m/a.mkv", "subtitles:4").language_codes, ["fre", "fr"]);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").track_name, "Commentary");
    // The clicked row's next value goes to every row of the click.
    store.cycle_rows_flag(
      &scope,
      "0:audio:1",
      &s(&["0:audio:1", "0:audio:2"]),
      FlagKind::Default,
      cx,
    );
    assert_eq!(row(store, "/m/a.mkv", "audio:1").default_track, TrackFlag::True);
    assert_eq!(row(store, "/m/a.mkv", "audio:2").default_track, TrackFlag::True);
    store.cycle_rows_flag(&scope, "0:audio:1", &s(&["0:audio:1"]), FlagKind::Default, cx);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").default_track, TrackFlag::False);
    store.cycle_rows_flag(&scope, "0:audio:1", &s(&["0:audio:1"]), FlagKind::Forced, cx);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").forced, TrackFlag::True);
    // An unknown clicked row changes nothing.
    store.cycle_rows_flag(&scope, "0:audio:9", &s(&["0:audio:2"]), FlagKind::Default, cx);
    assert_eq!(row(store, "/m/a.mkv", "audio:2").default_track, TrackFlag::True);

    // The headers: the first checked track of each type becomes default, the
    // other checked ones lose it; forced is cleared on the checked tracks.
    store.reset_rows_flags(&scope, true, false, cx);
    assert_eq!(row(store, "/m/a.mkv", "video:0").default_track, TrackFlag::True);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").default_track, TrackFlag::True);
    assert_eq!(row(store, "/m/a.mkv", "audio:2").default_track, TrackFlag::False);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").forced, TrackFlag::True);
    store.reset_rows_flags(&scope, false, true, cx);
    assert_eq!(row(store, "/m/a.mkv", "audio:1").forced, TrackFlag::Unspecified);
  });
  let events = harness.take_events();
  assert!(!events.is_empty());
  assert!(events.iter().all(|event| *event
    == FileEvent::Changed {
      files: s(&["/m/a.mkv"])
    }));
}

#[gpui_kit::test]
fn a_single_member_unit_reorders_its_files_rows(cx: &mut TestAppContext) {
  let (harness, scope) = single(cx);
  harness.update(cx, |store, cx| {
    store.reorder_rows(&scope, "0:audio:2", "0:audio:1", cx);
    assert_eq!(
      keys(store, &scope),
      ["0:video:0", "0:audio:2", "0:audio:1", "0:subtitles:3", "0:subtitles:4"]
    );
    // A row moved across types lands in the file's list, but the table
    // still groups the rows by type.
    store.reorder_rows(&scope, "0:subtitles:4", "0:video:0", cx);
    assert_eq!(
      keys(store, &scope),
      ["0:video:0", "0:audio:2", "0:audio:1", "0:subtitles:4", "0:subtitles:3"]
    );
    assert!(store.combined_track_order("/m/a.mkv").is_none());
  });
}

#[gpui_kit::test]
fn a_multi_member_unit_keeps_a_combined_order_across_members(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([
    ("/m/a.mkv".to_string(), movie()),
    ("/m/a.en.srt".to_string(), subtitle("eng")),
  ]);
  let harness = Harness::loaded(cx, settings(default_profile()), &["/m/a.mkv", "/m/a.en.srt"], fixtures);
  let scope = TableScope::unit(s(&["/m/a.mkv", "/m/a.en.srt"]));
  assert!(scope.qualified_ids() && scope.can_reorder());
  harness.update(cx, |store, cx| {
    assert_eq!(store.units(), [s(&["/m/a.mkv", "/m/a.en.srt"])]);
    assert_eq!(
      keys(store, &scope),
      [
        "0:video:0",
        "0:audio:1",
        "0:audio:2",
        "0:subtitles:3",
        "0:subtitles:4",
        "1:subtitles:0"
      ]
    );
    // The srt's subtitle moves above the audio tracks of the movie.
    store.reorder_rows(&scope, "1:subtitles:0", "0:audio:1", cx);
    assert_eq!(
      keys(store, &scope),
      [
        "0:video:0",
        "1:subtitles:0",
        "0:audio:1",
        "0:audio:2",
        "0:subtitles:3",
        "0:subtitles:4"
      ]
    );
    assert_eq!(
      store.combined_track_order("/m/a.mkv").unwrap()[1],
      "/m/a.en.srt\0subtitles:0"
    );
    // Edits address the member file of the row.
    store.set_rows_language(&scope, &s(&["1:subtitles:0"]), "de", cx);
    assert_eq!(row(store, "/m/a.en.srt", "subtitles:0").language, "de");
    assert_eq!(row(store, "/m/a.mkv", "subtitles:3").language, "en");
    store.set_rows_checked(&scope, &s(&["1:subtitles:0"]), false, cx);
    assert_eq!(store.selected_ids("/m/a.en.srt"), Some(&[][..]));
    // The header follows the combined order: the srt's subtitle comes first.
    store.set_rows_checked(&scope, &s(&["1:subtitles:0"]), true, cx);
    store.reset_rows_flags(&scope, true, false, cx);
    assert_eq!(row(store, "/m/a.en.srt", "subtitles:0").default_track, TrackFlag::True);
    assert_eq!(row(store, "/m/a.mkv", "subtitles:3").default_track, TrackFlag::False);
  });
}

#[gpui_kit::test]
fn a_group_card_fans_edits_out_to_every_unit(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([("/m/b.mkv".to_string(), movie()), ("/m/c.mkv".to_string(), movie())]);
  let (harness, scope) = group(cx, &["/m/b.mkv", "/m/c.mkv"], fixtures);
  assert_eq!(scope.id(), "/m/b.mkv");
  assert_eq!(scope.roots().collect::<Vec<_>>(), ["/m/b.mkv", "/m/c.mkv"]);
  assert!(scope.is_group() && !scope.qualified_ids() && scope.can_reorder());
  harness.update(cx, |store, cx| {
    store.set_all_rows_checked(&scope, false, cx);
    for file in ["/m/b.mkv", "/m/c.mkv"] {
      assert_eq!(store.selected_ids(file), Some(&[][..]));
    }
    store.set_rows_checked(&scope, &s(&["0:audio:2"]), true, cx);
    store.flip_rows_checked(&scope, &s(&["0:video:0"]), cx);
    store.set_rows_language(&scope, &s(&["0:audio:2"]), "ja", cx);
    store.set_rows_name(&scope, &s(&["0:audio:2"]), "日本語", cx);
    store.cycle_rows_flag(&scope, "0:audio:2", &s(&["0:audio:2"]), FlagKind::Forced, cx);
    for file in ["/m/b.mkv", "/m/c.mkv"] {
      assert_eq!(store.selected_ids(file).unwrap(), ["audio:2", "video:0"], "{file}");
      let audio = row(store, file, "audio:2");
      assert_eq!(
        (audio.language.as_str(), audio.track_name.as_str(), audio.forced),
        ("ja", "日本語", TrackFlag::True)
      );
    }
    // The roots reorder in lock-step.
    store.reorder_rows(&scope, "0:audio:2", "0:audio:1", cx);
    for file in ["/m/b.mkv", "/m/c.mkv"] {
      let order: Vec<String> = store.tracks(file).unwrap().iter().map(|track| track.key()).collect();
      assert_eq!(order[1..3], ["audio:2", "audio:1"], "{file}");
    }
  });
}

#[gpui_kit::test]
fn a_group_header_evaluates_each_unit_on_its_own(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([("/m/b.mkv".to_string(), movie()), ("/m/c.mkv".to_string(), movie())]);
  let (harness, scope) = group(cx, &["/m/b.mkv", "/m/c.mkv"], fixtures);
  harness.update(cx, |store, cx| {
    // Only c's English audio is unchecked, so c's German audio becomes its
    // default while b keeps its English one.
    store.set_file_selected_ids("/m/c.mkv", s(&["video:0", "audio:2"]), cx);
    store.reset_rows_flags(&scope, true, false, cx);
    assert_eq!(row(store, "/m/b.mkv", "audio:1").default_track, TrackFlag::True);
    assert_eq!(row(store, "/m/b.mkv", "audio:2").default_track, TrackFlag::False);
    assert_eq!(row(store, "/m/c.mkv", "audio:1").default_track, TrackFlag::Unspecified);
    assert_eq!(row(store, "/m/c.mkv", "audio:2").default_track, TrackFlag::True);
  });
}

#[gpui_kit::test]
fn a_group_of_multi_member_units_does_not_reorder(cx: &mut TestAppContext) {
  let files = ["/m/b.mkv", "/m/b.en.srt", "/m/c.mkv", "/m/c.en.srt"];
  let fixtures = HashMap::from([
    ("/m/b.mkv".to_string(), movie()),
    ("/m/b.en.srt".to_string(), subtitle("eng")),
    ("/m/c.mkv".to_string(), movie()),
    ("/m/c.en.srt".to_string(), subtitle("eng")),
  ]);
  let (harness, scope) = group(cx, &files, fixtures);
  assert_eq!(
    scope.units(),
    [s(&["/m/b.mkv", "/m/b.en.srt"]), s(&["/m/c.mkv", "/m/c.en.srt"])]
  );
  assert!(scope.qualified_ids() && !scope.can_reorder());
  assert_eq!(scope.files().count(), 4);
  harness.update(cx, |store, cx| {
    let before = keys(store, &scope);
    store.reorder_rows(&scope, "1:subtitles:0", "0:video:0", cx);
    assert_eq!(keys(store, &scope), before);
    assert!(store.combined_track_order("/m/b.mkv").is_none());
    // The second member's row is the srt of every unit.
    store.set_rows_language(&scope, &s(&["1:subtitles:0"]), "fr", cx);
    assert_eq!(row(store, "/m/b.en.srt", "subtitles:0").language, "fr");
    assert_eq!(row(store, "/m/c.en.srt", "subtitles:0").language, "fr");
    assert_eq!(row(store, "/m/b.mkv", "subtitles:3").language, "en");
  });
}

#[gpui_kit::test]
fn the_selection_of_unparsed_files_is_left_alone(cx: &mut TestAppContext) {
  let (harness, _) = single(cx);
  let scope = TableScope::unit(s(&["/m/a.mkv", "/m/unknown.srt"]));
  harness.update(cx, |store, cx| {
    store.set_all_rows_checked(&scope, true, cx);
    store.set_rows_checked(&scope, &s(&["1:subtitles:0"]), true, cx);
    assert!(store.selected_ids("/m/unknown.srt").is_none());
    let checked: HashSet<String> = scope.checked_rows(store);
    assert_eq!(checked.len(), 5);
  });
}
