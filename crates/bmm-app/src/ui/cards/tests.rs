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

//! Headless tests of the cards in the application shell, with the golden
//! fixtures of the merge-argument test parsed by the native parser.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use bmm_core::config::{Config, GroupMode};
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track::TrackType;
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::MediaTrackCounts;
use bmm_core::merge::{MergeEvent, MergeOutcome};
use core::prelude::v1::test;
use gpui_kit::component::WindowExt;
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;
use serde_json::Value;

use super::group_card::root_list_width;
use super::*;
use crate::domain::file_tree::FlagKind;
use crate::domain::status::QueueItem;
use crate::state::files::tests::TempDir;
use crate::ui::file_list::{CardView, DropTarget, drop_target};
use crate::ui::tests::{Shell, finished, started};
use crate::ui::track_table::tests::{golden_files, movie_shell, selected_ids, shell};

/// Select All in a text field: Cmd+A on macOS, Ctrl+A elsewhere.
const SELECT_ALL: &str = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };

fn s(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

fn config(group_mode: GroupMode, group_by_file_name: bool) -> Config {
  Config {
    group_mode,
    group_by_file_name,
    ..Config::default()
  }
}

/// Two golden movies, `a-movie.mkv` and `b-movie.mkv`, in one directory.
fn two_movies(name: &str) -> (TempDir, String, String, HashMap<String, MediaMetadata>) {
  let dir = TempDir::new(name);
  let (first, mut parsed) = golden_files(&dir, "a-");
  let (second, more) = golden_files(&dir, "b-");
  parsed.extend(more);
  (dir, first[0].clone(), second[0].clone(), parsed)
}

fn card(shell: &Shell, cx: &mut TestAppContext, id: &str) -> CardView {
  cx.read(|cx| {
    shell
      .view
      .read(cx)
      .main()
      .read(cx)
      .file_list()
      .read(cx)
      .card(id)
      .cloned()
      .unwrap_or_else(|| panic!("no card {id}"))
  })
}

fn units(shell: &Shell, cx: &mut TestAppContext) -> Vec<Vec<String>> {
  shell.stores.files.read_with(cx, |files, _| files.units().to_vec())
}

/// The texts painted inside the file-list entry `index` in a fresh frame.
fn texts(shell: &Shell, cx: &mut TestAppContext, index: usize) -> Vec<String> {
  let entry = ElementId::from(("file-entry", index));
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .filter(|text| text.path().contains(&entry))
      .map(|text| text.text().to_string())
      .collect()
  })
}

/// Whether `id` inside the file-list entry `index` is on screen and its
/// disabled state.
fn control(shell: &Shell, cx: &mut TestAppContext, index: usize, id: impl Into<ElementId>) -> Option<bool> {
  let id = id.into();
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window
      .within(("file-entry", index))
      .try_find(id)
      .map(|control| control.disabled() == Some(true))
  })
}

fn click_in(shell: &Shell, cx: &mut TestAppContext, index: usize, id: impl Into<ElementId>) {
  let id = id.into();
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.within(("file-entry", index)).click(id, cx);
  });
  cx.run_until_parked();
}

