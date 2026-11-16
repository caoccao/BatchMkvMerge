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

//! Headless tests of the track table in the application shell, with the
//! golden fixtures of the merge-argument test parsed by the native parser.

use std::collections::{HashMap, HashSet};

use bmm_core::config::{
  Config, ConfigFormatField, ConfigFormatting, ConfigStreamFormat, FormatPrecision, FormatUnit, GroupMode,
};
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::{ParseOptions, parse};
use core::prelude::v1::test;
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;

use super::autocomplete::{CellKind, Placement};
use super::*;
use crate::state::files::tests::TempDir;
use crate::ui::tests::{Shell, started};

const SELECT_ALL: &str = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };

// The synthetic files of `crates/bmm-core/tests/merge_args_golden.rs`: a
// Matroska movie with every row type, an audio-only `.mka` and an `.srt`.
#[allow(dead_code)]
#[path = "../../../../bmm-core/tests/golden/fixtures.rs"]
pub(crate) mod golden;

/// The golden fixtures written under `dir` as `{prefix}{name}.{ext}` and
/// parsed: the paths in fixture order (movie, `.mka`, `.srt`) and the
/// metadata by path.
pub(crate) fn golden_files(dir: &TempDir, prefix: &str) -> (Vec<String>, HashMap<String, MediaMetadata>) {
  let mut paths = Vec::new();
  let mut parsed = HashMap::new();
  for fixture in golden::fixtures() {
    let path = dir.write(
      &format!("{prefix}{}.{}", fixture.name, fixture.extension),
      &fixture.bytes,
    );
    let metadata = parse(&path, ParseOptions::default()).expect("a golden fixture parses");
    parsed.insert(path.clone(), metadata);
    paths.push(path);
  }
  (paths, parsed)
}

fn english(config: Config) -> Config {
  Config {
    language: bmm_core::config::Language::EnUS,
    ..config
  }
}

/// A shell listing `files` (parsed from `fixtures`).
pub(crate) fn shell(
  cx: &mut TestAppContext,
  config: Config,
  fixtures: HashMap<String, MediaMetadata>,
  files: &[&str],
) -> Shell {
  let shell = Shell::with_config(cx, english(config), fixtures);
  shell.add_parsed(cx, files);
  // GPUI delivers focus events only to an active window.
  shell.window(cx, |window, _| window.activate_window());
  cx.run_until_parked();
  shell
}

/// A shell with the golden movie alone on its card.
pub(crate) fn movie_shell(cx: &mut TestAppContext, name: &str) -> (Shell, String, TempDir) {
  let dir = TempDir::new(name);
  let (paths, parsed) = golden_files(&dir, "");
  let movie = paths[0].clone();
  let shell = shell(cx, Config::default(), parsed, &[movie.as_str()]);
  (shell, movie, dir)
}

#[gpui_kit::test]
fn columns_fit_their_values_and_track_name_uses_the_remaining_width(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-widths");
  let widths =
    |cx: &mut TestAppContext| {
      shell.window(cx, |window, cx| {
        window.render_frame(cx);
        let language = window
          .find("language-0:audio:1-input")
          .bounds()
          .scale(window.scale_factor());
        assert!(window.painted_quads().iter().any(|quad| {
          quad.bounds == language && quad.border_widths.top.0 > 0.0 && quad.border_widths.right.0 > 0.0
        }));
        std::array::from_fn::<_, { Column::Forced as usize + 1 }, _>(|index| {
          let heading = window.within("track-head").find(("head", index + 1)).bounds();
          let cell = window.within(("track-row", 0usize)).find(("cell", index + 1)).bounds();
          assert_eq!(heading.origin.x, cell.origin.x);
          assert_eq!(heading.size.width, cell.size.width);
          heading.size.width
        })
      })
    };
  shell.resize(cx, 1100.0, 800.0);
  let narrow = widths(cx);
  shell.resize(cx, 1340.0, 800.0);
  let wide = widths(cx);
  for (index, (narrow, wide)) in narrow.into_iter().zip(wide).enumerate() {
    assert_eq!(
      wide - narrow,
      if index == Column::Name as usize {
        px(240.0)
      } else {
        px(0.0)
      }
    );
  }

  let scope = table(&shell, cx, &movie).read_with(cx, |table, _| table.scope().clone());
  shell.stores.files.update(cx, |files, cx| {
    files.set_rows_language(
      &scope,
      &["0:audio:1".to_owned()],
      "A custom language tag longer than the column heading",
      cx,
    );
  });
  cx.run_until_parked();
  let edited = widths(cx);
  let growth = edited[Column::Language as usize] - wide[Column::Language as usize];
  assert!(growth > px(0.0));
  assert_eq!(wide[Column::Name as usize] - edited[Column::Name as usize], growth);
}

