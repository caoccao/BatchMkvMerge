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

//! Headless tests of the Settings tab in the application shell: every page
//! mounts, every control writes its config field (checked against the
//! serialised config), Reset All, the profile editors, the tool path rows with
//! a stand-in detector, and typing that never reverts.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmm_core::config::{Config, ConfigProfile, Language};
use core::prelude::v1::test;
use gpui_kit::component::input::InputState;
use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::*;
use serde_json::{Value, json};

use super::language_picker::{LanguageDrag, Pane, drop_on_available, drop_on_preferred};
use super::*;
use crate::tools::{Tool, ToolDetector, ToolStatus};
use crate::ui::tests::Shell;

const SELECT_ALL: &str = if cfg!(target_os = "macos") { "cmd-a" } else { "ctrl-a" };

fn config() -> Config {
  Config {
    language: Language::EnUS,
    ..Config::default()
  }
}

type Calls = Arc<Mutex<Vec<(Tool, String, bool)>>>;

/// Finds MKVToolNix only in `/opt/mkvtoolnix` (also from `/opt/apps`, as a
/// path elsewhere, and for Detect as a running application) and
/// BetterMediaInfo only in `/opt/bmi`; records every call.
fn detector() -> (ToolDetector, Calls) {
  let calls = Calls::default();
  let seen = calls.clone();
  let tools = ToolDetector::new(move |tool, path, check_running| {
    seen.lock().unwrap().push((tool, path.to_string(), check_running));
    let found = |path: &str| ToolStatus {
      found: true,
      path: path.to_string(),
    };
    match (tool, path, check_running) {
      (Tool::MkvToolNix, _, true) => found("/run/mkvtoolnix"),
      (Tool::MkvToolNix, "/opt/mkvtoolnix", _) => found("/opt/mkvtoolnix"),
      (Tool::MkvToolNix, "/opt/apps", _) => found("/opt/mkvtoolnix"),
      (Tool::BetterMediaInfo, "/opt/bmi", _) => found("/opt/bmi"),
      _ => ToolStatus {
        found: false,
        path: path.to_string(),
      },
    }
  });
  (tools, calls)
}

/// The shell on the Settings tab, tall enough to lay out every group.
fn open_with(cx: &mut TestAppContext, config: Config, tools: ToolDetector) -> Shell {
  let shell = Shell::with_tools(cx, config, HashMap::new(), tools);
  shell.resize(cx, 1200.0, 3200.0);
  shell.window(cx, |window, _| window.activate_window());
  shell.press(cx, "f10");
  draw(&shell, cx);
  shell
}

fn open(cx: &mut TestAppContext) -> Shell {
  open_with(cx, config(), detector().0)
}

fn settings(shell: &Shell, cx: &mut TestAppContext) -> Entity<SettingsView> {
  shell
    .view
    .read_with(cx, |view, cx| view.main().read(cx).settings().cloned())
    .expect("the settings view")
}

fn draw(shell: &Shell, cx: &mut TestAppContext) {
  cx.run_until_parked();
  shell.window(cx, |window, cx| window.render_frame(cx));
}

fn click_at(shell: &Shell, cx: &mut TestAppContext, position: Point<Pixels>) {
  VisualTestContext::from_window(shell.handle, cx).simulate_click(position, Modifiers::none());
  draw(shell, cx);
}

/// Clicks the one element inside the settings with this accessible name.
fn click_label(shell: &Shell, cx: &mut TestAppContext, label: &str) {
  draw(shell, cx);
  let center = shell.window(cx, |window, _| {
    window.within("settings").find_by_label(label).bounds().center()
  });
  click_at(shell, cx, center);
}

/// Clicks a menu item, which opens outside the settings.
fn click_menu_item(shell: &Shell, cx: &mut TestAppContext, label: &str) {
  draw(shell, cx);
  let center = shell.window(cx, |window, _| {
    let items: Vec<_> = window
      .find_all_by_label(label)
      .into_iter()
      .filter(|item| item.role() == Some(Role::MenuItem) || item.role() == Some(Role::MenuItemCheckBox))
      .collect();
    assert_eq!(items.len(), 1, "one menu item {label:?}: {items:?}");
    items[0].bounds().center()
  });
  click_at(shell, cx, center);
}

fn click_id(shell: &Shell, cx: &mut TestAppContext, id: impl Into<ElementId>) {
  draw(shell, cx);
  let id = id.into();
  shell.window(cx, |window, cx| window.click(id, cx));
  draw(shell, cx);
}