/// Drags the handle of the card at `from` onto the middle of the entry
/// at `to`.
fn drag_card(shell: &Shell, cx: &mut TestAppContext, from: usize, to: usize) {
  let (start, end) = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    let start = window.within(("file-entry", from)).find("card-drag").bounds().center();
    let end = window.find(("file-entry", to)).bounds().center();
    (start, end)
  });
  shell.window(cx, |window, cx| window.drag(start, end, cx));
  cx.run_until_parked();
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
  cx.read(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn progress(file: &str, percent: u32) -> MergeEvent {
  MergeEvent::Progress {
    file: file.to_string(),
    percent,
  }
}

#[gpui_kit::test]
fn the_file_list_scrollbar_overlays_full_width_cards(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-scrollbar");
  let shell = shell(cx, config(GroupMode::None, false), parsed, &[&a, &b]);
  cx.simulate_window_scale_factor_change(shell.handle, 1.5);
  let resize = |cx: &mut TestAppContext, height| {
    shell.resize(cx, 1200.0, height);
    shell.window(cx, |window, cx| {
      window.simulate_next_frame(cx);
      window.render_frame(cx);
      let viewport = window.find("file-list").bounds();
      let first = window.find(FileCard::element_id(&a)).bounds();
      (viewport, first)
    })
  };
  let (viewport, first) = resize(cx, 1000.0);
  assert_eq!(first.right(), viewport.right());
  let (viewport, first) = resize(cx, 450.0);
  assert_eq!(first.right(), viewport.right());
  shell.window(cx, |window, cx| {
    let width = gpui_kit::component::scroll::Scrollbar::width();
    let scale = window.scale_factor();
    let clip = viewport.scale(scale);
    let track = Bounds::new(
      point(viewport.right() - width, viewport.top()),
      size(width, viewport.size.height),
    )
    .scale(scale);
    let thumb_is_painted = |window: &Window| {
      window.painted_quads().iter().any(|quad| {
        quad.content_mask.bounds == clip
          && quad.bounds.left() >= track.left()
          && quad.bounds.right() <= track.right()
          && quad.bounds.size.width < track.size.width
          && quad.bounds.size.height > quad.bounds.size.width
          && quad.background.as_solid().is_some_and(|color| color.a > 0.0)
      })
    };
    assert!(!thumb_is_painted(window), "the scrollbar starts hidden");
    window.scroll("file-list", ScrollDelta::Lines(point(0.0, -200.0)), cx);
    window.simulate_next_frame(cx);
    window.render_frame(cx);
    let viewport = window.find("file-list").bounds();
    let last = window.find(FileCard::element_id(&b)).bounds();
    assert_eq!(last.right(), first.right());
    assert!(viewport.bottom() <= window.find("status-bar").bounds().top());
    let bottom_gap = viewport.bottom() - last.bottom();
    assert!(
      bottom_gap > px(0.0) && (bottom_gap - crate::ui::SHELL_MARGIN).abs() < px(1.0),
      "the last card ends inside the scroll viewport: {bottom_gap:?}, {last:?}, {viewport:?}"
    );
    let bounds = last.scale(scale);
    let quads = window.painted_quads();
    assert!(
      quads.iter().any(|quad| {
        (quad.bounds.bottom().0 - bounds.bottom().0).abs() <= 1.0
          && (quad.bounds.right().0 - bounds.right().0).abs() <= 1.0
          && quad.border_widths.bottom.0 > 0.0
          && quad.border_widths.right.0 > 0.0
          && quad.content_mask.bounds.bottom() >= quad.bounds.bottom()
          && quad.content_mask.bounds.right() >= quad.bounds.right()
          && quad.content_mask.bounds.top().0 <= quad.bounds.bottom().0 - quad.border_widths.bottom.0
      }),
      "the last card's bottom-right border is painted inside the clip"
    );
    assert!(
      thumb_is_painted(window),
      "scrolling reveals the thumb over the card edge"
    );
  });
  let (viewport, first) = resize(cx, 1000.0);
  assert_eq!(first.right(), viewport.right());
}

#[gpui_kit::test]
fn the_file_card_path_can_be_selected_and_copied_without_editing(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "cards-copy-path");
  shell.resize(cx, 1300.0, 800.0);
  let path = card(&shell, cx, &movie)
    .file()
    .unwrap()
    .read_with(cx, |card, _| card.path_state().clone());
  shell.window(cx, |window, cx| {
    let summary = window.find("card-summary").bounds();
    let text = path.read(cx).text_bounds().expect("path text was laid out");
    assert_eq!(text.left(), summary.left());
  });
  shell.click(cx, "card-path");
  shell.press(cx, SELECT_ALL);
  shell.window(cx, |window, cx| {
    assert!(path.read(cx).focus_handle(cx).is_focused(window));
    assert_eq!(path.read(cx).selected_range(), 0..movie.len());
  });
  let copy = if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" };
  shell.press(cx, copy);
  assert_eq!(clipboard(cx).as_deref(), Some(movie.as_str()));
  shell.window(cx, |window, cx| window.input("edited", cx));
  shell.press(cx, "backspace");
  assert_eq!(path.read_with(cx, |path, _| path.value()).as_ref(), movie);
}

/// Draws a frame without a refresh, as the app does: cached views that
/// were not notified are reused.
fn draw(shell: &Shell, cx: &mut TestAppContext) {
  shell.window(cx, |window, cx| window.draw(cx).clear(cx));
}

#[gpui_kit::test]
fn two_identical_files_form_one_group_card_in_each_group_mode(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-group-modes");
  let shell = shell(cx, config(GroupMode::TrackCount, false), parsed, &[&a, &b]);
  for mode in [GroupMode::TrackCount, GroupMode::TrackCountAndLanguage] {
    shell.stores.config.update(cx, |config, cx| {
      config.update(cx, |config| config.group_mode = mode.clone())
    });
    cx.run_until_parked();
    let group = card(&shell, cx, &a);
    let group = group.group().unwrap_or_else(|| panic!("{mode:?}: a group card"));
    assert_eq!(
      group.read_with(cx, |card, _| card.units().to_vec()),
      [s(&[&a]), s(&[&b])],
      "{mode:?}"
    );
    let shown = texts(&shell, cx, 0);
    for expected in [bmm_core::merge_args::parent_dir(&a), "a-movie.mkv", "b-movie.mkv"] {
      assert!(
        shown.iter().any(|text| text == expected),
        "{mode:?}: {expected} in {shown:?}"
      );
    }
    assert!(
      shown.iter().any(|text| text.starts_with("1 🎞️ 2 🔊 2 💬 1 📑 2 📎")),
      "{shown:?}"
    );
  }
  shell.stores.config.update(cx, |config, cx| {
    config.update(cx, |config| config.group_mode = GroupMode::None)
  });
  cx.run_until_parked();
  texts(&shell, cx, 0);
  assert!(card(&shell, cx, &a).file().is_some());
  assert!(card(&shell, cx, &b).file().is_some());
}