pub(crate) fn table(shell: &Shell, cx: &mut TestAppContext, id: &str) -> Entity<TrackTable> {
  cx.read(|cx| {
    shell
      .view
      .read(cx)
      .main()
      .read(cx)
      .file_list()
      .read(cx)
      .table(id)
      .expect("the card's table")
  })
}

/// Runs `f` on the window with queries scoped to the table of card `id`,
/// after a fresh frame, then lets the effects run.
fn in_table<R>(
  shell: &Shell,
  cx: &mut TestAppContext,
  id: &str,
  f: impl FnOnce(&mut gpui_kit::test::ScopedWindow<'_>, &mut App) -> R,
) -> R {
  let scope = TrackTable::element_id(&TableScope::unit(vec![id.to_string()]));
  let result = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    let mut scoped = window.within(scope);
    f(&mut scoped, cx)
  });
  cx.run_until_parked();
  result
}

/// A click on row `index`'s ID cell, away from its controls.
fn click_row(shell: &Shell, cx: &mut TestAppContext, id: &str, index: usize) {
  in_table(shell, cx, id, |window, cx| {
    window.click_at(("track-row", index), point(px(60.0), px(12.0)), cx)
  });
}

fn row_keys(shell: &Shell, cx: &mut TestAppContext, id: &str) -> Vec<String> {
  let table = table(shell, cx, id);
  cx.read(|cx| {
    let scope = table.read(cx).scope().clone();
    scope
      .rows(shell.stores.files.read(cx))
      .iter()
      .map(|row| row.row_key())
      .collect()
  })
}

pub(crate) fn selected_ids(shell: &Shell, cx: &mut TestAppContext, file: &str) -> Vec<String> {
  shell.stores.files.read_with(cx, |files, _| {
    files.selected_ids(file).map(<[String]>::to_vec).unwrap_or_default()
  })
}

fn track<R>(
  shell: &Shell,
  cx: &mut TestAppContext,
  file: &str,
  key: &str,
  f: impl FnOnce(&bmm_core::media_track::MediaTrack) -> R,
) -> R {
  shell.stores.files.read_with(cx, |files, _| {
    f(files
      .tracks(file)
      .unwrap()
      .iter()
      .find(|track| track.key() == key)
      .unwrap())
  })
}