/// The pages in sidebar order.
const PAGES: [&str; 6] = ["Appearance", "Formatting", "Group", "Integration", "Profiles", "Update"];

/// Shows a page: its entry in the sidebar (gpui-kit names the entries by
/// their index).
fn page(shell: &Shell, cx: &mut TestAppContext, title: &str) {
  let index = PAGES.iter().position(|page| *page == title).expect("a page");
  draw(shell, cx);
  let center = shell.window(cx, |window, _| {
    window
      .within("settings")
      .find(SharedString::from(format!("0-{index}")))
      .bounds()
      .center()
  });
  // The label, away from a submenu's caret on the right.
  click_at(shell, cx, center - point(px(40.0), px(0.0)));
}

fn json_of(shell: &Shell, cx: &mut TestAppContext) -> Value {
  shell
    .stores
    .config
    .read_with(cx, |store, _| serde_json::to_value(store.config()).unwrap())
}

/// The leaves that differ, as JSON pointers.
fn changes(before: &Value, after: &Value) -> BTreeMap<String, Value> {
  fn walk(path: String, before: Option<&Value>, after: Option<&Value>, out: &mut BTreeMap<String, Value>) {
    match (before, after) {
      (Some(Value::Object(a)), Some(Value::Object(b))) => {
        for key in a.keys().chain(b.keys()).collect::<std::collections::BTreeSet<_>>() {
          walk(format!("{path}/{key}"), a.get(key), b.get(key), out);
        }
      }
      (Some(Value::Array(a)), Some(Value::Array(b))) => {
        for index in 0..a.len().max(b.len()) {
          walk(format!("{path}/{index}"), a.get(index), b.get(index), out);
        }
      }
      (a, b) if a != b => {
        out.insert(path, b.cloned().unwrap_or(Value::Null));
      }
      _ => {}
    }
  }
  let mut out = BTreeMap::new();
  walk(String::new(), Some(before), Some(after), &mut out);
  out
}

/// Runs `act`, then checks that exactly `expected` changed in the config and
/// that the newest write is the result.
fn assert_writes(
  shell: &Shell,
  cx: &mut TestAppContext,
  what: &str,
  act: impl FnOnce(&Shell, &mut TestAppContext),
  expected: &[(&str, Value)],
) {
  let before = json_of(shell, cx);
  act(shell, cx);
  draw(shell, cx);
  let after = json_of(shell, cx);
  let expected: BTreeMap<String, Value> = expected
    .iter()
    .map(|(path, value)| (path.to_string(), value.clone()))
    .collect();
  assert_eq!(changes(&before, &after), expected, "{what}");
  let written = shell.written.lock().unwrap().last().cloned().expect("a write");
  assert_eq!(serde_json::to_value(written).unwrap(), after, "{what}: persisted");
}

fn focus(shell: &Shell, cx: &mut TestAppContext, input: &Entity<InputState>) {
  let input = input.clone();
  shell.window(cx, |window, cx| input.update(cx, |input, cx| input.focus(window, cx)));
  cx.run_until_parked();
}

/// Replaces the text of a focused field by typing.
fn type_into(shell: &Shell, cx: &mut TestAppContext, input: &Entity<InputState>, text: &str) {
  focus(shell, cx, input);
  shell.window(cx, |window, cx| {
    window.press(SELECT_ALL, cx);
    if text.is_empty() {
      window.press("backspace", cx);
    } else {
      window.input(text, cx);
    }
  });
  draw(shell, cx);
}

fn texts(shell: &Shell, cx: &mut TestAppContext) -> Vec<String> {
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .map(|text| text.text().to_string())
      .collect()
  })
}

fn profile(shell: &Shell, cx: &mut TestAppContext) -> ConfigProfile {
  shell.stores.config.read_with(cx, |store, _| {
    crate::state::config::active_profile(store.config()).cloned().unwrap()
  })
}

