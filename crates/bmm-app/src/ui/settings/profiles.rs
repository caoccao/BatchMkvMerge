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

//! The Profiles page: the active profile with Add, Delete and Reset (both
//! asking first), then four groups: Track Selection, Languages (the
//! [`LanguagePicker`]), Track Names (the [`TrackNamesEditor`]) and Automation.
//! Every control edits the active profile at once. The page has no Reset All;
//! its Reset puts the active profile back to its template.

use std::rc::Rc;

use bmm_core::config::ConfigProfile;
use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::setting::{RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{Disableable, Icon, Sizable, WindowExt, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::language_picker::LanguagePicker;
use super::language_select::LanguageChoices;
use super::track_names::TrackNamesEditor;
use super::{BUTTON_GROUP_GAP, FIELD_MIN_WIDTH, StreamKind, checkbox_description, checkbox_item};
use crate::state::config::active_profile;
use crate::state::{ConfigStore, Stores};
use crate::ui::{confirm_button, text_button};

/// What the page last put into its fields, to follow changes made
/// elsewhere (another profile, Reset) without overwriting typing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Synced {
  name: String,
  filters: [String; 3],
  und_language: String,
  options_key: [String; 3],
}

impl Synced {
  fn of(profile: Option<&ConfigProfile>) -> Self {
    let Some(profile) = profile else {
      return Self::default();
    };
    Self {
      name: profile.name.clone(),
      filters: StreamKind::ALL.map(|kind| kind.selection_languages(profile).to_string()),
      und_language: profile.automation.reset_und_language.language.clone(),
      options_key: StreamKind::ALL.map(|kind| kind.preferred_languages(profile).to_string()),
    }
  }
}

pub struct ProfileEditor {
  stores: Stores,
  new_name: Entity<InputState>,
  filters: [Entity<InputState>; 3],
  und_language: Entity<SelectState<LanguageChoices>>,
  synced: Synced,
  _subscriptions: Vec<Subscription>,
}

/// Edits the active profile.
fn edit_profile(store: &Entity<ConfigStore>, cx: &mut App, edit: impl FnOnce(&mut ConfigProfile)) {
  store.update(cx, |store, cx| store.update_active_profile(cx, edit));
}

/// Reads the active profile.
fn read_profile<R: Default>(store: &Entity<ConfigStore>, cx: &App, read: impl FnOnce(&ConfigProfile) -> R) -> R {
  active_profile(store.read(cx).config()).map(read).unwrap_or_default()
}

impl ProfileEditor {
  pub fn new(stores: Stores, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let profile = active_profile(stores.config.read(cx).config()).cloned();
    let synced = Synced::of(profile.as_ref());
    let new_name = cx.new(|cx| InputState::new(window, cx).placeholder(tr!("settings.newProfileName")));
    let filters = StreamKind::ALL.map(|kind| {
      let value = synced.filters[kind as usize].clone();
      cx.new(|cx| InputState::new(window, cx).default_value(value))
    });
    let choices = LanguageChoices::new(profile.as_ref());
    let und = SharedString::from(synced.und_language.clone());
    let und_language = cx.new(|cx| {
      let mut state = SelectState::new(choices, None, window, cx).searchable(true);
      state.set_selected_value(&und, window, cx);
      state
    });
    let mut subscriptions = vec![
      cx.subscribe(&new_name, |_, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          cx.notify();
        }
      }),
      cx.subscribe(&und_language, |this, _, event: &SelectEvent<LanguageChoices>, cx| {
        let SelectEvent::Confirm(Some(code)) = event else {
          return;
        };
        let code = code.to_string();
        this.synced.und_language = code.clone();
        edit_profile(&this.stores.config, cx, |profile| {
          profile.automation.reset_und_language.language = code
        });
      }),
      cx.observe_in(&stores.config, window, |this, _, window, cx| this.sync(window, cx)),
    ];
    for kind in StreamKind::ALL {
      subscriptions.push(
        cx.subscribe(&filters[kind as usize], move |this, input, event: &InputEvent, cx| {
          if matches!(event, InputEvent::Change) {
            let value = input.read(cx).value().to_string();
            this.synced.filters[kind as usize] = value.clone();
            edit_profile(&this.stores.config, cx, |profile| {
              kind.set_selection_languages(profile, value)
            });
          }
        }),
      );
    }
    Self {
      stores,
      new_name,
      filters,
      und_language,
      synced,
      _subscriptions: subscriptions,
    }
  }

  pub fn new_name(&self) -> &Entity<InputState> {
    &self.new_name
  }

  pub fn filter(&self, kind: StreamKind) -> &Entity<InputState> {
    &self.filters[kind as usize]
  }

  pub fn und_language(&self) -> &Entity<SelectState<LanguageChoices>> {
    &self.und_language
  }

  /// Puts the active profile's values into the fields that are not being
  /// edited, and the select's options when the preferred lists changed.
  fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let profile = active_profile(self.stores.config.read(cx).config()).cloned();
    let next = Synced::of(profile.as_ref());
    if next == self.synced {
      return;
    }
    for kind in StreamKind::ALL {
      let (index, input) = (kind as usize, self.filters[kind as usize].clone());
      let value = next.filters[index].clone();
      let focused = input.read(cx).focus_handle(cx).is_focused(window);
      if !focused && input.read(cx).value() != value.as_str() {
        input.update(cx, |input, cx| input.set_value(value, window, cx));
      }
    }
    if next.options_key != self.synced.options_key || next.name != self.synced.name {
      let choices = LanguageChoices::new(profile.as_ref());
      self
        .und_language
        .update(cx, |state, cx| state.set_items(choices, window, cx));
    }
    if next.und_language != self.synced.und_language
      || next.options_key != self.synced.options_key
      || next.name != self.synced.name
    {
      let code = SharedString::from(next.und_language.clone());
      self
        .und_language
        .update(cx, |state, cx| state.set_selected_value(&code, window, cx));
    }
    self.synced = next;
    cx.notify();
  }

  fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let name = self.new_name.read(cx).value().to_string();
    let added = self.stores.config.update(cx, |store, cx| store.add_profile(&name, cx));
    if added {
      self.new_name.update(cx, |input, cx| input.set_value("", window, cx));
      cx.notify();
    }
  }

  /// The new-name field with Add, Delete and Reset.
  fn name_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
    let store = self.stores.config.read(cx);
    let can_add = store.can_add_profile(&self.new_name.read(cx).value());
    let can_delete = store.can_delete_active_profile();
    let name = store.config().active_profile.clone();
    let config = self.stores.config.clone();
    // The buttons move below the name field when it would get narrower than
    // FIELD_MIN_WIDTH.
    let buttons = h_flex()
      .flex_none()
      .gap(BUTTON_GROUP_GAP)
      .child(
        text_button("settings-profile-add")
          .outline()
          .small()
          .label(tr!("settings.addProfile"))
          .disabled(!can_add)
          .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
      )
      .child(
        text_button("settings-profile-delete")
          .outline()
          .danger()
          .small()
          .label(tr!("settings.deleteProfile"))
          .disabled(!can_delete)
          .on_click({
            let (config, name) = (config.clone(), name.clone());
            move |_, window, cx| {
              let config = config.clone();
              confirm(
                window,
                cx,
                "settings-profile-delete",
                tr!("confirm.deleteProfileTitle"),
                tr!("confirm.deleteProfileMessage", name = name),
                move |cx| config.update(cx, |store, cx| store.delete_active_profile(cx)),
              );
            }
          }),
      )
      .child(
        text_button("settings-profile-reset")
          .outline()
          .small()
          .label(tr!("settings.reset"))
          .on_click(move |_, window, cx| {
            let config = config.clone();
            confirm(
              window,
              cx,
              "settings-profile-reset",
              tr!("confirm.resetProfileTitle"),
              tr!("confirm.resetProfileMessage", name = name),
              move |cx| config.update(cx, |store, cx| store.reset_active_profile(cx)),
            );
          }),
      );
    h_flex()
      .id("settings-profile-row")
      .test_support()
      .w_full()
      .flex_wrap()
      .gap(px(8.0))
      .child(
        div()
          .flex_1()
          .min_w(px(FIELD_MIN_WIDTH))
          .child(Input::new(&self.new_name).small().cleanable(true).w_full()),
      )
      .child(buttons)
  }

  /// A Track Selection row with an auto-select checkbox and a language filter.
  fn selection_row(&self, kind: StreamKind, options: &RenderOptions, cx: &App) -> impl IntoElement + use<> {
    let store = self.stores.config.clone();
    let checked = read_profile(&store, cx, |profile| selects(profile, kind));
    checkbox_field(
      options.layout(),
      px(220.0),
      Checkbox::new(SharedString::from(format!("settings-select-{}", kind.name())))
        .with_size(options.size())
        .disabled(options.is_disabled())
        .label(kind.label())
        .child(checkbox_description(
          tr!("settings.onlyAutoSelectOnDropForLanguages"),
          cx,
        ))
        .checked(checked)
        .on_change(move |checked, _, cx| edit_profile(&store, cx, |profile| set_selects(profile, kind, *checked))),
      Input::new(&self.filters[kind as usize])
        .small()
        .disabled(options.is_disabled())
        .cleanable(true)
        .w_full(),
    )
  }

  /// The und-language row: the box and the language.
  fn und_row(&self, options: &RenderOptions, cx: &App) -> impl IntoElement + use<> {
    let store = self.stores.config.clone();
    let enabled = read_profile(&store, cx, |profile| profile.automation.reset_und_language.enabled);
    checkbox_field(
      options.layout(),
      px(260.0),
      Checkbox::new("settings-automation-und")
        .with_size(options.size())
        .disabled(options.is_disabled())
        .label(tr!("settings.automationResetUndLanguage"))
        .checked(enabled)
        .on_change(move |checked, _, cx| {
          edit_profile(&store, cx, |profile| {
            profile.automation.reset_und_language.enabled = *checked
          });
        }),
      Select::new(&self.und_language)
        .id("settings-automation-und-language")
        .small()
        .disabled(options.is_disabled())
        .menu_width(px(320.0))
        .w_full(),
    )
  }
}