fn s(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

fn set(values: &[&str]) -> HashSet<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

#[gpui_kit::test]
fn the_golden_fixtures_render_with_the_ported_cell_texts(cx: &mut TestAppContext) {
  let dir = TempDir::new("table-golden");
  let (paths, parsed) = golden_files(&dir, "");
  let (movie, mka, srt) = (paths[0].clone(), paths[1].clone(), paths[2].clone());
  let shell = shell(cx, Config::default(), parsed, &[&movie, &mka, &srt]);
  // Group by file name: one unit, the root and its children by stem.
  let scope = table(&shell, cx, &movie).read_with(cx, |table, _| table.scope().clone());
  assert_eq!(scope.members(), [movie.clone(), srt.clone(), mka.clone()]);

  let texts = shell.stores.files.read_with(cx, |files, _| {
    scope
      .rows(files)
      .iter()
      .map(|row| row_texts(row, scope.qualified_ids(), &Config::default().formatting))
      .collect::<Vec<_>>()
  });
  let summary: Vec<[&str; 8]> = texts
    .iter()
    .map(|row| {
      [
        row.id.as_str(),
        row.emoji,
        row.codec.as_str(),
        row.description.as_str(),
        row.size.as_str(),
        row.bit_rate.as_str(),
        row.language.as_str(),
        row.track_name.as_str(),
      ]
    })
    .collect();
  let expected: Vec<[&str; 8]> = vec![
    [
      "0:0",
      "🎞️",
      "AVC/H.264/MPEG-4p10",
      "1920x1080 23.976fps",
      "5.03GB",
      "7.63Mbps",
      "en",
      "Main Feature",
    ],
    [
      "0:1",
      "🔊",
      "AC-3",
      "6ch 48kHz",
      "411.99MB",
      "625Kbps",
      "ja",
      "Japanese 5.1",
    ],
    ["0:2", "🔊", "AAC", "stereo 44.1kHz", "", "", "en", ""],
    ["2:0", "🔊", "Opus", "stereo 48kHz", "", "", "ja", "Commentary"],
    ["0:3", "💬", "SRT (UTF-8 text)", "Text SRT", "", "", "fr", ""],
    [
      "0:4",
      "💬",
      "SubStationAlpha (ASS)",
      "Text ASS",
      "",
      "",
      "zh",
      "Simplified Chinese",
    ],
    ["1:0", "💬", "SubRip Text", "Text SRT UTF-8", "", "", "und", ""],
    ["0:0", "📑", "Chapters", "3 entries", "", "", "", ""],
    ["0:1", "📎", "jpg", "", "2KB", "", "", "cover.jpg"],
    ["0:2", "📎", "ttf", "", "512B", "", "", "Subtitle Font.TTF"],
  ];
  assert_eq!(summary, expected);
  assert_eq!(texts[0].type_name, "video");

  // Every row is on screen; track rows have both cells, the others none.
  in_table(&shell, cx, &movie, |window, _| {
    for index in 0..10usize {
      window.find(("track-row", index));
    }
    assert!(window.try_find(("track-row", 10usize)).is_none());
    for index in 0..7usize {
      window.find(("track-default", index));
    }
    assert!(window.try_find(("track-default", 7usize)).is_none());
    // The default profile skips attachments and the undetermined subtitle.
    let all = window.find("track-check-all");
    assert_eq!(all.indeterminate(), Some(true));
    assert_eq!(window.find(("track-check", 0usize)).checked(), Some(true));
    assert_eq!(window.find(("track-check", 6usize)).checked(), Some(false));
  });
  let table = table(&shell, cx, &movie);
  table.read_with(cx, |table, cx| {
    let language = table.cell(CellKind::Language, "2:audio:0").expect("a language cell");
    assert_eq!(language.read(cx).value(cx).as_ref(), "ja");
    let name = table.cell(CellKind::TrackName, "0:subtitles:4").expect("a name cell");
    assert_eq!(name.read(cx).value(cx).as_ref(), "Simplified Chinese");
    assert!(table.cell(CellKind::Language, "0:chapters:0").is_none());
  });
}

#[test]
fn sizes_and_bit_rates_follow_the_formatting_settings() {
  let _locale = crate::i18n::tests::LOCALE_LOCK
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  let dir = TempDir::new("table-formatting");
  let (paths, parsed) = golden_files(&dir, "");
  let rows: Vec<_> = crate::domain::file_tree::combine_unit_tracks(
    &paths[..1],
    &HashMap::from([(
      paths[0].clone(),
      bmm_core::media_track::metadata_to_media_tracks(&parsed[&paths[0]]),
    )]),
    None,
  );
  let formatting = ConfigFormatting {
    video: ConfigStreamFormat::default(),
    audio: ConfigStreamFormat {
      bit_rate: ConfigFormatField {
        precision: FormatPrecision::One,
        unit: FormatUnit::KMi,
      },
      size: ConfigFormatField {
        precision: FormatPrecision::Zero,
        unit: FormatUnit::KMiGi,
      },
    },
    subtitle: ConfigStreamFormat::default(),
  };
  let audio = row_texts(&rows[1], false, &formatting);
  assert_eq!(
    (audio.id.as_str(), audio.size.as_str(), audio.bit_rate.as_str()),
    ("1", "432MiB", "640Kibps")
  );
  let video = row_texts(&rows[0], false, &formatting);
  assert_eq!(video.size, "5.03GB");
}

#[gpui_kit::test]
fn checkboxes_select_all_and_flags_edit_the_store(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-checks");
  let all = s(&[
    "video:0",
    "audio:1",
    "audio:2",
    "subtitles:3",
    "subtitles:4",
    "chapters:0",
    "attachment:1",
    "attachment:2",
  ]);
  in_table(&shell, cx, &movie, |window, cx| {
    window.click(("track-check", 2usize), cx)
  });
  assert!(!selected_ids(&shell, cx, &movie).contains(&"audio:2".to_string()));
  // Indeterminate → every row; again → none.
  in_table(&shell, cx, &movie, |window, cx| window.click("track-check-all", cx));
  let mut selected = selected_ids(&shell, cx, &movie);
  selected.sort();
  let mut expected = all.clone();
  expected.sort();
  assert_eq!(selected, expected);
  in_table(&shell, cx, &movie, |window, cx| {
    assert_eq!(window.find("track-check-all").checked(), Some(true));
    window.click("track-check-all", cx);
  });
  assert!(selected_ids(&shell, cx, &movie).is_empty());
  in_table(&shell, cx, &movie, |window, cx| {
    assert_eq!(window.find("track-check-all").checked(), Some(false));
    window.click("track-check-all", cx);
  });

  // The default button cycles false → unspecified → true → false.
  let mut seen = Vec::new();
  for _ in 0..3 {
    in_table(&shell, cx, &movie, |window, cx| {
      window.click(("track-default", 2usize), cx)
    });
    seen.push(track(&shell, cx, &movie, "audio:2", |track| track.default_track));
  }
  use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
  assert_eq!(seen, [TrackFlag::Unspecified, TrackFlag::True, TrackFlag::False]);
  in_table(&shell, cx, &movie, |window, cx| {
    window.click(("track-forced", 0usize), cx)
  });
  assert_eq!(
    track(&shell, cx, &movie, "video:0", |track| track.forced),
    TrackFlag::True
  );

  // The headers: the first checked track of each type is default, forced
  // is cleared.
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("track-reset-default", cx);
    window.click("track-reset-forced", cx);
  });
  let flags: Vec<(TrackFlag, TrackFlag)> = ["video:0", "audio:1", "audio:2", "subtitles:3", "subtitles:4"]
    .iter()
    .map(|key| track(&shell, cx, &movie, key, |track| (track.default_track, track.forced)))
    .collect();
  use TrackFlag::{False, True, Unspecified};
  assert_eq!(
    flags,
    [
      (True, Unspecified),
      (True, Unspecified),
      (False, Unspecified),
      (True, Unspecified),
      (False, Unspecified)
    ]
  );
}