#[gpui_kit::test]
fn every_page_and_group_mounts(cx: &mut TestAppContext) {
  let shell = open(cx);
  let pages: [(&str, &[&str]); 6] = [
    (
      "Appearance",
      &["settings-mode-auto", "settings-mode-light", "settings-mode-dark"],
    ),
    (
      "Formatting",
      &[
        "settings-format-video-bit-rate-precision",
        "settings-format-audio-size-unit",
        "settings-format-subtitles-size-precision",
      ],
    ),
    ("Group", &[]),
    ("Integration", &["settings-mkvtoolnix", "settings-better-media-info"]),
    (
      "Profiles",
      &[
        "settings-profile-row",
        "language-picker",
        "track-names",
        "settings-automation-und",
      ],
    ),
    ("Update", &[]),
  ];
  for (title, ids) in pages {
    page(&shell, cx, title);
    shell.window(cx, |window, _| {
      for id in ids {
        window.find(SharedString::from(*id));
      }
    });
  }
  page(&shell, cx, "Group");
  shell.window(cx, |window, _| {
    for mode in ["No grouping", "Group by identical track count per type"] {
      assert!(window.within("settings").try_find_by_label(mode).is_some(), "{mode}");
    }
  });
  // Pages with titled groups list them in the sidebar.
  shell.window(cx, |window, _| {
    for title in ["Formatting", "Integration", "Profiles"] {
      let caret = format!("Toggle {title} submenu");
      assert!(window.within("settings").try_find_by_label(&caret).is_some(), "{caret}");
    }
    assert!(
      window
        .within("settings")
        .try_find_by_label("Toggle Appearance submenu")
        .is_none()
    );
  });
  page(&shell, cx, "Update");
  assert!(shell.shows(cx, "Check New Version: Weekly"));
}

#[gpui_kit::test]
fn appearance_formatting_group_and_update_controls_write_their_fields(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Appearance");
  assert_writes(
    &shell,
    cx,
    "mode",
    |shell, cx| click_id(shell, cx, "settings-mode-dark"),
    &[("/displayMode", json!("Dark"))],
  );
  assert_writes(
    &shell,
    cx,
    "theme",
    |shell, cx| {
      click_label(shell, cx, "Theme: Ocean");
      click_menu_item(shell, cx, "Aqua");
    },
    &[("/theme", json!("Aqua"))],
  );
  page(&shell, cx, "Formatting");
  assert_writes(
    &shell,
    cx,
    "video bit rate precision",
    |shell, cx| {
      click_id(shell, cx, "settings-format-video-bit-rate-precision");
      click_menu_item(shell, cx, "#");
    },
    &[("/formatting/video/bitRate/precision", json!("Zero"))],
  );
  assert_writes(
    &shell,
    cx,
    "subtitle size unit",
    |shell, cx| {
      click_id(shell, cx, "settings-format-subtitles-size-unit");
      click_menu_item(shell, cx, "k/Mi");
    },
    &[("/formatting/subtitle/size/unit", json!("KMi"))],
  );
  page(&shell, cx, "Group");
  assert_writes(
    &shell,
    cx,
    "group mode",
    |shell, cx| click_label(shell, cx, "Group by identical track count and language per type"),
    &[("/groupMode", json!("TrackCountAndLanguage"))],
  );
  assert_writes(
    &shell,
    cx,
    "group by file name",
    |shell, cx| click_label(shell, cx, "Group by file name"),
    &[("/groupByFileName", json!(false))],
  );
  page(&shell, cx, "Update");
  assert_writes(
    &shell,
    cx,
    "check interval",
    |shell, cx| {
      click_label(shell, cx, "Check New Version: Weekly");
      click_menu_item(shell, cx, "Daily");
    },
    &[("/update/checkInterval", json!("Daily"))],
  );
  // Last, as it changes every text: the language.
  page(&shell, cx, "Appearance");
  assert_writes(
    &shell,
    cx,
    "language",
    |shell, cx| {
      click_label(shell, cx, "Language: English (US)");
      click_menu_item(shell, cx, "Deutsch");
    },
    &[("/language", json!("de"))],
  );
  crate::i18n::set_language(Language::EnUS);
}

#[gpui_kit::test]
fn reset_all_puts_a_page_back_to_its_defaults(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Formatting");
  // Nothing differs yet: no Reset All.
  assert!(shell.window(cx, |window, _| window.within("settings").try_find("reset").is_none()));
  click_id(&shell, cx, "settings-format-audio-size-precision");
  click_menu_item(&shell, cx, "#.#");
  click_id(&shell, cx, "settings-format-video-bit-rate-unit");
  click_menu_item(&shell, cx, "k");
  assert_eq!(
    json_of(&shell, cx)["formatting"]["audio"]["size"]["precision"],
    json!("One")
  );
  assert_writes(
    &shell,
    cx,
    "reset all",
    |shell, cx| click_id(shell, cx, "reset"),
    &[
      ("/formatting/audio/size/precision", json!("Two")),
      ("/formatting/video/bitRate/unit", json!("KMGT")),
    ],
  );
  // Appearance: mode and theme.
  page(&shell, cx, "Appearance");
  click_id(&shell, cx, "settings-mode-light");
  assert_writes(
    &shell,
    cx,
    "reset appearance",
    |shell, cx| click_id(shell, cx, "reset"),
    &[("/displayMode", json!("Auto"))],
  );
  // Profiles have their own Reset only.
  page(&shell, cx, "Profiles");
  click_label(&shell, cx, "Attachments");
  assert!(shell.window(cx, |window, _| window.within("settings").try_find("reset").is_none()));
}

