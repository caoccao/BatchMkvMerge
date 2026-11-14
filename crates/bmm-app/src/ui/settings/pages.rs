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

//! The four simple pages: Appearance (mode, theme, language), Formatting
//! (bit rate and size precision and unit per stream kind), Group (group mode
//! and group by file name) and Update (check interval).

use bmm_core::config::{
  Config, ConfigFormatField, ConfigStreamFormat, DisplayMode, FormatPrecision, FormatUnit, GroupMode, Language, Theme,
  UpdateCheckInterval,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme, Icon, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::*;

use super::{StreamKind, dropdown};
use crate::state::{ConfigStore, Stores};
use crate::ui::toolbar::{GROUP_MODES, group_mode_label_key};

/// The twenty themes in the order of the theme select (`getThemes`).
pub const THEMES: [Theme; 20] = [
  Theme::Ocean,
  Theme::Aqua,
  Theme::Sky,
  Theme::Arctic,
  Theme::Glacier,
  Theme::Mist,
  Theme::Slate,
  Theme::Charcoal,
  Theme::Midnight,
  Theme::Indigo,
  Theme::Violet,
  Theme::Lavender,
  Theme::Rose,
  Theme::Blush,
  Theme::Coral,
  Theme::Sunset,
  Theme::Amber,
  Theme::Sand,
  Theme::Forest,
  Theme::Emerald,
];

/// The nine languages with their config codes and own names
/// (`getLanguages`, `getLanguageLabel`).
pub const LANGUAGES: [(Language, &str, &str); 9] = [
  (Language::De, "de", "Deutsch"),
  (Language::EnUS, "en-US", "English (US)"),
  (Language::Es, "es", "Español"),
  (Language::Fr, "fr", "Français"),
  (Language::It, "it", "Italiano"),
  (Language::Ja, "ja", "日本語"),
  (Language::ZhCN, "zh-CN", "简体中文"),
  (Language::ZhHK, "zh-HK", "繁體中文 (香港)"),
  (Language::ZhTW, "zh-TW", "繁體中文 (台灣)"),
];

pub const PRECISIONS: [(FormatPrecision, &str); 3] = [
  (FormatPrecision::Zero, "#"),
  (FormatPrecision::One, "#.#"),
  (FormatPrecision::Two, "#.##"),
];

pub const UNITS: [(FormatUnit, &str); 7] = [
  (FormatUnit::K, "k"),
  (FormatUnit::KM, "k/M"),
  (FormatUnit::KMG, "k/M/G"),
  (FormatUnit::KMGT, "k/M/G/T"),
  (FormatUnit::KMi, "k/Mi"),
  (FormatUnit::KMiGi, "k/Mi/Gi"),
  (FormatUnit::KMiGiTi, "k/Mi/Gi/Ti"),
];

pub const INTERVALS: [(UpdateCheckInterval, &str); 3] = [
  (UpdateCheckInterval::Daily, "settings.daily"),
  (UpdateCheckInterval::Weekly, "settings.weekly"),
  (UpdateCheckInterval::Monthly, "settings.monthly"),
];

/// The name of a unit variant as the config writes it (`Ocean`, `KMGT`):
/// for these enums the variant's own name.
pub fn wire_name<T: std::fmt::Debug>(value: &T) -> SharedString {
  format!("{value:?}").into()
}

/// The variant of `options` called `name`.
pub fn from_wire_name<T: std::fmt::Debug + Clone>(options: impl IntoIterator<Item = T>, name: &str) -> Option<T> {
  options.into_iter().find(|option| wire_name(option) == name)
}

/// The config code of a language (`de`, `en-US`).
pub fn language_code(language: Language) -> &'static str {
  LANGUAGES
    .iter()
    .find(|(other, ..)| *other == language)
    .map_or("en-US", |(_, code, _)| code)
}

/// Reads the config for a field's value.
fn read<R>(store: &Entity<ConfigStore>, cx: &App, f: impl FnOnce(&Config) -> R) -> R {
  f(store.read(cx).config())
}

/// Writes one edit to the config.
fn write(store: &Entity<ConfigStore>, cx: &mut App, edit: impl FnOnce(&mut Config)) {
  store.update(cx, |store, cx| store.update(cx, edit));
}