#[gpui_kit::test]
fn an_edit_of_a_selected_row_applies_to_every_selected_row(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-targets");
  click_row(&shell, cx, &movie, 1);
  click_row(&shell, cx, &movie, 2);
  in_table(&shell, cx, &movie, |window, cx| {
    window.click(("track-check", 1usize), cx)
  });
  let selected = selected_ids(&shell, cx, &movie);
  assert!(!selected.contains(&"audio:1".to_string()) && !selected.contains(&"audio:2".to_string()));
  // An unselected row's control edits that row alone.
  in_table(&shell, cx, &movie, |window, cx| {
    window.click(("track-check", 0usize), cx)
  });
  assert!(!selected_ids(&shell, cx, &movie).contains(&"video:0".to_string()));
  assert!(!selected_ids(&shell, cx, &movie).contains(&"audio:1".to_string()));
}

#[gpui_kit::test]
fn selected_track_rows_follow_the_active_theme(cx: &mut TestAppContext) {
  use bmm_core::config::{DisplayMode, Theme as ThemeName};

  let (shell, movie, _dir) = movie_shell(cx, "table-selection-color");
  click_row(&shell, cx, &movie, 1);
  click_row(&shell, cx, &movie, 2);
  for (theme, display_mode) in [
    (ThemeName::Ocean, DisplayMode::Light),
    (ThemeName::Rose, DisplayMode::Dark),
  ] {
    shell.stores.config.update(cx, |store, cx| {
      store.update(cx, |config| {
        config.theme = theme;
        config.display_mode = display_mode;
      });
    });
    cx.run_until_parked();
    shell.window(cx, |window, cx| {
      window.render_frame(cx);
      let selection = crate::theme::selection_highlight(cx.theme());
      let quads = window.painted_quads();
      for (index, selected) in [(0usize, false), (1, true), (2, true)] {
        let bounds = window.find(("track-row", index)).bounds().scale(window.scale_factor());
        let has_highlight = quads
          .iter()
          .any(|quad| quad.bounds == bounds && quad.background.as_solid() == Some(selection));
        assert_eq!(has_highlight, selected, "row {index}, {theme:?} in {display_mode:?}");
      }
    });
  }
}