fn checkbox_field(layout: Axis, field_width: Pixels, checkbox: Checkbox, field: impl IntoElement) -> impl IntoElement {
  let stacked = layout == Axis::Vertical;
  h_flex()
    .w_full()
    .gap_3()
    .when(stacked, |row| row.flex_col().items_start())
    .child(checkbox.when(!stacked, |checkbox| checkbox.flex_1().min_w_0()))
    .child(div().flex_none().w(field_width).max_w_full().child(field))
}

/// A Yes / No question before `on_yes`; Enter and Escape answer No, as in
/// the exit dialog.
fn confirm(
  window: &mut Window,
  cx: &mut App,
  id: &'static str,
  title: SharedString,
  message: SharedString,
  on_yes: impl Fn(&mut App) + 'static,
) {
  let on_yes = Rc::new(on_yes);
  window.open_alert_dialog(cx, move |dialog, _, _| {
    let on_yes = on_yes.clone();
    dialog
      .title(title.clone())
      .description(message.clone())
      .on_ok(|_, _, _| true)
      .on_cancel(|_, _, _| true)
      .footer(
        DialogFooter::new()
          .child(
            text_button(SharedString::from(format!("{id}-no")))
              .outline()
              .label(tr!("confirm.no"))
              .on_click(|_, window, cx| window.close_dialog(cx)),
          )
          .child(
            confirm_button(SharedString::from(format!("{id}-yes")))
              .danger()
              .label(tr!("confirm.yes"))
              .on_click(move |_, window, cx| {
                on_yes(cx);
                window.close_dialog(cx);
              }),
          ),
      )
  });
}