#[gpui_kit::test]
fn a_file_card_shows_its_path_summary_and_actions(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "cards-summary");
  let metadata = shell
    .stores
    .files
    .read_with(cx, |files, _| files.metadata(&movie).cloned().unwrap());
  let shown = texts(&shell, cx, 0);
  assert!(shown.contains(&movie), "{shown:?}");
  let size = crate::domain::format::format_file_size(metadata.file_size as f64);
  let summary = shown.iter().find(|text| text.contains("🎞️")).expect("a summary");
  assert!(summary.starts_with("1 🎞️ 2 🔊 2 💬 1 📑 2 📎 "), "{summary}");
  assert!(summary.contains(&format!("Size: {size}")), "{summary}");
  if let Some(title) = &metadata.container.properties.title {
    assert!(summary.contains(&format!("Title: {title}")), "{summary}");
  }
  // The profile selected tracks: Copy and Merge are available; no badge,
  // no BetterMediaInfo, no progress.
  for id in ["card-output", "card-copy", "card-merge", "card-delete"] {
    assert_eq!(control(&shell, cx, 0, id), Some(false), "{id}");
  }
  for id in ["card-children", "card-bmi", "card-progress"] {
    assert_eq!(control(&shell, cx, 0, id), None, "{id}");
  }
  // Without a selection Copy and Merge are off.
  shell
    .stores
    .files
    .update(cx, |files, cx| files.set_file_selected_ids(&movie, Vec::new(), cx));
  assert_eq!(control(&shell, cx, 0, "card-copy"), Some(true));
  assert_eq!(control(&shell, cx, 0, "card-merge"), Some(true));
}

#[test]
fn the_summary_joins_counts_output_and_details() {
  let _locale = crate::i18n::tests::LOCALE_LOCK
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  crate::i18n::set_language(bmm_core::config::Language::EnUS);
  let counts = MediaTrackCounts {
    video: 1,
    audio: 2,
    ..Default::default()
  };
  assert_eq!(card_summary(None, None, None, None, None), None);
  assert_eq!(
    card_summary(Some(&MediaTrackCounts::default()), None, None, None, None),
    None
  );
  assert_eq!(
    card_summary(
      Some(&counts),
      Some("/out/x.mkv"),
      Some("1.2GB"),
      Some("2026-05-24 09:00:00"),
      Some("Movie")
    )
    .as_deref(),
    Some("1 🎞️ 2 🔊 To path: /out/x.mkv Size: 1.2GB, Encoded: 2026-05-24 09:00:00, Title: Movie")
  );
  assert_eq!(
    card_summary(None, None, Some("2KB"), None, None).as_deref(),
    Some("Size: 2KB")
  );
}

#[test]
fn the_time_label_shows_elapsed_and_eta_while_merging() {
  let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
  let mut item = QueueItem::waiting("/m/a.mkv");
  assert_eq!(time_label(None, start), "--:--:-- / --:--:--");
  assert_eq!(time_label(Some(&item), start), "--:--:-- / --:--:--");
  item.status = crate::domain::status::QueueItemStatus::Merging;
  item.merge_started_at = Some(start);
  item.progress = 25;
  assert_eq!(
    time_label(Some(&item), start + Duration::from_secs(90)),
    "00:01:30 / 00:04:30"
  );
}

#[gpui_kit::test]
fn merging_from_a_card_shows_progress_and_cancel(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "cards-merge");
  click_in(&shell, cx, 0, "card-merge");
  assert_eq!(shell.merges.lock().unwrap().enqueued.len(), 1);
  assert_eq!(shell.merges.lock().unwrap().enqueued[0].0, movie);
  // Waiting: no edits, no merge, no drag; Delete and Cancel stay.
  for (id, disabled) in [
    ("card-output", true),
    ("card-copy", true),
    ("card-merge", true),
    ("card-delete", false),
    ("card-cancel", false),
  ] {
    assert_eq!(control(&shell, cx, 0, id), Some(disabled), "waiting: {id}");
  }
  assert!(
    !texts(&shell, cx, 0).iter().any(|text| text.contains(" / ")),
    "no times while waiting"
  );
  shell.send(cx, started(&movie));
  shell.send(cx, progress(&movie, 40));
  assert_eq!(control(&shell, cx, 0, "card-delete"), Some(true), "merging");
  let shown = texts(&shell, cx, 0);
  assert!(
    shown.iter().any(|text| text.len() == 19 && text.contains(" / ")),
    "elapsed / ETA in {shown:?}"
  );
  click_in(&shell, cx, 0, "card-cancel");
  assert_eq!(shell.merges.lock().unwrap().cancelled, std::slice::from_ref(&movie));
  shell.send(cx, finished(&movie, MergeOutcome::Cancelled, None));
  assert_eq!(control(&shell, cx, 0, "card-progress"), None);
  assert_eq!(control(&shell, cx, 0, "card-merge"), Some(false));
}

#[gpui_kit::test]
fn delete_removes_the_card_and_cancels_a_waiting_merge(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "cards-delete");
  click_in(&shell, cx, 0, "card-merge");
  click_in(&shell, cx, 0, "card-delete");
  assert_eq!(shell.merges.lock().unwrap().cancelled, std::slice::from_ref(&movie));
  assert!(shell.stores.files.read_with(cx, |files, _| !files.has_files()));
  assert!(
    shell
      .stores
      .queue
      .read_with(cx, |queue, _| queue.item(&movie).is_none())
  );
}