#[gpui_kit::test]
fn the_keys_move_the_cursor_select_rows_and_flip_checkboxes(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-keys");
  let table = table(&shell, cx, &movie);
  click_row(&shell, cx, &movie, 1);
  assert_eq!(
    shell
      .stores
      .files
      .read_with(cx, |files, _| files.active_card().map(str::to_string)),
    Some(movie.clone()),
    "a click makes the card active"
  );
  let state = |cx: &mut TestAppContext| {
    table.read_with(cx, |table, _| {
      (table.cursor().map(str::to_string), table.selected_rows().clone())
    })
  };
  assert_eq!(state(cx), (Some("0:audio:1".into()), set(&["0:audio:1"])));
  shell.press(cx, "down");
  assert_eq!(state(cx), (Some("0:audio:2".into()), set(&["0:audio:1", "0:audio:2"])));
  shell.press(cx, "up");
  assert_eq!(state(cx), (Some("0:audio:1".into()), set(&["0:audio:2"])));
  shell.press(cx, "space");
  assert!(
    !selected_ids(&shell, cx, &movie).contains(&"audio:2".to_string()),
    "Space flips"
  );
  shell.press(cx, "space");
  assert!(selected_ids(&shell, cx, &movie).contains(&"audio:2".to_string()));
  shell.press(cx, "ctrl-a");
  assert_eq!(state(cx).1.len(), 8);
  shell.press(cx, "ctrl-shift-a");
  assert!(state(cx).1.is_empty());
  shell.press(cx, "cmd-a");
  assert_eq!(state(cx).1.len(), 8);
  shell.press(cx, "cmd-shift-a");
  // Up at the top stays on the first row.
  for _ in 0..3 {
    shell.press(cx, "up");
  }
  assert_eq!(state(cx).0.as_deref(), Some("0:video:0"));

  // Another tab: the keys leave the hidden table alone.
  shell.press(cx, "ctrl-2");
  shell.press(cx, "ctrl-a");
  assert_eq!(state(cx).1.len(), 1);
  shell.press(cx, "ctrl-1");

  // In a text field the keys are the field's.
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("language-0:audio:1-input", cx)
  });
  shell.press(cx, "down");
  shell.press(cx, "ctrl-a");
  assert_eq!(state(cx).0.as_deref(), Some("0:video:0"));
  assert_eq!(state(cx).1.len(), 1);
  let cell = table.read_with(cx, |table, _| table.cell(CellKind::Language, "0:audio:1").unwrap());
  assert!(cell.read_with(cx, |cell, _| cell.is_open()), "↓ opened the list");
}

#[gpui_kit::test]
fn a_merging_card_takes_no_edits(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-merging");
  click_row(&shell, cx, &movie, 0);
  shell.press(cx, "f3");
  shell.send(cx, started(&movie));
  in_table(&shell, cx, &movie, |window, _| {
    assert_eq!(window.find(("track-check", 0usize)).disabled(), Some(true));
    assert_eq!(window.find("track-reset-default").disabled(), Some(true));
    assert_eq!(window.find(("track-default", 0usize)).disabled(), Some(true));
  });
  let table = table(&shell, cx, &movie);
  shell.press(cx, "ctrl-a");
  shell.press(cx, "down");
  table.read_with(cx, |table, _| {
    assert_eq!(table.selected_rows().len(), 1);
    assert_eq!(table.cursor(), Some("0:video:0"));
  });
}

#[gpui_kit::test]
fn dragging_a_row_past_five_pixels_reorders_and_a_wobble_selects(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-drag");
  let (from, to, wobble) = in_table(&shell, cx, &movie, |window, _| {
    let row = |index: usize| window.find(("track-row", index)).bounds();
    let from = row(2).origin + point(px(60.0), row(2).size.height / 2.0);
    // Above the centre of row 1: insert before it.
    let to = row(1).origin + point(px(60.0), px(4.0));
    let wobble = row(3).origin + point(px(300.0), px(10.0));
    (from, to, wobble)
  });
  shell.window(cx, |window, cx| window.drag(from, to, cx));
  cx.run_until_parked();
  assert_eq!(
    row_keys(&shell, cx, &movie)[..4],
    ["0:video:0", "0:audio:2", "0:audio:1", "0:subtitles:3"]
  );
  let table = table(&shell, cx, &movie);
  table.read_with(cx, |table, _| {
    assert_eq!(table.drag_state(), DragState::Idle);
    assert!(table.selected_rows().is_empty(), "a drag selects nothing");
  });
  shell.window(cx, |window, cx| {
    window.drag(wobble, wobble + point(px(3.0), px(3.0)), cx)
  });
  cx.run_until_parked();
  table.read_with(cx, |table, _| {
    assert_eq!(table.selected_rows(), &set(&["0:subtitles:3"]));
  });
  assert_eq!(row_keys(&shell, cx, &movie)[3], "0:subtitles:3", "no move");
}