pub fn appearance(stores: &Stores, cx: &App) -> SettingPage {
  let store = stores.config.clone();
  let mode_store = store.clone();
  let reset_store = store.clone();
  let (theme_get, theme_set) = (store.clone(), store.clone());
  let (language_get, language_set) = (store.clone(), store);
  let default_language = SharedString::from(language_code(Config::default().language));
  let palette_icon = Icon::new(IconName::Palette);
  let _ = cx;
  SettingPage::new(tr!("settings.appearance"))
    .icon(palette_icon)
    .group(SettingGroup::new().items(vec![
      SettingItem::new(
        tr!("settings.mode"),
        SettingField::render(move |_, _, cx| mode_toggle(&mode_store, cx)).on_reset(
          {
            let store = reset_store.clone();
            move |cx| read(&store, cx, |config| config.display_mode != DisplayMode::Auto)
          },
          move |_, cx| write(&reset_store, cx, |config| config.display_mode = DisplayMode::Auto),
        ),
      ),
      SettingItem::new(
        tr!("settings.theme"),
        SettingField::scrollable_dropdown(
          THEMES
            .iter()
            .map(|theme| {
              let name = wire_name(theme);
              let key = format!("settings.theme{name}");
              (name, tr!(&key))
            })
            .collect(),
          move |cx| read(&theme_get, cx, |config| wire_name(&config.theme)),
          move |name, cx| {
            if let Some(theme) = from_wire_name(THEMES, &name) {
              write(&theme_set, cx, |config| config.theme = theme);
            }
          },
        )
        .default_value(wire_name(&Theme::default())),
      ),
      SettingItem::new(
        tr!("settings.language"),
        SettingField::dropdown(
          LANGUAGES
            .iter()
            .map(|(_, code, label)| (SharedString::from(*code), SharedString::from(*label)))
            .collect(),
          move |cx| read(&language_get, cx, |config| language_code(config.language).into()),
          move |code, cx| {
            if let Some((language, ..)) = LANGUAGES.iter().find(|(_, other, _)| *other == code.as_ref()) {
              let language = *language;
              write(&language_set, cx, |config| config.language = language);
            }
          },
        )
        .default_value(default_language),
      ),
    ]))
}

/// The mode toggle group (Auto, Light, Dark with their icons).
fn mode_toggle(store: &Entity<ConfigStore>, cx: &App) -> impl IntoElement + use<> {
  let current = read(store, cx, |config| config.display_mode);
  let modes = [
    (DisplayMode::Auto, IconName::SunMoon, "settings.autoMode", "auto"),
    (DisplayMode::Light, IconName::Sun, "settings.lightMode", "light"),
    (DisplayMode::Dark, IconName::Moon, "settings.darkMode", "dark"),
  ];
  let store = store.clone();
  ButtonGroup::new("settings-mode")
    .outline()
    .small()
    .children(modes.iter().map(|(mode, icon, key, name)| {
      Button::new(SharedString::from(format!("settings-mode-{name}")))
        .icon(Icon::new(*icon))
        .label(tr!(*key))
        .selected(*mode == current)
    }))
    .on_click(move |clicks: &Vec<usize>, _, cx| {
      if let Some((mode, ..)) = clicks.first().and_then(|index| modes.get(*index)) {
        let mode = *mode;
        write(&store, cx, |config| config.display_mode = mode);
      }
    })
}

/// The formatting of one stream kind.
fn stream(config: &Config, kind: StreamKind) -> &ConfigStreamFormat {
  match kind {
    StreamKind::Video => &config.formatting.video,
    StreamKind::Audio => &config.formatting.audio,
    StreamKind::Subtitles => &config.formatting.subtitle,
  }
}

fn stream_mut(config: &mut Config, kind: StreamKind) -> &mut ConfigStreamFormat {
  match kind {
    StreamKind::Video => &mut config.formatting.video,
    StreamKind::Audio => &mut config.formatting.audio,
    StreamKind::Subtitles => &mut config.formatting.subtitle,
  }
}

/// Bit rate or size of a stream format.
#[derive(Clone, Copy)]
enum FormatTarget {
  BitRate,
  Size,
}

impl FormatTarget {
  fn name(self) -> &'static str {
    match self {
      Self::BitRate => "bit-rate",
      Self::Size => "size",
    }
  }

  fn title(self) -> SharedString {
    match self {
      Self::BitRate => tr!("settings.bitRate"),
      Self::Size => tr!("settings.size"),
    }
  }

  fn get(self, format: &ConfigStreamFormat) -> &ConfigFormatField {
    match self {
      Self::BitRate => &format.bit_rate,
      Self::Size => &format.size,
    }
  }

  fn get_mut(self, format: &mut ConfigStreamFormat) -> &mut ConfigFormatField {
    match self {
      Self::BitRate => &mut format.bit_rate,
      Self::Size => &mut format.size,
    }
  }
}