#[gpui_kit::test]
fn profiles_are_added_switched_deleted_and_reset_after_asking(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Profiles");
  let view = settings(&shell, cx);
  let new_name = view.read_with(cx, |view, cx| view.profile().read(cx).new_name().clone());
  type_into(&shell, cx, &new_name, " Anime ");
  assert_writes(
    &shell,
    cx,
    "add",
    |shell, cx| click_id(shell, cx, "settings-profile-add"),
    &[
      ("/activeProfile", json!("Anime")),
      // A new profile: Default's values, nothing selected on its own.
      (
        "/profiles/1",
        serde_json::to_value(ConfigProfile::named("Anime")).unwrap(),
      ),
    ],
  );
  assert_eq!(new_name.read_with(cx, |input, _| input.value().to_string()), "");
  // A taken name cannot be added.
  type_into(&shell, cx, &new_name, "Default");
  shell.window(cx, |window, _| {
    assert_eq!(window.find("settings-profile-add").disabled(), Some(true));
  });
  // The active profile from the select.
  assert_writes(
    &shell,
    cx,
    "switch",
    |shell, cx| {
      click_label(shell, cx, "Active Profile: Anime");
      click_menu_item(shell, cx, "Default");
    },
    &[("/activeProfile", json!("Default"))],
  );
  shell.window(cx, |window, _| {
    assert_eq!(
      window.find("settings-profile-delete").disabled(),
      Some(true),
      "Default stays"
    );
  });
  shell
    .stores
    .config
    .update(cx, |store, cx| store.set_active_profile("Anime", cx));
  draw(&shell, cx);
  // Delete asks; No keeps the profile.
  click_id(&shell, cx, "settings-profile-delete");
  assert!(shell.window(cx, gpui_kit::component::WindowExt::has_active_dialog));
  click_id(&shell, cx, "settings-profile-delete-no");
  assert_eq!(profile(&shell, cx).name, "Anime");
  assert_writes(
    &shell,
    cx,
    "delete",
    |shell, cx| {
      click_id(shell, cx, "settings-profile-delete");
      click_id(shell, cx, "settings-profile-delete-yes");
    },
    &[("/activeProfile", json!("Default")), ("/profiles/1", Value::Null)],
  );
  // Reset asks and puts the active profile back to its template.
  click_label(&shell, cx, "Video");
  assert!(profile(&shell, cx).select_video);
  click_id(&shell, cx, "settings-profile-reset");
  shell.press(cx, "escape");
  assert!(profile(&shell, cx).select_video, "Escape answers No");
  assert_writes(
    &shell,
    cx,
    "reset",
    |shell, cx| {
      click_id(shell, cx, "settings-profile-reset");
      click_id(shell, cx, "settings-profile-reset-yes");
    },
    &[("/profiles/0/selectVideo", json!(false))],
  );
}

#[gpui_kit::test]
fn track_selection_and_automation_write_the_active_profile(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Profiles");
  let view = settings(&shell, cx);
  let video_filter = view.read_with(cx, |view, cx| view.profile().read(cx).filter(StreamKind::Video).clone());
  let checkboxes = [
    ("Video", "/profiles/0/selectVideo", true),
    ("Audio", "/profiles/0/selectAudio", false),
    ("Subtitles", "/profiles/0/selectSubtitle", false),
    ("Chapters", "/profiles/0/selectChapters", false),
    ("Attachments", "/profiles/0/selectAttachments", true),
    (
      "Automatically set language und to",
      "/profiles/0/automation/reset_und_language/enabled",
      true,
    ),
    (
      "Automatically set track name per language",
      "/profiles/0/automation/set_track_name/enabled",
      true,
    ),
    (
      "Automatically reset default track",
      "/profiles/0/automation/reset_default_track/enabled",
      true,
    ),
    (
      "Automatically reset forced display",
      "/profiles/0/automation/reset_forced_display/enabled",
      true,
    ),
  ];
  for (label, path, value) in checkboxes {
    assert_writes(
      &shell,
      cx,
      label,
      |shell, cx| click_label(shell, cx, label),
      &[(path, json!(value))],
    );
  }
  assert_writes(
    &shell,
    cx,
    "video languages",
    |shell, cx| type_into(shell, cx, &video_filter, "jpn, eng"),
    &[("/profiles/0/videoLanguagesForTrackSelection", json!("jpn, eng"))],
  );
  // The und language: a searchable select of short codes.
  assert_writes(
    &shell,
    cx,
    "und language",
    |shell, cx| {
      // The search box has the focus; Enter takes the first match.
      click_id(shell, cx, "settings-automation-und-language");
      shell.window(cx, |window, cx| window.input("japan", cx));
      draw(shell, cx);
      shell.press(cx, "enter");
    },
    &[("/profiles/0/automation/reset_und_language/language", json!("ja"))],
  );
}

