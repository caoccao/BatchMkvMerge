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

use anyhow::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::constants::APP_NAME;

static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
  #[serde(rename = "displayMode", default)]
  pub display_mode: DisplayMode,
  #[serde(default)]
  pub theme: Theme,
  #[serde(default)]
  pub language: Language,
  #[serde(rename = "externalTools", default)]
  pub external_tools: ConfigExternalTools,
  #[serde(default = "Config::default_profiles")]
  pub profiles: Vec<ConfigProfile>,
  #[serde(rename = "activeProfile", default = "Config::default_active_profile")]
  pub active_profile: String,
  #[serde(default)]
  pub window: ConfigWindow,
  #[serde(default)]
  pub update: ConfigUpdate,
  #[serde(default)]
  pub parser: ConfigParser,
}

impl Config {
  fn default_profiles() -> Vec<ConfigProfile> {
    vec![ConfigProfile::default()]
  }

  fn default_active_profile() -> String {
    ConfigProfile::DEFAULT_NAME.to_owned()
  }
}

impl Default for Config {
  fn default() -> Self {
    Self {
      display_mode: Default::default(),
      theme: Default::default(),
      language: Default::default(),
      external_tools: Default::default(),
      profiles: Self::default_profiles(),
      active_profile: Self::default_active_profile(),
      window: Default::default(),
      update: Default::default(),
      parser: Default::default(),
    }
  }
}

/// User-tunable parser settings.  See [[feedback-parser-timeout]].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigParser {
  /// Per-file parse budget in milliseconds.  Default 10000.  Clamped to the
  /// supported range when read.  Not exposed in the UI — persisted in config only.
  #[serde(rename = "timeoutMs", default = "ConfigParser::default_timeout_ms")]
  pub timeout_ms: u64,
  /// Fallback charset used to decode BOM-less SRT / SSA / USF / MicroDVD
  /// subtitle files.  Empty string means "auto" (the parser keeps using
  /// UTF-8 lossy decode).  Mirrors mkvtoolnix's `--sub-charset` knob.
  /// PARSER-089.
  #[serde(rename = "subtitleCharset", default)]
  pub subtitle_charset: String,
}

impl ConfigParser {
  /// Default parser timeout in ms.
  pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
  /// Minimum allowed timeout — small enough for ridiculous test runs but
  /// large enough that a real file's headers can be read.
  pub const MIN_TIMEOUT_MS: u64 = 100;
  /// Maximum allowed timeout — 60 s; anything beyond this should be a
  /// streaming / partial parse, not header probing.
  pub const MAX_TIMEOUT_MS: u64 = 60_000;

  pub fn default_timeout_ms() -> u64 {
    Self::DEFAULT_TIMEOUT_MS
  }

  /// Clamp `timeout_ms` to the supported range.  Returns the effective value
  /// callers should hand to the parser.
  pub fn effective_timeout_ms(&self) -> u64 {
    self.timeout_ms.clamp(Self::MIN_TIMEOUT_MS, Self::MAX_TIMEOUT_MS)
  }
}

