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

//! Colour themes and light/dark mode.
//!
//! Each configured theme is a gpui-kit theme set under `themes/` with a light
//! and a dark variant that set the primary, ring and link tokens from the
//! React palette (`App.tsx::getPaletteByTheme`) and keep every other token at
//! gpui-kit's defaults. Only Ocean ships so far; the other nineteen follow and
//! fall back to it until then.

use bmm_core::config::{Config, DisplayMode, Theme as ThemeName};
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, WindowAppearance};

/// The embedded theme sets, one per configured theme.
const THEME_SETS: [&str; 1] = [include_str!("../themes/ocean.json")];

/// Registers every embedded theme set. Call once after `gpui_kit::init`.
pub fn install(cx: &mut App) {
  let registry = ThemeRegistry::global_mut(cx);
  for theme_set in THEME_SETS {
    if let Err(error) = registry.load_themes_from_str(theme_set) {
      log::error!("Failed to load a theme set: {error}");
    }
  }
}

/// Registry names of the light and dark variant of a configured theme. Every
/// theme maps to Ocean until its own theme set is embedded.
pub fn theme_names(_theme: ThemeName) -> (&'static str, &'static str) {
  ("Ocean Light", "Ocean Dark")
}

/// The mode a display mode shows under the given system appearance; Auto
/// follows the system, like `prefers-color-scheme`.
pub fn resolve_mode(display_mode: DisplayMode, appearance: WindowAppearance) -> ThemeMode {
  match display_mode {
    DisplayMode::Light => ThemeMode::Light,
    DisplayMode::Dark => ThemeMode::Dark,
    DisplayMode::Auto => match appearance {
      WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
      WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
    },
  }
}

/// Applies the configured theme and mode; `appearance` is the system's (the
/// window's once one exists). Refreshes every window.
pub fn apply(config: &Config, appearance: WindowAppearance, cx: &mut App) {
  let (light, dark) = theme_names(config.theme);
  let registry = ThemeRegistry::global(cx);
  let light = registry.themes().get(light).cloned();
  let dark = registry.themes().get(dark).cloned();
  match (light, dark) {
    (Some(light), Some(dark)) => {
      let theme = Theme::global_mut(cx);
      theme.light_theme = light;
      theme.dark_theme = dark;
    }
    _ => log::error!("Theme {:?} is not registered", config.theme),
  }
  Theme::change(resolve_mode(config.display_mode, appearance), None, cx);
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::component::ActiveTheme;
  use gpui_kit::{TestAppContext, px, rgb};

  fn config(theme: ThemeName, display_mode: DisplayMode) -> Config {
    Config {
      theme,
      display_mode,
      ..Config::default()
    }
  }

  #[test]
  fn auto_follows_the_system_appearance() {
    assert_eq!(resolve_mode(DisplayMode::Auto, WindowAppearance::Dark), ThemeMode::Dark);
    assert_eq!(
      resolve_mode(DisplayMode::Auto, WindowAppearance::VibrantDark),
      ThemeMode::Dark
    );
    assert_eq!(
      resolve_mode(DisplayMode::Auto, WindowAppearance::Light),
      ThemeMode::Light
    );
    assert_eq!(
      resolve_mode(DisplayMode::Light, WindowAppearance::Dark),
      ThemeMode::Light
    );
    assert_eq!(
      resolve_mode(DisplayMode::Dark, WindowAppearance::Light),
      ThemeMode::Dark
    );
  }

  #[gpui_kit::test]
  fn ocean_is_applied_in_both_modes(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      install(cx);
      for name in ["Ocean Light", "Ocean Dark"] {
        assert!(ThemeRegistry::global(cx).themes().contains_key(name), "{name}");
      }
      let primary: gpui_kit::Hsla = rgb(0x0288d1).into();

      apply(
        &config(ThemeName::Ocean, DisplayMode::Light),
        WindowAppearance::Dark,
        cx,
      );
      assert!(!cx.theme().is_dark());
      assert_eq!(cx.theme().primary, primary);
      assert_eq!(cx.theme().font_size, px(14.0));
      let light_background = cx.theme().background;

      apply(&config(ThemeName::Ocean, DisplayMode::Auto), WindowAppearance::Dark, cx);
      assert!(cx.theme().is_dark());
      assert_eq!(cx.theme().primary, primary);
      // Tokens the theme does not set keep gpui-kit's per-mode defaults.
      assert_ne!(cx.theme().background, light_background);
    });
  }

  #[gpui_kit::test]
  fn themes_without_a_set_yet_fall_back_to_ocean(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      install(cx);
      apply(
        &config(ThemeName::Rose, DisplayMode::Light),
        WindowAppearance::Light,
        cx,
      );
      assert_eq!(cx.theme().theme_name().as_ref(), "Ocean Light");
    });
  }
}