/// `Movie.srt` added while `Movie.en.srt` merges becomes the root of a card
/// that holds the merging file: Delete waits for that merge, which would
/// otherwise run on untracked.
#[gpui_kit::test]
fn delete_waits_for_a_member_that_merges(cx: &mut TestAppContext) {
  let dir = TempDir::new("cards-member-merges");
  let (subtitle, base) = (dir.file("Movie.en.srt"), dir.file("Movie.srt"));
  let english = || crate::state::files::tests::metadata(&[(TrackType::Subtitles, "eng")]);
  let parsed = HashMap::from([(subtitle.clone(), english()), (base.clone(), english())]);
  let shell = shell(cx, config(GroupMode::None, true), parsed, &[subtitle.as_str()]);
  click_in(&shell, cx, 0, "card-merge");
  shell.send(cx, started(&subtitle));
  shell.add_parsed(cx, &[base.as_str()]);
  assert_eq!(units(&shell, cx), [s(&[&base, &subtitle])]);
  assert_eq!(control(&shell, cx, 0, "card-delete"), Some(true), "a member merges");
  click_in(&shell, cx, 0, "card-delete");
  assert_eq!(units(&shell, cx).len(), 1);
  assert!(cx.update(|cx| shell.stores.has_active_merges(cx)));
  shell.send(cx, finished(&subtitle, MergeOutcome::Completed, None));
  assert_eq!(control(&shell, cx, 0, "card-delete"), Some(false));
  click_in(&shell, cx, 0, "card-delete");
  assert!(shell.stores.files.read_with(cx, |files, _| !files.has_files()));
}

#[gpui_kit::test]
fn only_the_merging_card_repaints(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-repaint");
  let shell = shell(cx, config(GroupMode::None, false), parsed, &[&a, &b]);
  let (card_a, card_b) = (card(&shell, cx, &a), card(&shell, cx, &b));
  draw(&shell, cx);
  draw(&shell, cx);
  let counts = |cx: &mut TestAppContext| cx.read(|cx| (card_a.render_count(cx), card_b.render_count(cx)));
  let before = counts(cx);
  draw(&shell, cx);
  assert_eq!(counts(cx), before, "an idle frame reuses both cards");
  // The lower card merges, so its progress row moves nothing.
  for event in [started(&b), progress(&b, 10), progress(&b, 20)] {
    let (a_before, b_before) = counts(cx);
    shell.send(cx, event);
    draw(&shell, cx);
    draw(&shell, cx);
    let (a_after, b_after) = counts(cx);
    assert!(b_after > b_before, "the merging card repaints");
    assert_eq!(a_after, a_before, "the other card does not");
  }
}

/// A view that counts its renders, drawn by [`MeasuredHost`].
struct Counted(Rc<Cell<usize>>);

impl Render for Counted {
  fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
    self.0.set(self.0.get() + 1);
    div().h(px(20.0))
  }
}

struct MeasuredHost {
  child: Entity<Counted>,
  height: CardHeight,
  cache: bool,
}

impl Render for MeasuredHost {
  fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
    div().size_full().child(self.height.view(&self.child, self.cache))
  }
}

#[gpui_kit::test]
fn cards_are_drawn_uncached_for_assistive_technology(cx: &mut TestAppContext) {
  // GPUI does not replay a cached view's accessibility nodes, so the list
  // draws its cards uncached while a screen reader is connected.
  for cache in [true, false] {
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, cx| MeasuredHost {
      child: cx.new(|_| Counted(renders.clone())),
      height: CardHeight(Rc::new(Cell::new(Some(px(20.0))))),
      cache,
    });
    // The host renders again (notified), the child only when not cached.
    let draw = |cx: &mut TestAppContext| {
      window.update(cx, |_, _, cx| cx.notify()).unwrap();
      cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap()
    };
    draw(cx);
    let before = renders.get();
    draw(cx);
    draw(cx);
    let renders = renders.get() - before;
    if cache {
      assert_eq!(renders, 0, "a cached view is reused");
    } else {
      assert!(renders >= 2, "an uncached view renders every frame: {renders}");
    }
  }
}

#[gpui_kit::test]
fn drag_merging_a_card_and_detaching_its_child(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-drag");
  let shell = shell(cx, config(GroupMode::None, false), parsed, &[&a, &b]);
  drag_card(&shell, cx, 0, 1);
  assert_eq!(units(&shell, cx), [s(&[&b, &a])], "a merged into b");
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.drop_target_root().map(str::to_string)),
    None
  );
  // One card with a child badge of 1, whose list removes the child.
  assert_eq!(control(&shell, cx, 0, "card-children"), Some(false));
  let file_card = card(&shell, cx, &b);
  click_in(&shell, cx, 0, "card-children");
  assert!(
    file_card
      .file()
      .unwrap()
      .read_with(cx, |card, _| card.is_children_open())
  );
  let shown = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .map(|text| text.text().to_string())
      .collect::<Vec<_>>()
  });
  assert!(shown.contains(&"1".to_string()) && shown.contains(&a), "{shown:?}");
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.click(("child-remove", 0usize), cx);
  });
  cx.run_until_parked();
  assert_eq!(units(&shell, cx), [s(&[&a]), s(&[&b])], "detached again");
  assert!(
    !file_card
      .file()
      .unwrap()
      .read_with(cx, |card, _| card.is_children_open())
  );
}

#[gpui_kit::test]
fn a_group_card_refuses_a_card_drop(cx: &mut TestAppContext) {
  let dir = TempDir::new("cards-refuse");
  let (first, mut parsed) = golden_files(&dir, "a-");
  let (second, more) = golden_files(&dir, "b-");
  parsed.extend(more);
  // Two movies form a group; the audio-only file stays a card of its own.
  let shell = shell(
    cx,
    config(GroupMode::TrackCount, false),
    parsed,
    &[&first[0], &second[0], &first[1]],
  );
  assert!(card(&shell, cx, &first[0]).group().is_some());
  let before = units(&shell, cx);
  drag_card(&shell, cx, 1, 0);
  assert_eq!(units(&shell, cx), before);
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.drop_target_root().map(str::to_string)),
    None
  );
}