impl Default for ConfigParser {
  fn default() -> Self {
    Self {
      timeout_ms: Self::DEFAULT_TIMEOUT_MS,
      subtitle_charset: String::new(),
    }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigUpdate {
  #[serde(rename = "checkInterval", default)]
  pub check_interval: UpdateCheckInterval,
  #[serde(rename = "lastChecked", default)]
  pub last_checked: i64,
  #[serde(rename = "lastVersion", default)]
  pub last_version: String,
  #[serde(rename = "ignoreVersion", default)]
  pub ignore_version: String,
}

impl Default for ConfigUpdate {
  fn default() -> Self {
    Self {
      check_interval: Default::default(),
      last_checked: 0,
      last_version: String::new(),
      ignore_version: String::new(),
    }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum UpdateCheckInterval {
  Daily,
  Weekly,
  Monthly,
}

impl Default for UpdateCheckInterval {
  fn default() -> Self {
    Self::Weekly
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigProfile {
  pub name: String,
  #[serde(rename = "selectVideo", default)]
  pub select_video: bool,
  #[serde(rename = "selectAudio", default)]
  pub select_audio: bool,
  #[serde(rename = "selectSubtitle", default = "ConfigProfile::default_select_subtitle")]
  pub select_subtitle: bool,
  #[serde(rename = "selectChapters", default)]
  pub select_chapters: bool,
  #[serde(rename = "selectAttachments", default)]
  pub select_attachments: bool,
  #[serde(rename = "videoLanguages", default)]
  pub video_languages: String,
  #[serde(rename = "audioLanguages", default)]
  pub audio_languages: String,
  #[serde(rename = "subtitleLanguages", default = "ConfigProfile::default_subtitle_languages")]
  pub subtitle_languages: String,
  #[serde(rename = "defaultGroupMode", default = "ConfigProfile::default_default_group_mode")]
  pub default_group_mode: bool,
}

impl ConfigProfile {
  pub const DEFAULT_NAME: &'static str = "Default";

  fn default_select_subtitle() -> bool {
    true
  }

  pub const DEFAULT_SUBTITLE_LANGUAGES: &'static str = "eng, chi, spa, ger, fre, jpn";

  fn default_subtitle_languages() -> String {
    Self::DEFAULT_SUBTITLE_LANGUAGES.to_owned()
  }

  fn default_default_group_mode() -> bool {
    true
  }
}

impl Default for ConfigProfile {
  fn default() -> Self {
    Self {
      name: Self::DEFAULT_NAME.to_owned(),
      select_video: false,
      select_audio: false,
      select_subtitle: true,
      select_chapters: false,
      select_attachments: false,
      video_languages: String::new(),
      audio_languages: String::new(),
      subtitle_languages: Self::default_subtitle_languages(),
      default_group_mode: true,
    }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigExternalTools {
  #[serde(rename = "mkvToolNixPath", default = "ConfigExternalTools::default_mkv_toolnix_path")]
  pub mkv_toolnix_path: String,
  #[serde(rename = "betterMediaInfoPath", default)]
  pub better_media_info_path: String,
}

impl ConfigExternalTools {
  fn default_mkv_toolnix_path() -> String {
    if cfg!(target_os = "windows") {
      r"C:\Program Files\MKVToolNix".to_owned()
    } else if cfg!(target_os = "macos") {
      "/Applications/MKVToolNix.app/Contents/MacOS".to_owned()
    } else {
      "/usr/bin".to_owned()
    }
  }
}

impl Default for ConfigExternalTools {
  fn default() -> Self {
    Self {
      mkv_toolnix_path: Self::default_mkv_toolnix_path(),
      better_media_info_path: String::new(),
    }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum DisplayMode {
  Auto,
  Light,
  Dark,
}

impl Default for DisplayMode {
  fn default() -> Self {
    Self::Auto
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Theme {
  #[serde(alias = "Default")]
  Ocean,
  Aqua,
  Sky,
  Arctic,
  Glacier,
  Mist,
  Slate,
  Charcoal,
  Midnight,
  Indigo,
  Violet,
  Lavender,
  Rose,
  Blush,
  Coral,
  Sunset,
  Amber,
  Sand,
  Forest,
  Emerald,
}

impl Default for Theme {
  fn default() -> Self {
    Self::Ocean
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Language {
  #[serde(rename = "de")]
  De,
  #[serde(rename = "en-US")]
  EnUS,
  #[serde(rename = "es")]
  Es,
  #[serde(rename = "fr")]
  Fr,
  #[serde(rename = "it")]
  It,
  #[serde(rename = "ja")]
  Ja,
  #[serde(rename = "zh-CN")]
  ZhCN,
  #[serde(rename = "zh-HK")]
  ZhHK,
  #[serde(rename = "zh-TW")]
  ZhTW,
}

impl Default for Language {
  fn default() -> Self {
    Self::EnUS
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigWindow {
  #[serde(default)]
  pub position: ConfigWindowPosition,
  #[serde(default)]
  pub size: ConfigWindowSize,
}

impl Default for ConfigWindow {
  fn default() -> Self {
    Self {
      position: Default::default(),
      size: Default::default(),
    }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigWindowPosition {
  pub x: i32,
  pub y: i32,
}

impl Default for ConfigWindowPosition {
  fn default() -> Self {
    Self { x: -1, y: -1 }
  }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigWindowSize {
  pub width: u32,
  pub height: u32,
}

impl Default for ConfigWindowSize {
  fn default() -> Self {
    Self {
      width: 1200,
      height: 900,
    }
  }
}

impl Config {
  fn new() -> Self {
    let config_path_buf = Self::get_path_buf();
    if config_path_buf.exists() {
      Self::load(config_path_buf)
    } else {
      log::debug!("Loading default config.");
      let config = Self::default();
      if let Err(err) = config.save(config_path_buf) {
        log::error!("Couldn't save the default config because {}", err);
      }
      config
    }
  }

  fn get_path_buf() -> PathBuf {
    let config_dir = Self::get_config_dir();
    if !config_dir.exists() {
      if let Err(err) = std::fs::create_dir_all(&config_dir) {
        log::warn!("Couldn't create config dir {}: {}", config_dir.display(), err);
      }
    }
    config_dir.join(format!("{}.json", APP_NAME))
  }

  fn get_exe_dir() -> PathBuf {
    std::env::current_exe().unwrap().parent().unwrap().to_path_buf()
  }

  #[cfg(target_os = "linux")]
  fn get_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
      if !xdg.is_empty() {
        return PathBuf::from(xdg).join(APP_NAME);
      }
    }
    if let Ok(home) = std::env::var("HOME") {
      return PathBuf::from(home).join(".config").join(APP_NAME);
    }
    Self::get_exe_dir()
  }

  #[cfg(target_os = "macos")]
  fn get_config_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
      return PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join(APP_NAME);
    }
    Self::get_exe_dir()
  }

  #[cfg(target_os = "windows")]
  fn get_config_dir() -> PathBuf {
    let exe_dir = Self::get_exe_dir();
    let exe_path_lc = exe_dir.to_string_lossy().to_ascii_lowercase();
    let starts_with_env = |env_var: &str| -> bool {
      std::env::var(env_var)
        .ok()
        .map(|p| !p.is_empty() && exe_path_lc.starts_with(&p.to_ascii_lowercase()))
        .unwrap_or(false)
    };
    let is_installed =
      starts_with_env("LOCALAPPDATA") || starts_with_env("ProgramFiles") || starts_with_env("ProgramFiles(x86)");
    if is_installed {
      if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
          return PathBuf::from(appdata).join(APP_NAME);
        }
      }
    }
    exe_dir
  }

  fn load(path: PathBuf) -> Self {
    let cloned_path = path.clone();
    let path_string = cloned_path.to_str().unwrap();
    log::debug!("Loading config from {}.", path_string);
    let file = File::open(path).expect(format!("Couldn't open config file {}.", path_string).as_str());
    let buf_reader = BufReader::new(file);
    serde_json::from_reader(buf_reader).expect(format!("Couldn't parse config file {}.", path_string).as_str())
  }

  fn save(&self, path: PathBuf) -> Result<()> {
    let cloned_path = path.clone();
    let path_string = cloned_path.to_str().unwrap();
    log::debug!("Saving config to {}.", path_string);
    let file = File::create(path).expect(format!("Couldn't create config file {}.", path_string).as_str());
    let buf_writer = BufWriter::new(file);
    serde_json::to_writer_pretty(buf_writer, &self).map_err(Error::msg)
  }
}

pub fn get_config() -> Config {
  CONFIG
    .get_or_init(|| RwLock::new(Config::new()))
    .read()
    .unwrap()
    .clone()
}

pub fn set_config(config: Config) -> Result<()> {
  let config_path_buf = Config::get_path_buf();
  let result = config.save(config_path_buf);
  CONFIG
    .get_or_init(|| RwLock::new(Config::new()))
    .write()
    .unwrap()
    .clone_from(&config);
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parser_defaults_to_ten_seconds() {
    let p = ConfigParser::default();
    assert_eq!(p.timeout_ms, 10_000);
    assert_eq!(p.effective_timeout_ms(), 10_000);
  }

  #[test]
  fn parser_deserializes_from_json_with_camel_case() {
    let json = r#"{"timeoutMs": 2500}"#;
    let p: ConfigParser = serde_json::from_str(json).unwrap();
    assert_eq!(p.timeout_ms, 2500);
  }

  #[test]
  fn parser_deserializes_missing_field_via_serde_default() {
    // Older configs lacked the timeoutMs field entirely.
    let json = "{}";
    let p: ConfigParser = serde_json::from_str(json).unwrap();
    assert_eq!(p.timeout_ms, ConfigParser::DEFAULT_TIMEOUT_MS);
  }

  #[test]
  fn parser_clamps_too_small() {
    let p = ConfigParser {
      timeout_ms: 0,
      subtitle_charset: String::new(),
    };
    assert_eq!(p.effective_timeout_ms(), ConfigParser::MIN_TIMEOUT_MS);
    let p = ConfigParser {
      timeout_ms: 50,
      subtitle_charset: String::new(),
    };
    assert_eq!(p.effective_timeout_ms(), ConfigParser::MIN_TIMEOUT_MS);
  }

  #[test]
  fn parser_clamps_too_large() {
    let p = ConfigParser {
      timeout_ms: 999_999,
      subtitle_charset: String::new(),
    };
    assert_eq!(p.effective_timeout_ms(), ConfigParser::MAX_TIMEOUT_MS);
  }

  #[test]
  fn parser_passes_through_in_range_values() {
    for ms in [100u64, 500, 1000, 2000, 30_000, 60_000] {
      let p = ConfigParser {
        timeout_ms: ms,
        subtitle_charset: String::new(),
      };
      assert_eq!(p.effective_timeout_ms(), ms);
    }
  }

  #[test]
  fn config_default_contains_parser_block() {
    let cfg = Config::default();
    assert_eq!(cfg.parser.timeout_ms, 10_000);
  }

  #[test]
  fn config_deserializes_legacy_payload_without_parser_block() {
    // A persisted config from before Phase 2 lacked the `parser` key.
    let json = r#"{
            "displayMode": "Auto",
            "theme": "Ocean",
            "language": "en-US",
            "externalTools": { "mkvToolNixPath": "", "betterMediaInfoPath": "" },
            "profiles": [],
            "activeProfile": "Default",
            "window": {
                "position": { "x": -1, "y": -1 },
                "size": { "width": 1200, "height": 900 }
            },
            "update": {
                "checkInterval": "Weekly",
                "lastChecked": 0,
                "lastVersion": "",
                "ignoreVersion": ""
            }
        }"#;
    let cfg: Config = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.parser.timeout_ms, ConfigParser::DEFAULT_TIMEOUT_MS);
  }

  #[test]
  fn config_round_trips_with_parser_block() {
    let mut cfg = Config::default();
    cfg.parser.timeout_ms = 2500;
    let json = serde_json::to_string(&cfg).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(back.parser.timeout_ms, 2500);
  }
}