#[gpui_kit::test]
fn rapid_typing_never_reverts_while_writes_are_pending(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Profiles");
  let view = settings(&shell, cx);
  let input = view.read_with(cx, |view, cx| view.profile().read(cx).filter(StreamKind::Audio).clone());
  type_into(&shell, cx, &input, "");
  let writes_before = shell.written.lock().unwrap().len();
  let text: String = "abcdefghij".repeat(5);
  for (index, character) in text.char_indices() {
    // No run until parked: every write is still pending while typing on.
    shell.window(cx, |window, cx| window.input(&character.to_string(), cx));
    let typed = &text[..=index];
    assert_eq!(input.read_with(cx, |input, _| input.value().to_string()), typed);
    assert_eq!(profile(&shell, cx).audio_languages_for_track_selection, typed);
    if index % 10 == 9 {
      // Some frames in between, as a user typing would see.
      shell.window(cx, |window, cx| window.render_frame(cx));
      assert_eq!(input.read_with(cx, |input, _| input.value().to_string()), typed);
    }
  }
  cx.run_until_parked();
  draw(&shell, cx);
  assert_eq!(input.read_with(cx, |input, _| input.value().to_string()), text);
  let written = shell.written.lock().unwrap();
  assert!(written.len() > writes_before);
  assert_eq!(
    written.last().unwrap().profiles[0].audio_languages_for_track_selection,
    text
  );
}

#[gpui_kit::test]
fn the_language_picker_adds_removes_and_reorders(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Profiles");
  let view = settings(&shell, cx);
  let picker = view.read_with(cx, |view, _| view.picker().clone());
  let filter = picker.read_with(cx, |picker, _| picker.filter().clone());
  let preferred = |cx: &mut TestAppContext| json_of(&shell, cx)["profiles"][0]["preferredAudioLanguages"].clone();
  assert_eq!(preferred(cx), json!("eng, chi, spa, ger, fre, jpn"));
  let row = |shell: &Shell, cx: &mut TestAppContext, pane: &str, code: &str| {
    draw(shell, cx);
    shell.window(cx, |window, _| {
      window
        .find(SharedString::from(format!("language-{pane}-{code}")))
        .bounds()
        .center()
    })
  };
  // Drag Korean from Available onto Chinese: it goes before it.
  type_into(&shell, cx, &filter, "korean");
  let (from, to) = (row(&shell, cx, "available", "kor"), row(&shell, cx, "preferred", "chi"));
  assert_writes(
    &shell,
    cx,
    "drag in",
    |shell, cx| shell.window(cx, |window, cx| window.drag(from, to, cx)),
    &[(
      "/profiles/0/preferredAudioLanguages",
      json!("eng, kor, chi, spa, ger, fre, jpn"),
    )],
  );
  // Two checked rows go together, at the end below the last row.
  type_into(&shell, cx, &filter, "it");
  click_label(&shell, cx, "Italian (it; ita)");
  click_label(&shell, cx, "Lithuanian (lt; lit)");
  let from = row(&shell, cx, "available", "ita");
  let below = shell.window(cx, |window, _| {
    let list = window.find("language-preferred").bounds();
    point(list.center().x, list.bottom() - px(10.0))
  });
  assert_writes(
    &shell,
    cx,
    "drag a checked set",
    |shell, cx| shell.window(cx, |window, cx| window.drag(from, below, cx)),
    &[(
      "/profiles/0/preferredAudioLanguages",
      json!("eng, kor, chi, spa, ger, fre, jpn, ita, lit"),
    )],
  );
  assert!(picker.read_with(cx, |picker, _| picker.checked(Pane::Available).is_empty()));
  // Within Preferred: Japanese onto English moves it first.
  let (from, to) = (row(&shell, cx, "preferred", "jpn"), row(&shell, cx, "preferred", "eng"));
  assert_writes(
    &shell,
    cx,
    "reorder",
    |shell, cx| shell.window(cx, |window, cx| window.drag(from, to, cx)),
    &[(
      "/profiles/0/preferredAudioLanguages",
      json!("jpn, eng, kor, chi, spa, ger, fre, ita, lit"),
    )],
  );
  // From Preferred onto Available removes.
  let from = row(&shell, cx, "preferred", "kor");
  let to = shell.window(cx, |window, _| window.find("language-available").bounds().center());
  assert_writes(
    &shell,
    cx,
    "drag out",
    |shell, cx| shell.window(cx, |window, cx| window.drag(from, to, cx)),
    &[(
      "/profiles/0/preferredAudioLanguages",
      json!("jpn, eng, chi, spa, ger, fre, ita, lit"),
    )],
  );
  // Add All adds what the filter shows and is not there yet; Remove All
  // empties the list.
  type_into(&shell, cx, &filter, "swedish");
  assert_writes(
    &shell,
    cx,
    "add all",
    |shell, cx| click_id(shell, cx, "language-add-all"),
    &[(
      "/profiles/0/preferredAudioLanguages",
      json!("jpn, eng, chi, spa, ger, fre, ita, lit, swe"),
    )],
  );
  assert_writes(
    &shell,
    cx,
    "remove all",
    |shell, cx| click_id(shell, cx, "language-remove-all"),
    &[("/profiles/0/preferredAudioLanguages", json!(""))],
  );
  // The toggle shows the subtitle list, with a fresh filter.
  click_id(&shell, cx, "language-kind-subtitles");
  assert_eq!(picker.read_with(cx, |picker, _| picker.kind()), StreamKind::Subtitles);
  assert_eq!(filter.read_with(cx, |input, _| input.value().to_string()), "");
  draw(&shell, cx);
  shell.window(cx, |window, _| {
    window.find("language-preferred-chi");
  });
}