#[test]
fn a_drag_targets_only_other_file_cards() {
  use crate::state::files::TableScope;
  let scopes = [
    TableScope::unit(s(&["/m/a.mkv"])),
    TableScope::group(vec![s(&["/m/b.mkv"]), s(&["/m/c.mkv"])]),
    TableScope::unit(s(&["/m/d.mkv"])),
  ];
  let bounds = [0.0, 100.0, 200.0].map(|y| Bounds::new(point(px(0.0), px(y)), size(px(500.0), px(90.0))));
  let at = |y: f32| point(px(10.0), px(y));
  assert_eq!(
    drop_target("/m/a.mkv", at(250.0), &scopes, &bounds),
    DropTarget::Card("/m/d.mkv".into())
  );
  assert_eq!(
    drop_target("/m/a.mkv", at(150.0), &scopes, &bounds),
    DropTarget::Refused
  );
  assert_eq!(drop_target("/m/a.mkv", at(50.0), &scopes, &bounds), DropTarget::Refused);
  assert_eq!(drop_target("/m/a.mkv", at(95.0), &scopes, &bounds), DropTarget::None);
}

/// A golden case as the stores would hold it: each input listed at its
/// file, selected and edited as the case says, the output path set on the
/// root and the case's MKVToolNix directory.
fn golden_shell(cx: &mut TestAppContext, case: &Value, name: &str) -> (Shell, TempDir) {
  let dir = TempDir::new(name);
  let (paths, parsed) = golden_files(&dir, "");
  let by_fixture: HashMap<&str, MediaMetadata> = ["movie", "movie.ja", "movie.en"]
    .into_iter()
    .zip(paths.iter().map(|path| parsed[path].clone()))
    .collect();
  let inputs = case["inputs"].as_array().unwrap();
  let files: Vec<String> = inputs
    .iter()
    .map(|input| input["file"].as_str().unwrap().to_string())
    .collect();
  let fixtures = inputs
    .iter()
    .map(|input| {
      (
        input["file"].as_str().unwrap().to_string(),
        by_fixture[input["fixture"].as_str().unwrap()].clone(),
      )
    })
    .collect();
  let mut config = config(GroupMode::None, true);
  config.external_tools.mkv_toolnix_path = case["mkvToolNixPath"].as_str().unwrap().to_string();
  let listed: Vec<&str> = files.iter().map(String::as_str).collect();
  let shell = shell(cx, config, fixtures, &listed);
  let unit = units(&shell, cx);
  assert_eq!(unit.len(), 1, "one unit");
  let (mut members, mut listed_files) = (unit[0].clone(), files.clone());
  assert_eq!(members[0], files[0], "the movie is the root");
  members.sort();
  listed_files.sort();
  assert_eq!(members, listed_files);
  shell.stores.files.update(cx, |store, cx| {
    for input in inputs {
      let file = input["file"].as_str().unwrap().to_string();
      let one = [file.clone()];
      let selected = input["selected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap().to_string())
        .collect();
      store.set_file_selected_ids(&file, selected, cx);
      for (key, edit) in input.get("edits").and_then(Value::as_object).into_iter().flatten() {
        let keys = [key.clone()];
        for (field, value) in edit.as_object().unwrap() {
          let value = value.as_str().unwrap();
          let flag = || match value {
            "true" => TrackFlag::True,
            "false" => TrackFlag::False,
            _ => TrackFlag::Unspecified,
          };
          match field.as_str() {
            "language" => store.set_track_language(&one, &keys, value, cx),
            "trackName" => store.set_track_name(&one, &keys, value, cx),
            "defaultTrack" => store.set_track_flag(&one, &keys, FlagKind::Default, flag(), cx),
            "forced" => store.set_track_flag(&one, &keys, FlagKind::Forced, flag(), cx),
            other => panic!("edit {other}"),
          }
        }
      }
    }
    store.set_file_output_path(&files[0], case["outputPath"].as_str().unwrap().to_string(), cx);
  });
  (shell, dir)
}

fn golden_case(json: &str) -> Value {
  serde_json::from_str(json).unwrap()
}

#[gpui_kit::test]
fn copy_command_copies_the_golden_single_file_command(cx: &mut TestAppContext) {
  let case = golden_case(include_str!(
    "../../../../bmm-core/tests/golden/single_edited_reordered.json"
  ));
  let (shell, _dir) = golden_shell(cx, &case, "cards-golden-single");
  let file = case["inputs"][0]["file"].as_str().unwrap().to_string();
  // The case's dragged order: audio 2 first, subtitles 4 before audio 1.
  shell.stores.files.update(cx, |store, cx| {
    let one = [file.clone()];
    store.reorder_tracks(&one, "audio:2", "video:0", cx);
    store.reorder_tracks(&one, "subtitles:4", "audio:1", cx);
  });
  assert_eq!(selected_ids(&shell, cx, &file).len(), 6);
  click_in(&shell, cx, 0, "card-copy");
  let copied = clipboard(cx).expect("a copied command");
  if std::path::MAIN_SEPARATOR == '/' {
    assert_eq!(copied, case["expected"]["command"].as_str().unwrap());
  }
  assert_eq!(shell.toasts(cx), 1, "Command copied");
}

#[gpui_kit::test]
fn copy_command_copies_the_golden_merge_tree_command(cx: &mut TestAppContext) {
  let case = golden_case(include_str!(
    "../../../../bmm-core/tests/golden/multi_default_order.json"
  ));
  let (shell, _dir) = golden_shell(cx, &case, "cards-golden-multi");
  click_in(&shell, cx, 0, "card-copy");
  let copied = clipboard(cx).expect("a copied command");
  // The unit lists its members by name, so the inputs come in that order;
  // the card's combined order is the golden default type-major order.
  let unit = units(&shell, cx).remove(0);
  let inputs: Vec<bmm_core::merge_args::MergeInput> = shell.stores.files.read_with(cx, |files, _| {
    unit
      .iter()
      .map(|file| bmm_core::merge_args::MergeInput {
        file: file.clone(),
        tracks: files.selected_tracks(file),
        sub_charset: files.merge_sub_charset(file),
      })
      .filter(|input| !input.tracks.is_empty())
      .collect()
  });
  let expected = bmm_core::merge_args::build_command_string_multi(
    &inputs,
    case["outputPath"].as_str().unwrap(),
    case["mkvToolNixPath"].as_str().unwrap(),
    None,
  );
  assert_eq!(copied, expected);
  if std::path::MAIN_SEPARATOR == '/' {
    // The golden selection and edits; a member with nothing selected is left
    // out.
    for part in [
      "/opt/mkvtoolnix/mkvmerge -o /out/Movie.mkv -d 0 -a 2 -s 3 --language 0:en",
      "--track-name '0:Director Commentary'",
    ] {
      assert!(copied.contains(part), "{part} in {copied}");
    }
    assert!(!copied.contains("Movie.forced.srt"), "{copied}");
    assert!(copied.ends_with("--track-order 0:0,0:2,2:0,0:3,1:0"), "{copied}");
  }
}

#[gpui_kit::test]
fn a_group_card_merges_copies_and_clears_every_unit(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-group-actions");
  let shell = shell(cx, config(GroupMode::TrackCount, false), parsed, &[&a, &b]);
  shell.resize(cx, 1400.0, 760.0);
  shell.stores.config.update(cx, |config, cx| {
    config.update(cx, |config| {
      config.external_tools.mkv_toolnix_path = "/opt/mkvtoolnix".into()
    })
  });
  click_in(&shell, cx, 0, "group-copy");
  let copied = clipboard(cx).expect("copied commands");
  let lines: Vec<&str> = copied.lines().collect();
  assert_eq!(lines.len(), 2, "{copied}");
  assert!(
    lines[0].contains(a.as_str()) && lines[1].contains(b.as_str()),
    "{copied}"
  );
  click_in(&shell, cx, 0, "group-merge");
  let enqueued: Vec<String> = shell
    .merges
    .lock()
    .unwrap()
    .enqueued
    .iter()
    .map(|(root, _)| root.clone())
    .collect();
  assert_eq!(enqueued, [a.clone(), b.clone()]);
  for id in ["group-merge", "group-output", "group-clear"] {
    assert_eq!(control(&shell, cx, 0, id), Some(true), "{id} while active");
  }
  assert_eq!(control(&shell, cx, 0, "group-copy"), Some(false));
  shell.send(cx, started(&a));
  shell.send(cx, progress(&a, 30));
  let shown = texts(&shell, cx, 0);
  assert!(shown.contains(&"30%".to_string()), "{shown:?}");
  assert_eq!(control(&shell, cx, 0, ("group-cancel", 0usize)), Some(false));
  for (file, outcome) in [(&a, MergeOutcome::Completed), (&b, MergeOutcome::Cancelled)] {
    shell.send(cx, finished(file, outcome, None));
  }
  click_in(&shell, cx, 0, "group-clear");
  assert!(shell.stores.files.read_with(cx, |files, _| !files.has_files()));
}

#[test]
fn the_root_list_keeps_its_limits() {
  let (left, width) = (px(100.0), px(1000.0));
  assert_eq!(root_list_width(px(400.0), left, width), px(300.0));
  assert_eq!(root_list_width(px(150.0), left, width), px(160.0));
  assert_eq!(root_list_width(px(1000.0), left, width), px(760.0));
  // A narrow pane keeps the minimum.
  assert_eq!(root_list_width(px(400.0), left, px(300.0)), px(160.0));
}

#[gpui_kit::test]
fn the_group_splitter_resizes_the_root_list(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-splitter");
  let shell = shell(cx, config(GroupMode::TrackCount, false), parsed, &[&a, &b]);
  let group = card(&shell, cx, &a).group().unwrap().clone();
  let start = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window
      .within(("file-entry", 0usize))
      .find("group-splitter")
      .bounds()
      .center()
  });
  shell.window(cx, |window, cx| {
    window.drag(start, start + point(px(100.0), px(0.0)), cx)
  });
  cx.run_until_parked();
  let width = group.read_with(cx, |card, _| card.root_list_width());
  assert!((width - px(340.0)).abs() < px(4.0), "{width:?}");
  let start = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window
      .within(("file-entry", 0usize))
      .find("group-splitter")
      .bounds()
      .center()
  });
  shell.window(cx, |window, cx| window.drag(start, point(px(0.0), start.y), cx));
  cx.run_until_parked();
  assert_eq!(group.read_with(cx, |card, _| card.root_list_width()), px(160.0));
}

