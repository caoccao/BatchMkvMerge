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

//! The Settings tab (`Settings.tsx`) on gpui-kit's `Settings` component: a
//! searchable, resizable sidebar of six pages (Appearance, Formatting,
//! Group, Integration, Profiles, Update, in React's order) whose React inner
//! tabs became titled groups listed under their page. Every control writes
//! the same config field as its React control, at once (the config store
//! persists in the background and never reads back). Every page but Profiles
//! has gpui-kit's Reset All, back to `Config::default()`; Profiles keeps
//! React's Reset of the active profile.
//!
//! The view lives while the tab is open; its children keep the state React
//! kept in the component (the text fields, the picker's selection, the track
//! name language).

pub mod integration;
pub mod language_picker;
pub mod language_select;
mod pages;
pub mod profiles;
pub mod track_names;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;

use bmm_core::config::ConfigProfile;
use bmm_core::media_track::MediaTrackType;
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::setting::Settings;
use gpui_kit::component::{Selectable, Sizable, h_flex};
use gpui_kit::*;

use self::integration::ToolPathRow;
use self::language_picker::LanguagePicker;
use self::profiles::ProfileEditor;
use self::track_names::TrackNamesEditor;
use crate::domain::format::track_type_emoji;
use crate::state::Stores;
use crate::tools::Tool;

/// The stream kinds with languages of their own (`LanguageTrackType` and
/// the Formatting tabs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamKind {
  Video,
  Audio,
  Subtitles,
}

impl StreamKind {
  pub const ALL: [Self; 3] = [Self::Video, Self::Audio, Self::Subtitles];

  /// A stable name for element ids.
  pub fn name(self) -> &'static str {
    match self {
      Self::Video => "video",
      Self::Audio => "audio",
      Self::Subtitles => "subtitles",
    }
  }

  pub fn label(self) -> SharedString {
    match self {
      Self::Video => tr!("settings.video"),
      Self::Audio => tr!("settings.audio"),
      Self::Subtitles => tr!("settings.subtitles"),
    }
  }

  pub fn emoji(self) -> &'static str {
    track_type_emoji(match self {
      Self::Video => MediaTrackType::Video,
      Self::Audio => MediaTrackType::Audio,
      Self::Subtitles => MediaTrackType::Subtitles,
    })
  }

  /// The Languages tab's ordered list (`preferred*Languages`).
  pub fn preferred_languages(self, profile: &ConfigProfile) -> &str {
    match self {
      Self::Video => &profile.preferred_video_languages,
      Self::Audio => &profile.preferred_audio_languages,
      Self::Subtitles => &profile.preferred_subtitle_languages,
    }
  }

  pub fn set_preferred_languages(self, profile: &mut ConfigProfile, value: String) {
    match self {
      Self::Video => profile.preferred_video_languages = value,
      Self::Audio => profile.preferred_audio_languages = value,
      Self::Subtitles => profile.preferred_subtitle_languages = value,
    }
  }

  /// The Track Names tab's presets (`trackNames*`).
  pub fn track_names(self, profile: &ConfigProfile) -> &HashMap<String, String> {
    match self {
      Self::Video => &profile.track_names_video,
      Self::Audio => &profile.track_names_audio,
      Self::Subtitles => &profile.track_names_subtitle,
    }
  }

  pub fn track_names_mut(self, profile: &mut ConfigProfile) -> &mut HashMap<String, String> {
    match self {
      Self::Video => &mut profile.track_names_video,
      Self::Audio => &mut profile.track_names_audio,
      Self::Subtitles => &mut profile.track_names_subtitle,
    }
  }

  /// The Track Selection tab's filter (`*LanguagesForTrackSelection`).
  pub fn selection_languages(self, profile: &ConfigProfile) -> &str {
    match self {
      Self::Video => &profile.video_languages_for_track_selection,
      Self::Audio => &profile.audio_languages_for_track_selection,
      Self::Subtitles => &profile.subtitle_languages_for_track_selection,
    }
  }

  pub fn set_selection_languages(self, profile: &mut ConfigProfile, value: String) {
    match self {
      Self::Video => profile.video_languages_for_track_selection = value,
      Self::Audio => profile.audio_languages_for_track_selection = value,
      Self::Subtitles => profile.subtitle_languages_for_track_selection = value,
    }
  }
}

/// The narrowest text field of a row with buttons (a tool path, the new
/// profile name); below it the buttons wrap under the field, so long German
/// and French labels still fit at the 600 × 450 minimum window size.
pub const FIELD_MIN_WIDTH: f32 = 200.0;