#[test]
fn drops_insert_remove_and_reorder_languages() {
  let list = |codes: &str| -> Vec<String> { parse_language_codes(codes) };
  let drag = |from: Pane, codes: &str| LanguageDrag {
    from,
    codes: list(codes),
  };
  let preferred = list("eng, chi, spa, jpn");
  // From Available: before the target, existing ones skipped, else nothing.
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Available, "kor, eng"), Some(1)),
    Some(list("eng, kor, chi, spa, jpn"))
  );
  assert_eq!(drop_on_preferred(&preferred, &drag(Pane::Available, "eng"), None), None);
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Available, "fre"), Some(99)),
    Some(list("eng, chi, spa, jpn, fre"))
  );
  // Within Preferred: before the target; onto a dragged row or in place,
  // nothing.
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Preferred, "spa, jpn"), Some(0)),
    Some(list("spa, jpn, eng, chi"))
  );
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Preferred, "eng"), Some(3)),
    Some(list("chi, spa, eng, jpn"))
  );
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Preferred, "chi, spa"), Some(1)),
    None
  );
  assert_eq!(drop_on_preferred(&preferred, &drag(Pane::Preferred, "jpn"), None), None);
  assert_eq!(
    drop_on_preferred(&preferred, &drag(Pane::Preferred, "chi"), None),
    Some(list("eng, spa, jpn, chi"))
  );
  // Onto Available: a drag from Preferred removes.
  assert_eq!(
    drop_on_available(&preferred, &drag(Pane::Preferred, "chi, jpn")),
    Some(list("eng, spa"))
  );
  assert_eq!(drop_on_available(&preferred, &drag(Pane::Available, "chi")), None);
}