/// The dialog's field, after a fresh frame.
fn dialog_value(shell: &Shell, cx: &mut TestAppContext) -> Option<String> {
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window
      .try_find("output-path")
      .and_then(|input| input.value().map(str::to_string))
  })
}

fn has_dialog(shell: &Shell, cx: &mut TestAppContext) -> bool {
  shell.window(cx, |window, cx| window.has_active_dialog(cx))
}

/// Replaces the focused dialog field's text and presses Enter.
fn type_and_confirm(shell: &Shell, cx: &mut TestAppContext, text: &str) {
  shell.window(cx, |window, cx| {
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
    if !text.is_empty() {
      window.input(text, cx);
    }
  });
  cx.run_until_parked();
  shell.press(cx, "enter");
  cx.run_until_parked();
}

#[gpui_kit::test]
fn word_keys_stop_inside_numbers_in_a_dialog_field(cx: &mut TestAppContext) {
  let (shell, _, _dir) = movie_shell(cx, "cards-word-keys");
  click_in(&shell, cx, 0, "card-output");
  assert!(has_dialog(&shell, cx));
  shell.window(cx, |window, cx| {
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
    window.input("Movie.2024.1080p", cx);
  });
  cx.run_until_parked();
  let word_left = if cfg!(target_os = "macos") {
    "alt-left"
  } else {
    "ctrl-left"
  };
  let caret = |shell: &Shell, cx: &mut TestAppContext| {
    shell.window(cx, |window, cx| match window.focused_input(cx) {
      Some(gpui_kit::component::input::AnyInputState::Input(state)) => state.read(cx).cursor(),
      _ => panic!("the dialog field has the focus"),
    })
  };
  shell.press(cx, word_left);
  assert_eq!(caret(&shell, cx), 11);
  shell.press(cx, word_left);
  assert_eq!(caret(&shell, cx), 6);
}