/// A comma-separated language list as codes, in order (`parseLanguageCodes`).
pub fn parse_language_codes(list: &str) -> Vec<String> {
  list
    .split(',')
    .map(str::trim)
    .filter(|code| !code.is_empty())
    .map(str::to_string)
    .collect()
}

/// The Video / Audio / Subtitles toggle (`TrackTypeToggle`): `id` names the
/// group, each button is `{id}-{kind}`.
pub fn kind_toggle(
  id: &'static str,
  current: StreamKind,
  on_change: impl Fn(StreamKind, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
  let on_change = Rc::new(on_change);
  h_flex().id(id).children(StreamKind::ALL.map(|kind| {
    let on_change = on_change.clone();
    Button::new(SharedString::from(format!("{id}-{}", kind.name())))
      .outline()
      .small()
      .selected(kind == current)
      .label(format!("{} {}", kind.emoji(), kind.label()))
      .on_click(move |_, window, cx| on_change(kind, window, cx))
  }))
}

/// A choice of `options` (value, label) as gpui-kit's settings dropdown
/// draws it: an outlined button with the current label and a menu with the
/// current value checked. Its accessible name is `{title}: {label}`.
pub fn dropdown(
  id: impl Into<ElementId>,
  title: SharedString,
  options: Vec<(SharedString, SharedString)>,
  value: SharedString,
  on_select: impl Fn(SharedString, &mut App) + 'static,
) -> impl IntoElement {
  let on_select = Rc::new(on_select);
  let label = options
    .iter()
    .find(|(option, _)| *option == value)
    .map(|(_, label)| label.clone())
    .unwrap_or_else(|| value.clone());
  Button::new(id)
    .accessibility_label(format!("{title}: {label}"))
    .label(label)
    .dropdown_caret(true)
    .outline()
    .small()
    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
      options.iter().fold(menu, |menu, (option, label)| {
        let (option, on_select) = (option.clone(), on_select.clone());
        menu.item(
          PopupMenuItem::new(label.clone())
            .checked(option == value)
            .on_click(move |_, _, cx| on_select(option.clone(), cx)),
        )
      })
    })
}

pub struct SettingsView {
  stores: Stores,
  mkvtoolnix: Entity<ToolPathRow>,
  better_media_info: Entity<ToolPathRow>,
  profile: Entity<ProfileEditor>,
  picker: Entity<LanguagePicker>,
  track_names: Entity<TrackNamesEditor>,
  _subscriptions: Vec<Subscription>,
}

impl SettingsView {
  pub fn new(stores: Stores, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let profile = cx.new(|cx| ProfileEditor::new(stores.clone(), window, cx));
    // The profile editor's parts are drawn inside the settings, not as a view.
    let subscriptions = vec![
      cx.observe(&stores.config, |_, _, cx| cx.notify()),
      cx.observe(&profile, |_, _, cx| cx.notify()),
    ];
    Self {
      mkvtoolnix: cx.new(|cx| ToolPathRow::new(stores.clone(), Tool::MkvToolNix, window, cx)),
      better_media_info: cx.new(|cx| ToolPathRow::new(stores.clone(), Tool::BetterMediaInfo, window, cx)),
      profile,
      picker: cx.new(|cx| LanguagePicker::new(stores.clone(), window, cx)),
      track_names: cx.new(|cx| TrackNamesEditor::new(stores.clone(), window, cx)),
      stores,
      _subscriptions: subscriptions,
    }
  }

  pub fn tool_row(&self, tool: Tool) -> &Entity<ToolPathRow> {
    match tool {
      Tool::MkvToolNix => &self.mkvtoolnix,
      Tool::BetterMediaInfo => &self.better_media_info,
    }
  }

  pub fn profile(&self) -> &Entity<ProfileEditor> {
    &self.profile
  }

  pub fn picker(&self) -> &Entity<LanguagePicker> {
    &self.picker
  }

  pub fn track_names(&self) -> &Entity<TrackNamesEditor> {
    &self.track_names
  }
}

impl Render for SettingsView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let stores = &self.stores;
    div().id("settings").test_support().size_full().child(
      Settings::new("settings-pages").sidebar_width(px(200.0)).pages([
        pages::appearance(stores, cx),
        pages::formatting(stores),
        pages::group(stores),
        integration::page(stores, &self.mkvtoolnix, &self.better_media_info),
        profiles::page(stores, &self.profile, &self.picker, &self.track_names, cx),
        pages::update(stores),
      ]),
    )
  }
}