/// Whether the profile auto-selects tracks of `kind` (`select*`).
fn selects(profile: &ConfigProfile, kind: StreamKind) -> bool {
  match kind {
    StreamKind::Video => profile.select_video,
    StreamKind::Audio => profile.select_audio,
    StreamKind::Subtitles => profile.select_subtitle,
  }
}

fn set_selects(profile: &mut ConfigProfile, kind: StreamKind, value: bool) {
  match kind {
    StreamKind::Video => profile.select_video = value,
    StreamKind::Audio => profile.select_audio = value,
    StreamKind::Subtitles => profile.select_subtitle = value,
  }
}

/// A plain checkbox item of the active profile.
fn profile_checkbox(
  store: &Entity<ConfigStore>,
  id: &'static str,
  title: SharedString,
  description: Option<SharedString>,
  get: fn(&ConfigProfile) -> bool,
  set: fn(&mut ConfigProfile, bool),
) -> SettingItem {
  let (get_store, set_store) = (store.clone(), store.clone());
  checkbox_item(
    id,
    title,
    description,
    move |cx| read_profile(&get_store, cx, get),
    move |checked, cx| edit_profile(&set_store, cx, |profile| set(profile, checked)),
  )
}

pub fn page(
  stores: &Stores,
  editor: &Entity<ProfileEditor>,
  picker: &Entity<LanguagePicker>,
  track_names: &Entity<TrackNamesEditor>,
  cx: &App,
) -> SettingPage {
  let store = stores.config.clone();
  let config = store.read(cx).config();
  let profiles: Vec<(SharedString, SharedString)> = config
    .profiles
    .iter()
    .map(|profile| (profile.name.clone().into(), profile.name.clone().into()))
    .collect();
  let (active_get, active_set) = (store.clone(), store.clone());
  let row_editor = editor.clone();
  let mut selection: Vec<SettingItem> = StreamKind::ALL
    .iter()
    .map(|&kind| {
      let editor = editor.clone();
      SettingItem::render(move |options, _, cx| editor.update(cx, |editor, cx| editor.selection_row(kind, options, cx)))
        .keywords([kind.label(), tr!("settings.onlyAutoSelectOnDropForLanguages")])
    })
    .collect();
  selection.push(profile_checkbox(
    &store,
    "settings-select-chapters",
    tr!("settings.chapters"),
    Some(tr!("settings.autoSelectOnDrop")),
    |profile| profile.select_chapters,
    |profile, value| profile.select_chapters = value,
  ));
  selection.push(profile_checkbox(
    &store,
    "settings-select-attachments",
    tr!("settings.attachments"),
    Some(tr!("settings.autoSelectOnDrop")),
    |profile| profile.select_attachments,
    |profile, value| profile.select_attachments = value,
  ));
  let und_editor = editor.clone();
  let (picker, track_names) = (picker.clone(), track_names.clone());
  SettingPage::new(tr!("settings.profiles"))
    .icon(Icon::new(IconName::User))
    .resettable(false)
    .groups([
      SettingGroup::new().items(vec![
        SettingItem::new(
          tr!("settings.activeProfile"),
          SettingField::dropdown(
            profiles,
            move |cx| active_get.read(cx).config().active_profile.clone().into(),
            move |name, cx| active_set.update(cx, |store, cx| store.set_active_profile(&name, cx)),
          ),
        ),
        SettingItem::render(move |_, _, cx| row_editor.update(cx, |editor, cx| editor.name_row(cx))).keywords([
          tr!("settings.newProfileName"),
          tr!("settings.addProfile"),
          tr!("settings.reset"),
        ]),
      ]),
      SettingGroup::new()
        .title(tr!("settings.trackSelection"))
        .items(selection),
      SettingGroup::new().title(tr!("settings.languages")).item(
        SettingItem::render(move |_, _, _| picker.clone()).keywords([
          tr!("settings.languages"),
          tr!("settings.available"),
          tr!("settings.preferred"),
        ]),
      ),
      SettingGroup::new()
        .title(tr!("settings.trackNames"))
        .item(SettingItem::render(move |_, _, _| track_names.clone()).keywords([tr!("settings.trackNames")])),
      SettingGroup::new().title(tr!("settings.automation")).items(vec![
        SettingItem::render(move |options, _, cx| und_editor.update(cx, |editor, cx| editor.und_row(options, cx)))
          .keywords([tr!("settings.automationResetUndLanguage")]),
        profile_checkbox(
          &store,
          "settings-automation-track-name",
          tr!("settings.automationSetTrackName"),
          None,
          |profile| profile.automation.set_track_name.enabled,
          |profile, value| profile.automation.set_track_name.enabled = value,
        ),
        profile_checkbox(
          &store,
          "settings-automation-default-track",
          tr!("settings.automationResetDefaultTrack"),
          None,
          |profile| profile.automation.reset_default_track.enabled,
          |profile, value| profile.automation.reset_default_track.enabled = value,
        ),
        profile_checkbox(
          &store,
          "settings-automation-forced-display",
          tr!("settings.automationResetForcedDisplay"),
          None,
          |profile| profile.automation.reset_forced_display.enabled,
          |profile, value| profile.automation.reset_forced_display.enabled = value,
        ),
      ]),
    ])
}