#[gpui_kit::test]
fn a_group_cards_table_edits_every_unit(cx: &mut TestAppContext) {
  let dir = TempDir::new("table-group");
  let (first, mut parsed) = golden_files(&dir, "a-");
  let (second, more) = golden_files(&dir, "b-");
  parsed.extend(more);
  let config = Config {
    group_mode: GroupMode::TrackCount,
    group_by_file_name: false,
    ..Config::default()
  };
  let shell = shell(cx, config, parsed, &[&first[0], &second[0]]);
  // Wide enough for the root list and the whole table.
  shell.resize(cx, 1400.0, 760.0);
  let scope = table(&shell, cx, &first[0]).read_with(cx, |table, _| table.scope().clone());
  assert!(scope.is_group());
  in_table(&shell, cx, &first[0], |window, cx| {
    window.click(("track-check", 2usize), cx);
    window.click(("track-forced", 1usize), cx);
  });
  for file in [&first[0], &second[0]] {
    assert!(
      !selected_ids(&shell, cx, file).contains(&"audio:2".to_string()),
      "{file}"
    );
    assert_eq!(
      track(&shell, cx, file, "audio:1", |track| track.forced),
      bmm_core::media_metadata::model::track_properties_common::TrackFlag::True
    );
  }
  // Single-member units reorder in lock-step.
  let (from, to) = in_table(&shell, cx, &first[0], |window, _| {
    let row = |index: usize| window.find(("track-row", index)).bounds();
    (
      row(2).origin + point(px(60.0), px(12.0)),
      row(1).origin + point(px(60.0), px(4.0)),
    )
  });
  shell.window(cx, |window, cx| window.drag(from, to, cx));
  cx.run_until_parked();
  for file in [&first[0], &second[0]] {
    let order = shell
      .stores
      .files
      .read_with(cx, |files, _| files.tracks(file).unwrap()[1].key());
    assert_eq!(order, "audio:2", "{file}");
  }
}

/// Leaving a language cell settles what was typed: a tag in the usual case,
/// and text mkvmerge would reject goes back to the value before the edit.
#[gpui_kit::test]
fn the_language_cell_settles_or_reverts_when_it_loses_the_focus(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-language-blur");
  let language = |cx: &mut TestAppContext| track(&shell, cx, &movie, "audio:1", |track| track.language.clone());
  let input = "language-0:audio:1-input";
  let elsewhere = "name-0:audio:2-input";
  in_table(&shell, cx, &movie, |window, cx| window.click(input, cx));
  shell.window(cx, |window, cx| window.input("pt-br", cx));
  cx.run_until_parked();
  assert_eq!(language(cx), "pt-br", "every keystroke reaches the row");
  // Escape closes the list, which may cover the cell clicked next.
  shell.press(cx, "escape");
  in_table(&shell, cx, &movie, |window, cx| window.click(elsewhere, cx));
  cx.run_until_parked();
  assert_eq!(language(cx), "pt-BR");
  in_table(&shell, cx, &movie, |window, cx| window.click(input, cx));
  shell.window(cx, |window, cx| window.input("Englsh", cx));
  cx.run_until_parked();
  shell.press(cx, "escape");
  in_table(&shell, cx, &movie, |window, cx| window.click(elsewhere, cx));
  cx.run_until_parked();
  assert_eq!(language(cx), "pt-BR", "a typo goes back");
  in_table(&shell, cx, &movie, |window, cx| window.click(input, cx));
  shell.window(cx, |window, cx| window.input("xx", cx));
  shell.press(cx, "enter");
  assert_eq!(language(cx), "pt-BR", "Enter does not take it either");
  // A language taken with Enter is what a later typo in the same edit goes
  // back to.
  shell.press(cx, SELECT_ALL);
  shell.window(cx, |window, cx| window.input("de", cx));
  shell.press(cx, "enter");
  assert_eq!(language(cx), "de");
  shell.press(cx, SELECT_ALL);
  shell.window(cx, |window, cx| window.input("Englsh", cx));
  cx.run_until_parked();
  shell.press(cx, "escape");
  in_table(&shell, cx, &movie, |window, cx| window.click(elsewhere, cx));
  cx.run_until_parked();
  assert_eq!(language(cx), "de", "the typo goes back to the language taken");
}