/// The precision and unit selects of one field (`FormatFieldControls`),
/// each with its caption.
fn format_controls(
  store: &Entity<ConfigStore>,
  kind: StreamKind,
  target: FormatTarget,
  cx: &App,
) -> impl IntoElement + use<> {
  let field = read(store, cx, |config| target.get(stream(config, kind)).clone());
  let id = format!("settings-format-{}-{}", kind.name(), target.name());
  let muted = cx.theme().muted_foreground;
  let caption = |key: &str| div().text_xs().text_color(muted).child(tr!(key));
  let (precision_store, unit_store) = (store.clone(), store.clone());
  h_flex()
    .gap(px(16.0))
    .child(
      v_flex()
        .gap(px(4.0))
        .child(caption("settings.precision"))
        .child(dropdown(
          SharedString::from(format!("{id}-precision")),
          format!("{} {}", target.title(), tr!("settings.precision")).into(),
          PRECISIONS
            .iter()
            .map(|(precision, label)| (wire_name(precision), SharedString::from(*label)))
            .collect(),
          wire_name(&field.precision),
          move |name, cx| {
            if let Some(precision) = from_wire_name(PRECISIONS.map(|(precision, _)| precision), &name) {
              write(&precision_store, cx, |config| {
                target.get_mut(stream_mut(config, kind)).precision = precision
              });
            }
          },
        )),
    )
    .child(
      v_flex().gap(px(4.0)).child(caption("settings.unit")).child(dropdown(
        SharedString::from(format!("{id}-unit")),
        format!("{} {}", target.title(), tr!("settings.unit")).into(),
        UNITS
          .iter()
          .map(|(unit, label)| (wire_name(unit), SharedString::from(*label)))
          .collect(),
        wire_name(&field.unit),
        move |name, cx| {
          if let Some(unit) = from_wire_name(UNITS.map(|(unit, _)| unit), &name) {
            write(&unit_store, cx, |config| {
              target.get_mut(stream_mut(config, kind)).unit = unit
            });
          }
        },
      )),
    )
}

fn format_item(store: &Entity<ConfigStore>, kind: StreamKind, target: FormatTarget) -> SettingItem {
  let (render_store, dirty_store, reset_store) = (store.clone(), store.clone(), store.clone());
  SettingItem::new(
    target.title(),
    SettingField::render(move |_, _, cx| format_controls(&render_store, kind, target, cx)).on_reset(
      move |cx| {
        read(&dirty_store, cx, |config| {
          let field = target.get(stream(config, kind));
          let default = ConfigFormatField::default();
          field.precision != default.precision || field.unit != default.unit
        })
      },
      move |_, cx| {
        write(&reset_store, cx, |config| {
          *target.get_mut(stream_mut(config, kind)) = ConfigFormatField::default()
        })
      },
    ),
  )
}

pub fn formatting(stores: &Stores) -> SettingPage {
  SettingPage::new(tr!("settings.formatting"))
    .icon(Icon::new(IconName::SlidersHorizontal))
    .groups(StreamKind::ALL.map(|kind| {
      SettingGroup::new()
        .title(format!("{} {}", kind.emoji(), kind.label()))
        .items(vec![
          format_item(&stores.config, kind, FormatTarget::BitRate),
          format_item(&stores.config, kind, FormatTarget::Size),
        ])
    }))
}

pub fn group(stores: &Stores) -> SettingPage {
  let store = stores.config.clone();
  let (radio_store, dirty_store, reset_store) = (store.clone(), store.clone(), store.clone());
  let (by_name_get, by_name_set) = (store.clone(), store);
  SettingPage::new(tr!("groupMode.title"))
    .icon(Icon::new(IconName::Images))
    .group(SettingGroup::new().items(vec![
      SettingItem::render(move |_, _, cx| {
        let current = read(&radio_store, cx, |config| config.group_mode.clone());
        let store = radio_store.clone();
        RadioGroup::vertical("settings-group-mode")
          // gpui-kit names the radios by their index.
          .children(GROUP_MODES.iter().map(|mode| Radio::new(wire_name(mode)).label(tr!(group_mode_label_key(mode)))))
          .selected_index(GROUP_MODES.iter().position(|mode| *mode == current))
          .on_click(move |index: &usize, _, cx| {
            if let Some(mode) = GROUP_MODES.get(*index).cloned() {
              write(&store, cx, |config| config.group_mode = mode);
            }
          })
      })
      .keywords([tr!("groupMode.none"), tr!("groupMode.trackCount"), tr!("groupMode.trackCountAndLanguage")])
      .on_reset(
        move |cx| read(&dirty_store, cx, |config| config.group_mode != GroupMode::default()),
        move |_, cx| write(&reset_store, cx, |config| config.group_mode = GroupMode::default()),
      ),
      SettingItem::new(
        tr!("groupMode.byFileName"),
        SettingField::checkbox(
          move |cx| read(&by_name_get, cx, |config| config.group_by_file_name),
          move |checked, cx| write(&by_name_set, cx, |config| config.group_by_file_name = checked),
        )
        .default_value(true),
      ),
    ]))
}

pub fn update(stores: &Stores) -> SettingPage {
  let (get, set) = (stores.config.clone(), stores.config.clone());
  SettingPage::new(tr!("settings.update"))
    .icon(Icon::new(IconName::RefreshCw))
    .group(
      SettingGroup::new().item(SettingItem::new(
        tr!("settings.checkNewVersion"),
        SettingField::dropdown(
          INTERVALS
            .iter()
            .map(|(interval, key)| (wire_name(interval), tr!(*key)))
            .collect(),
          move |cx| read(&get, cx, |config| wire_name(&config.update.check_interval)),
          move |name, cx| {
            if let Some(interval) = from_wire_name(INTERVALS.map(|(interval, _)| interval), &name) {
              write(&set, cx, |config| config.update.check_interval = interval);
            }
          },
        )
        .default_value(wire_name(&UpdateCheckInterval::default())),
      )),
    )
}