#[gpui_kit::test]
fn the_output_dialog_sets_and_clears_a_cards_output_path(cx: &mut TestAppContext) {
  let (shell, movie, dir) = movie_shell(cx, "cards-output");
  click_in(&shell, cx, 0, "card-output");
  assert!(has_dialog(&shell, cx));
  // The path Merge would write: the input's directory and stem, " (1)"
  // because the input itself is an `.mkv` there.
  let expected = dir.path().join("movie (1).mkv").to_string_lossy().into_owned();
  assert_eq!(dialog_value(&shell, cx).as_deref(), Some(expected.as_str()));
  let chosen = dir.path().join("out").join("Chosen.mkv").to_string_lossy().into_owned();
  type_and_confirm(&shell, cx, &chosen);
  assert!(!has_dialog(&shell, cx));
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.output_path(&movie).map(str::to_string)),
    Some(chosen.clone())
  );
  assert!(
    texts(&shell, cx, 0)
      .iter()
      .any(|text| text.contains(&format!("To path: {chosen}")))
  );
  // Reopened, it starts from the override; empty clears it.
  click_in(&shell, cx, 0, "card-output");
  assert_eq!(dialog_value(&shell, cx).as_deref(), Some(chosen.as_str()));
  type_and_confirm(&shell, cx, "");
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.output_path(&movie).map(str::to_string)),
    None
  );
}

#[gpui_kit::test]
fn the_output_dialog_warns_about_missing_and_refuses_unwritable_paths(cx: &mut TestAppContext) {
  let (shell, movie, dir) = movie_shell(cx, "cards-output-checks");
  click_in(&shell, cx, 0, "card-output");
  let tip = tr!("merge.outputPathHint");
  let warning = tr!("merge.outputPathDoesNotExist");
  let error = tr!("merge.outputPathNotWritable");
  let messages = [tip.as_ref(), warning.as_ref(), error.as_ref()];
  let snapshot = |cx: &mut TestAppContext| {
    shell.window(cx, |window, cx| {
      window.render_frame(cx);
      let bounds = window.find("surface").bounds();
      let shown: Vec<_> = gpui_kit::base::test_support::texts(window)
        .into_iter()
        .filter_map(|text| messages.iter().copied().find(|message| *message == text.text()))
        .collect();
      (bounds, shown)
    })
  };
  let (bounds, shown) = snapshot(cx);
  assert_eq!(shown, [tip.as_ref()]);

  let missing = dir.path().join("new").join("Movie.mkv").to_string_lossy().into_owned();
  shell.window(cx, |window, cx| {
    window.press(SELECT_ALL, cx);
    window.input(&missing, cx);
  });
  cx.executor().advance_clock(Duration::from_millis(400));
  cx.run_until_parked();
  assert_eq!(snapshot(cx), (bounds, vec![warning.as_ref()]));
  // Below a file nothing can be written: the dialog stays with an error.
  let blocked = format!("{}/x.mkv", movie);
  type_and_confirm(&shell, cx, &blocked);
  assert!(has_dialog(&shell, cx));
  assert_eq!(snapshot(cx), (bounds, vec![error.as_ref()]));
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.output_path(&movie).map(str::to_string)),
    None
  );
  shell.window(cx, |window, cx| {
    window.press(SELECT_ALL, cx);
    window.press("backspace", cx);
  });
  cx.run_until_parked();
  assert_eq!(snapshot(cx), (bounds, vec![tip.as_ref()]));
  shell.press(cx, "escape");
  assert!(!has_dialog(&shell, cx), "Escape cancels");
}