#[gpui_kit::test]
fn the_language_cell_lists_preferred_languages_and_normalises_on_enter(cx: &mut TestAppContext) {
  let (shell, movie, _dir) = movie_shell(cx, "table-language");
  let input = "language-0:audio:1-input";
  in_table(&shell, cx, &movie, |window, cx| window.click(input, cx));
  let table = table(&shell, cx, &movie);
  let cell = table.read_with(cx, |table, _| table.cell(CellKind::Language, "0:audio:1").unwrap());
  cell.read_with(cx, |cell, _| {
    assert!(cell.is_open());
    assert_eq!(cell.placement(), Placement::Below);
    assert_eq!(cell.options()[..7], ["en", "zh", "es", "de", "fr", "ja", "und"]);
    assert_eq!(cell.preferred_count(), 6);
    assert_eq!((cell.matching(), cell.highlighted()), (Some(5), Some(5)), "ja");
    assert!(cell.list_width() >= px(280.0));
  });
  // Every keystroke reaches the row; Enter turns the alias into its code.
  shell.window(cx, |window, cx| window.input("ger", cx));
  cx.run_until_parked();
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "ger"
  );
  cell.read_with(cx, |cell, _| assert_eq!(cell.matching(), Some(3), "de"));
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "de"
  );
  cell.read_with(cx, |cell, cx| {
    assert!(!cell.is_open());
    assert_eq!(cell.value(cx).as_ref(), "de");
  });

  // ↓ reopens the list on the current value; ↑/↓ move, Enter takes it.
  shell.press(cx, "down");
  cell.read_with(cx, |cell, _| {
    assert_eq!((cell.is_open(), cell.highlighted()), (true, Some(3)))
  });
  shell.press(cx, "down");
  shell.press(cx, "up");
  shell.press(cx, "up");
  cell.read_with(cx, |cell, _| assert_eq!(cell.highlighted(), Some(2)));
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "es"
  );
  // Escape closes the list and keeps the value.
  shell.press(cx, "down");
  shell.press(cx, "escape");
  cell.read_with(cx, |cell, cx| {
    assert!(!cell.is_open());
    assert_eq!(cell.value(cx).as_ref(), "es");
  });
  // A click on an option takes it.
  in_table(&shell, cx, &movie, |window, cx| window.click(input, cx));
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.click(("option", 1usize), cx);
  });
  cx.run_until_parked();
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "zh"
  );
}

#[gpui_kit::test]
fn a_language_tag_stays_and_names_follow_the_language(cx: &mut TestAppContext) {
  let dir = TempDir::new("table-names");
  let (paths, parsed) = golden_files(&dir, "");
  let mut config = Config::default();
  config.profiles[0]
    .track_names_audio
    .insert("jpn".to_string(), "Japanese\nCommentary\njapanese".to_string());
  let shell = shell(cx, config, parsed, &[paths[0].as_str()]);
  let movie = paths[0].clone();
  let table = table(&shell, cx, &movie);
  let cell = table.read_with(cx, |table, _| table.cell(CellKind::TrackName, "0:audio:1").unwrap());
  cell.read_with(cx, |cell, _| {
    assert_eq!(
      cell.options(),
      ["Japanese", "Commentary"],
      "deduplicated, case-insensitively"
    );
    assert_eq!(cell.matching(), None, "\"Japanese 5.1\" matches no preset");
  });
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("name-0:audio:1-input", cx)
  });
  shell.window(cx, |window, cx| window.input("Comm", cx));
  cx.run_until_parked();
  cell.read_with(cx, |cell, _| {
    assert_eq!((cell.matching(), cell.highlighted()), (Some(1), None))
  });
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.track_name.clone()),
    "Comm"
  );
  shell.press(cx, "down");
  shell.press(cx, "down");
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.track_name.clone()),
    "Japanese"
  );

  // A language tag that is not listed stays (trimmed, in the usual case);
  // the name cell then has no presets and no list. Text mkvmerge would
  // reject goes back to the value before the edit.
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("language-0:audio:1-input", cx)
  });
  shell.window(cx, |window, cx| window.input("Klingon ", cx));
  cx.run_until_parked();
  shell.press(cx, "escape");
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "ja"
  );
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("language-0:audio:1-input", cx)
  });
  shell.window(cx, |window, cx| window.input("pt-br ", cx));
  cx.run_until_parked();
  shell.press(cx, "escape");
  shell.press(cx, "enter");
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.language.clone()),
    "pt-BR"
  );
  cell.read_with(cx, |cell, _| assert!(cell.options().is_empty()));
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("name-0:audio:1-input", cx)
  });
  cell.read_with(cx, |cell, _| assert!(!cell.is_open()));
}