#[gpui_kit::test]
fn track_names_are_edited_per_language_and_kind(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Profiles");
  let view = settings(&shell, cx);
  let editor = view.read_with(cx, |view, _| view.track_names().clone());
  let names = editor.read_with(cx, |editor, _| editor.names().clone());
  // The preferred languages come first; English has names.
  click_id(&shell, cx, "track-names-eng");
  assert_eq!(
    editor.read_with(cx, |editor, _| editor.selected().map(str::to_string)),
    Some("eng".into())
  );
  assert_eq!(names.read_with(cx, |names, _| names.value().to_string()), "English");
  // ↓ moves to the next language, which has its own names.
  shell.press(cx, "down");
  assert_eq!(
    editor.read_with(cx, |editor, _| editor.selected().map(str::to_string)),
    Some("chi".into())
  );
  assert_eq!(
    names.read_with(cx, |names, _| names.value().to_string()),
    "Mandarin\nCantonese"
  );
  shell.press(cx, "up");
  assert_eq!(
    editor.read_with(cx, |editor, _| editor.selected().map(str::to_string)),
    Some("eng".into())
  );
  // Typing stores the names; an empty text removes the language.
  let names_state = names.clone();
  assert_writes(
    &shell,
    cx,
    "names",
    move |shell, cx| {
      let names = names_state.clone();
      shell.window(cx, |window, cx| names.update(cx, |names, cx| names.focus(window, cx)));
      shell.window(cx, |window, cx| {
        window.press(SELECT_ALL, cx);
        window.input("English", cx);
        window.press("enter", cx);
        window.input("Commentary", cx);
      });
    },
    &[("/profiles/0/trackNamesAudio/eng", json!("English\nCommentary"))],
  );
  let names_state = names.clone();
  assert_writes(
    &shell,
    cx,
    "no names",
    move |shell, cx| {
      shell.window(cx, |window, cx| {
        names_state.update(cx, |names, cx| names.focus(window, cx));
        window.press(SELECT_ALL, cx);
        window.press("backspace", cx);
      });
    },
    &[("/profiles/0/trackNamesAudio/eng", Value::Null)],
  );
  // The toggle shows the subtitle presets.
  click_id(&shell, cx, "track-names-kind-subtitles");
  click_id(&shell, cx, "track-names-chi");
  assert_eq!(
    names.read_with(cx, |names, _| names.value().to_string()),
    "Simplified Chinese\nTraditional Chinese"
  );
  // The filter narrows the list.
  let filter = editor.read_with(cx, |editor, _| editor.filter().clone());
  type_into(&shell, cx, &filter, "korean");
  assert_eq!(
    editor.read_with(cx, |editor, cx| editor.codes(cx)),
    (vec!["kor".to_string()], 0)
  );
}

#[gpui_kit::test]
fn tool_paths_are_detected_after_typing_stored_on_blur_and_found_elsewhere(cx: &mut TestAppContext) {
  let (tools, calls) = detector();
  let shell = open_with(cx, config(), tools);
  page(&shell, cx, "Integration");
  cx.executor().advance_clock(Duration::from_millis(300));
  draw(&shell, cx);
  let view = settings(&shell, cx);
  let row = view.read_with(cx, |view, _| view.tool_row(Tool::MkvToolNix).clone());
  let input = row.read_with(cx, |row, _| row.input().clone());
  // The stored default path was checked once it showed.
  assert_eq!(row.read_with(cx, |row, _| row.found()), Some(false));
  assert!(
    texts(&shell, cx)
      .iter()
      .any(|text| text == "mkvmerge not found under MKVToolNix path.")
  );
  // Typing: checked 250 ms after the last key, not stored yet.
  calls.lock().unwrap().clear();
  type_into(&shell, cx, &input, " /opt/mkvtoolnix ");
  cx.executor().advance_clock(Duration::from_millis(100));
  cx.run_until_parked();
  assert!(calls.lock().unwrap().is_empty(), "debounced");
  cx.executor().advance_clock(Duration::from_millis(200));
  draw(&shell, cx);
  assert_eq!(
    calls.lock().unwrap().as_slice(),
    [(Tool::MkvToolNix, "/opt/mkvtoolnix".to_string(), false)]
  );
  assert_eq!(row.read_with(cx, |row, _| row.found()), Some(true));
  assert!(
    texts(&shell, cx)
      .iter()
      .any(|text| text == "mkvmerge found under MKVToolNix path.")
  );
  assert_eq!(
    json_of(&shell, cx)["externalTools"]["mkvToolNixPath"],
    json!(Tool::MkvToolNix.default_path())
  );
  // Leaving the field stores the trimmed text.
  let other = view.read_with(cx, |view, cx| {
    view.tool_row(Tool::BetterMediaInfo).read(cx).input().clone()
  });
  assert_writes(
    &shell,
    cx,
    "blur",
    |shell, cx| focus(shell, cx, &other),
    &[("/externalTools/mkvToolNixPath", json!("/opt/mkvtoolnix"))],
  );
  // A path where the tool is found elsewhere fills in that path.
  type_into(&shell, cx, &input, "/opt/apps");
  assert_writes(
    &shell,
    cx,
    "found elsewhere",
    |_, cx| {
      cx.executor().advance_clock(Duration::from_millis(300));
      cx.run_until_parked();
    },
    &[],
  );
  assert_eq!(
    input.read_with(cx, |input, _| input.value().to_string()),
    "/opt/mkvtoolnix"
  );
  // Detect looks for a running application at once.
  calls.lock().unwrap().clear();
  assert_writes(
    &shell,
    cx,
    "detect",
    |shell, cx| click_id(shell, cx, "settings-mkvtoolnix-detect"),
    &[("/externalTools/mkvToolNixPath", json!("/run/mkvtoolnix"))],
  );
  assert_eq!(
    calls.lock().unwrap().as_slice(),
    [(Tool::MkvToolNix, "/opt/mkvtoolnix".to_string(), true)]
  );
  assert_eq!(
    input.read_with(cx, |input, _| input.value().to_string()),
    "/run/mkvtoolnix"
  );
  // Browse stores the chosen folder.
  assert_writes(
    &shell,
    cx,
    "browse",
    |shell, cx| {
      click_id(shell, cx, "settings-better-media-info-browse");
      assert!(cx.did_prompt_for_paths());
      cx.simulate_path_prompt_response(|_| Some(vec![std::path::PathBuf::from("/opt/bmi")]));
      cx.executor().advance_clock(Duration::from_millis(300));
      draw(shell, cx);
    },
    &[("/externalTools/betterMediaInfoPath", json!("/opt/bmi"))],
  );
  let bmi = view.read_with(cx, |view, _| view.tool_row(Tool::BetterMediaInfo).clone());
  assert_eq!(bmi.read_with(cx, |row, _| row.found()), Some(true));
  // Reset All puts both paths back.
  assert_writes(
    &shell,
    cx,
    "reset all",
    |shell, cx| {
      click_id(shell, cx, "reset");
      cx.executor().advance_clock(Duration::from_millis(300));
      draw(shell, cx);
    },
    &[
      ("/externalTools/betterMediaInfoPath", json!("")),
      ("/externalTools/mkvToolNixPath", json!(Tool::MkvToolNix.default_path())),
    ],
  );
}