/// A merge replaces no file, so the dialog refuses an existing one with the
/// message the merge would give, and keeps the dialog open.
#[gpui_kit::test]
fn the_output_dialog_refuses_an_existing_file(cx: &mut TestAppContext) {
  let (shell, movie, dir) = movie_shell(cx, "cards-output-exists");
  let existing = dir.path().join("Taken.mkv");
  std::fs::write(&existing, b"another program's file").unwrap();
  let existing = existing.to_string_lossy().into_owned();
  click_in(&shell, cx, 0, "card-output");
  type_and_confirm(&shell, cx, &existing);
  assert!(has_dialog(&shell, cx), "refused");
  let message = crate::actions::output_exists_message(&existing);
  let shown = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .any(|text| text.text() == message)
  });
  assert!(shown, "{message}");
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.output_path(&movie).map(str::to_string)),
    None
  );
  // A directory in file mode is fine: the merge writes the input's name in it.
  let directory = dir.path().to_string_lossy().into_owned();
  type_and_confirm(&shell, cx, &directory);
  assert!(!has_dialog(&shell, cx));
}

/// A folder dialog takes no file: a merge would treat it as the folder.
#[gpui_kit::test]
fn a_group_cards_output_directory_is_no_file(cx: &mut TestAppContext) {
  let (_dir, a, b, parsed) = two_movies("cards-group-output-file");
  let shell = shell(cx, config(GroupMode::TrackCount, false), parsed, &[&a, &b]);
  shell.resize(cx, 1400.0, 760.0);
  click_in(&shell, cx, 0, "group-output");
  type_and_confirm(&shell, cx, &a);
  assert!(has_dialog(&shell, cx), "refused");
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.output_dir(&a).map(str::to_string)),
    None
  );
}

#[gpui_kit::test]
fn a_group_cards_output_directory_goes_to_every_root(cx: &mut TestAppContext) {
  let (dir, a, b, parsed) = two_movies("cards-group-output");
  let shell = shell(cx, config(GroupMode::TrackCount, false), parsed, &[&a, &b]);
  shell.resize(cx, 1400.0, 760.0);
  click_in(&shell, cx, 0, "group-output");
  assert_eq!(
    dialog_value(&shell, cx).as_deref(),
    Some(bmm_core::merge_args::parent_dir(&a)),
    "starts at the directory"
  );
  let out = dir.path().join("out").to_string_lossy().into_owned();
  type_and_confirm(&shell, cx, &out);
  for root in [&a, &b] {
    assert_eq!(
      shell
        .stores
        .files
        .read_with(cx, |files, _| files.output_dir(root).map(str::to_string)),
      Some(out.clone())
    );
  }
  assert!(
    texts(&shell, cx, 0)
      .iter()
      .any(|text| text.ends_with(&format!("To path: {out}")))
  );
}

#[gpui_kit::test]
fn the_toolbar_sets_the_global_output_directory(cx: &mut TestAppContext) {
  let (shell, _movie, dir) = movie_shell(cx, "cards-global-output");
  shell.click(cx, "toolbar-global-output");
  shell.window(cx, |window, _| {
    let cancel = window.find("output-cancel").bounds();
    let ok = window.find("output-ok").bounds();
    assert_eq!(cancel.size.width, ok.size.width);
  });
  assert_eq!(dialog_value(&shell, cx).as_deref(), Some(""));
  let out = dir.path().to_string_lossy().into_owned();
  type_and_confirm(&shell, cx, &out);
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.global_output_dir().map(str::to_string)),
    Some(out)
  );
}

#[gpui_kit::test]
fn better_media_info_is_detected_and_a_failed_launch_is_shown(cx: &mut TestAppContext) {
  let (shell, _movie, dir) = movie_shell(cx, "cards-bmi");
  assert_eq!(control(&shell, cx, 0, "card-bmi"), None);
  // A directory holding the executable is found from the config.
  let tool = TempDir::new("cards-bmi-tool");
  let exe = if cfg!(windows) {
    "BetterMediaInfo.exe"
  } else {
    "BetterMediaInfo"
  };
  tool.file(exe);
  let tool_dir = tool.path().to_string_lossy().into_owned();
  shell.stores.config.update(cx, |config, cx| {
    config.update(cx, |config| {
      config.external_tools.better_media_info_path = tool_dir.clone()
    })
  });
  cx.update(|cx| crate::actions::watch_better_media_info(&shell.stores, cx));
  cx.run_until_parked();
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.better_media_info_dir().map(str::to_string)),
    Some(tool_dir)
  );
  assert_eq!(control(&shell, cx, 0, "card-bmi"), Some(false));
  // A launch from a directory without it fails with a message.
  let gone = dir.path().join("gone").to_string_lossy().into_owned();
  shell
    .stores
    .files
    .update(cx, |files, cx| files.set_better_media_info(Some(gone), cx));
  let toasts = shell.toasts(cx);
  click_in(&shell, cx, 0, "card-bmi");
  assert_eq!(shell.toasts(cx), toasts + 1);
}