#[gpui_kit::test]
fn word_keys_stop_inside_numbers_in_a_cell(cx: &mut TestAppContext) {
  use gpui_kit::component::WindowExt;
  use gpui_kit::component::input::{AnyInputState, InputState};

  let (shell, movie, _dir) = movie_shell(cx, "table-word-keys");
  in_table(&shell, cx, &movie, |window, cx| {
    window.click("name-0:audio:1-input", cx)
  });
  let field = |shell: &Shell, cx: &mut TestAppContext| -> Entity<InputState> {
    shell.window(cx, |window, cx| match window.focused_input(cx) {
      Some(AnyInputState::Input(state)) => state,
      _ => panic!("the name cell has the focus"),
    })
  };
  let input = field(&shell, cx);
  assert_eq!(
    input.read_with(cx, |state, _| state.value().to_string()),
    "Japanese 5.1"
  );
  input.update(cx, |state, cx| state.set_selected_range(12..12, cx));
  let word = if cfg!(target_os = "macos") { "alt" } else { "ctrl" };
  shell.press(cx, &format!("{word}-left"));
  assert_eq!(input.read_with(cx, |state, _| state.cursor()), 11);
  // A word deletion is an edit of the row like any other.
  input.update(cx, |state, cx| state.set_selected_range(12..12, cx));
  shell.press(cx, &format!("{word}-backspace"));
  assert_eq!(
    track(&shell, cx, &movie, "audio:1", |track| track.track_name.clone()),
    "Japanese 5."
  );
}

#[gpui_kit::test]
fn the_list_opens_above_a_cell_near_the_bottom_of_the_window(cx: &mut TestAppContext) {
  let dir = TempDir::new("table-placement");
  let (first, mut parsed) = golden_files(&dir, "a-");
  let (second, more) = golden_files(&dir, "b-");
  parsed.extend(more);
  let config = Config {
    group_mode: GroupMode::None,
    group_by_file_name: false,
    ..Config::default()
  };
  let shell = shell(cx, config, parsed, &[&first[0], &second[0]]);
  let bottom = in_table(&shell, cx, &second[0], |window, _| {
    window.find("language-0:audio:1-input").bounds().bottom()
  });
  assert!(bottom > px(500.0), "the second card is low in the window: {bottom:?}");
  in_table(&shell, cx, &second[0], |window, cx| {
    window.click("language-0:audio:1-input", cx)
  });
  let cell =
    table(&shell, cx, &second[0]).read_with(cx, |table, _| table.cell(CellKind::Language, "0:audio:1").unwrap());
  cell.read_with(cx, |cell, _| {
    assert!(cell.is_open());
    assert_eq!(cell.placement(), Placement::Above);
  });
  let list = shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.find("language-0:audio:1-list").bounds()
  });
  assert!(
    list.bottom() <= px(bottom.as_f32() - 20.0),
    "the list sits above the cell: {list:?}"
  );
}

#[test]
fn drag_geometry() {
  let rows: Vec<Bounds<Pixels>> = (0..3)
    .map(|i| Bounds::new(point(px(0.0), px(i as f32 * 34.0)), size(px(100.0), px(34.0))))
    .collect();
  assert_eq!(insert_index(&rows, px(0.0)), 0);
  assert_eq!(insert_index(&rows, px(18.0)), 1);
  assert_eq!(insert_index(&rows, px(50.0)), 1);
  assert_eq!(insert_index(&rows, px(52.0)), 2);
  assert_eq!(insert_index(&rows, px(200.0)), 3);
  // Dropping a row next to itself moves nothing.
  assert_eq!(move_target(2, 2), None);
  assert_eq!(move_target(2, 3), None);
  assert_eq!(move_target(2, 1), Some(1));
  assert_eq!(move_target(1, 4), Some(3));
  assert_eq!(move_target(0, 3), Some(2));
}

#[test]
fn the_cursor_steps_and_stops_at_the_ends() {
  assert_eq!(step_cursor(0, None, true), None);
  assert_eq!(step_cursor(4, None, true), Some(0));
  assert_eq!(step_cursor(4, None, false), Some(3));
  assert_eq!(step_cursor(4, Some(3), true), Some(3));
  assert_eq!(step_cursor(4, Some(0), false), Some(0));
  assert_eq!(step_cursor(4, Some(1), true), Some(2));
}