#[gpui_kit::test]
fn the_settings_keep_their_page_while_hidden_and_start_over_when_reopened(cx: &mut TestAppContext) {
  let shell = open(cx);
  page(&shell, cx, "Update");
  assert!(shell.shows(cx, "Check New Version: Weekly"));
  shell.press(cx, "ctrl-1");
  assert!(!shell.shows(cx, "Check New Version: Weekly"));
  shell.press(cx, "f10");
  assert!(
    shell.shows(cx, "Check New Version: Weekly"),
    "the page survived the tab switch"
  );
  shell.press(cx, "ctrl-w");
  assert!(
    shell
      .view
      .read_with(cx, |view, cx| view.main().read(cx).settings().is_none())
  );
  shell.press(cx, "f10");
  assert!(
    !shell.shows(cx, "Check New Version: Weekly"),
    "a reopened tab starts on Appearance"
  );
}

/// How far below the top of `row` the element `id` starts.
fn offset_in_row(shell: &Shell, cx: &mut TestAppContext, row: &'static str, id: &'static str) -> Pixels {
  draw(shell, cx);
  shell.window(cx, |window, _| {
    window.find(id).bounds().top() - window.find(row).bounds().top()
  })
}

#[gpui_kit::test]
fn rows_with_buttons_wrap_them_below_the_field_when_narrow(cx: &mut TestAppContext) {
  let shell = open_with(
    cx,
    Config {
      language: Language::De,
      ..Config::default()
    },
    detector().0,
  );
  let rows = [
    ("Integration", "settings-mkvtoolnix", "settings-mkvtoolnix-browse"),
    ("Profiles", "settings-profile-row", "settings-profile-add"),
  ];
  for (title, row, button) in rows {
    page(&shell, cx, title);
    assert!(
      offset_in_row(&shell, cx, row, button) < px(10.0),
      "{button} beside the field when wide"
    );
  }
  // Narrower than the field's minimum and the buttons (the test platform's
  // text is narrower than a real font's, so not the 600 px minimum window);
  // tall enough for every group.
  shell.resize(cx, 480.0, 3200.0);
  for (title, row, button) in rows {
    page(&shell, cx, title);
    assert!(
      offset_in_row(&shell, cx, row, button) > px(24.0),
      "{button} below the field when narrow"
    );
  }
}
