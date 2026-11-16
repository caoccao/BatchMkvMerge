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

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::constants::APP_NAME;

static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();
static UNREADABLE_CONFIG: OnceLock<UnreadableConfig> = OnceLock::new();

/// How many backup names [`keep_aside`] tries before it gives up.
const MAX_BACKUPS: u32 = 100;
/// How many times a load reads the config file again when another instance
/// of the app replaced it meanwhile (see [`Config::reload_if_replaced`]).
const MAX_RELOADS: u32 = 3;

/// A config file that did not load as it was. The app started with every
/// value that could still be read from it and the defaults for the rest, and
/// the original was kept under a backup name so that no save overwrites it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableConfig {
  /// What was wrong with the file.
  pub reason: String,
  /// Where the original file was kept; `None` when it could not be moved.
  pub backup: Option<PathBuf>,
}

/// The problem with the config file read at startup, if it had one.
pub fn unreadable_config() -> Option<&'static UnreadableConfig> {
  UNREADABLE_CONFIG.get()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
  #[serde(rename = "displayMode", default)]
  pub display_mode: DisplayMode,
  #[serde(default)]
  pub theme: Theme,
  #[serde(default = "Language::detect_system")]
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
  #[serde(rename = "groupMode", default)]
  pub group_mode: GroupMode,
  #[serde(rename = "groupByFileName", default = "Config::default_group_by_file_name")]
  pub group_by_file_name: bool,
  #[serde(default)]
  pub formatting: ConfigFormatting,
}

impl Config {
  fn default_profiles() -> Vec<ConfigProfile> {
    vec![ConfigProfile::default()]
  }

  fn default_active_profile() -> String {
    ConfigProfile::DEFAULT_NAME.to_owned()
  }

  fn default_group_by_file_name() -> bool {
    true
  }
}

impl Default for Config {
  fn default() -> Self {
    Self {
      display_mode: Default::default(),
      theme: Default::default(),
      language: Language::detect_system(),
      external_tools: Default::default(),
      profiles: Self::default_profiles(),
      active_profile: Self::default_active_profile(),
      window: Default::default(),
      update: Default::default(),
      parser: Default::default(),
      group_mode: Default::default(),
      group_by_file_name: Self::default_group_by_file_name(),
      formatting: Default::default(),
    }
  }
}

/// Size / bit-rate display formatting, one [`ConfigStreamFormat`] per stream kind.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConfigFormatting {
  #[serde(default)]
  pub video: ConfigStreamFormat,
  #[serde(default)]
  pub audio: ConfigStreamFormat,
  #[serde(default)]
  pub subtitle: ConfigStreamFormat,
}

/// Per-stream-kind bit-rate + size formatting.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConfigStreamFormat {
  #[serde(rename = "bitRate", default)]
  pub bit_rate: ConfigFormatField,
  #[serde(default)]
  pub size: ConfigFormatField,
}

/// Precision + unit for a single formatted numeric field.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConfigFormatField {
  #[serde(default)]
  pub precision: FormatPrecision,
  #[serde(default)]
  pub unit: FormatUnit,
}

/// Number of fractional digits shown.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum FormatPrecision {
  Zero,
  One,
  #[default]
  Two,
}

/// Unit ladder: `K*` use 1024-step divisors, `K*i` use 1000-step divisors.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum FormatUnit {
  K,
  KM,
  KMG,
  #[default]
  KMGT,
  KMi,
  KMiGi,
  KMiGiTi,
}

/// How the file list buckets dropped files into group cards.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum GroupMode {
  /// Render every file on its own card.
  None,
  /// Group files that share the same track count per type.
  #[default]
  TrackCount,
  /// Group files that share the same track count *and* language per type.
  TrackCountAndLanguage,
}

/// User-tunable parser settings.  See [[feedback-parser-timeout]].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigParser {
  /// Per-file parse budget in milliseconds.  Default 10000.  Clamped to the
  /// supported range when read.  Not exposed in the UI — persisted in config only.
  #[serde(rename = "timeoutMs", default = "ConfigParser::default_timeout_ms")]
  pub timeout_ms: u64,
  /// Fallback charset used to decode BOM-less SRT / SSA / USF / MicroDVD
  /// subtitle files: a WHATWG encoding label (`windows-1252`, `latin1`,
  /// `sjis`, …). Empty string means "auto" (the parser keeps using UTF-8
  /// lossy decode), and so does a label that names no encoding. The merge
  /// passes the encoding the parser used to mkvmerge as `--sub-charset`, by
  /// its iconv name (`merge_args::mkvmerge_sub_charset`), for the subtitle
  /// tracks of SRT, SSA/ASS and MicroDVD files
  /// (`merge_args::takes_sub_charset`), so the output is read the way the
  /// table was. PARSER-089.
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

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum UpdateCheckInterval {
  Daily,
  #[default]
  Weekly,
  Monthly,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigProfile {
  pub name: String,
  #[serde(rename = "selectVideo", default)]
  pub select_video: bool,
  #[serde(rename = "selectAudio", default = "ConfigProfile::default_select_audio")]
  pub select_audio: bool,
  #[serde(rename = "selectSubtitle", default = "ConfigProfile::default_select_subtitle")]
  pub select_subtitle: bool,
  #[serde(rename = "selectChapters", default = "ConfigProfile::default_select_chapters")]
  pub select_chapters: bool,
  #[serde(rename = "selectAttachments", default)]
  pub select_attachments: bool,
  #[serde(rename = "videoLanguagesForTrackSelection", default)]
  pub video_languages_for_track_selection: String,
  #[serde(
    rename = "audioLanguagesForTrackSelection",
    default = "ConfigProfile::default_languages"
  )]
  pub audio_languages_for_track_selection: String,
  #[serde(
    rename = "subtitleLanguagesForTrackSelection",
    default = "ConfigProfile::default_languages"
  )]
  pub subtitle_languages_for_track_selection: String,
  #[serde(rename = "preferredVideoLanguages", default = "ConfigProfile::default_languages")]
  pub preferred_video_languages: String,
  #[serde(rename = "preferredAudioLanguages", default = "ConfigProfile::default_languages")]
  pub preferred_audio_languages: String,
  #[serde(rename = "preferredSubtitleLanguages", default = "ConfigProfile::default_languages")]
  pub preferred_subtitle_languages: String,
  #[serde(
    rename = "trackNamesVideo",
    default = "ConfigProfile::default_track_names_video_audio"
  )]
  pub track_names_video: HashMap<String, String>,
  #[serde(
    rename = "trackNamesAudio",
    default = "ConfigProfile::default_track_names_video_audio"
  )]
  pub track_names_audio: HashMap<String, String>,
  #[serde(
    rename = "trackNamesSubtitle",
    default = "ConfigProfile::default_track_names_subtitle"
  )]
  pub track_names_subtitle: HashMap<String, String>,
  #[serde(default)]
  pub automation: ConfigAutomation,
}

/// Per-profile automation toggles.  Snake_case keys on the wire (per spec).
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConfigAutomation {
  #[serde(default)]
  pub reset_und_language: AutomationResetUndLanguage,
  #[serde(default)]
  pub set_track_name: AutomationToggle,
  #[serde(default)]
  pub reset_default_track: AutomationToggle,
  #[serde(default)]
  pub reset_forced_display: AutomationToggle,
}

/// Replace a track's "und" language with `language` automatically.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AutomationResetUndLanguage {
  #[serde(default)]
  pub enabled: bool,
  #[serde(default = "AutomationResetUndLanguage::default_language")]
  pub language: String,
}

impl AutomationResetUndLanguage {
  fn default_language() -> String {
    "en".to_owned()
  }
}

impl Default for AutomationResetUndLanguage {
  fn default() -> Self {
    Self {
      enabled: false,
      language: Self::default_language(),
    }
  }
}

/// A simple on/off automation toggle (set track name, reset default track,
/// reset forced display).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AutomationToggle {
  #[serde(default)]
  pub enabled: bool,
}

impl ConfigProfile {
  pub const DEFAULT_NAME: &'static str = "Default";

  fn default_select_subtitle() -> bool {
    true
  }

  fn default_select_audio() -> bool {
    true
  }

  fn default_select_chapters() -> bool {
    true
  }

  pub const DEFAULT_LANGUAGES: &'static str = "eng, chi, spa, ger, fre, jpn";

  fn default_languages() -> String {
    Self::DEFAULT_LANGUAGES.to_owned()
  }

  fn default_track_names_video_audio() -> HashMap<String, String> {
    HashMap::from([
      ("eng".to_owned(), "English".to_owned()),
      ("chi".to_owned(), "Mandarin\nCantonese".to_owned()),
      ("spa".to_owned(), "Spanish".to_owned()),
      ("ger".to_owned(), "German".to_owned()),
      ("fre".to_owned(), "French".to_owned()),
      ("jpn".to_owned(), "Japanese".to_owned()),
    ])
  }

  fn default_track_names_subtitle() -> HashMap<String, String> {
    HashMap::from([
      ("eng".to_owned(), "English".to_owned()),
      ("chi".to_owned(), "Simplified Chinese\nTraditional Chinese".to_owned()),
      ("spa".to_owned(), "Spanish".to_owned()),
      ("ger".to_owned(), "German".to_owned()),
      ("fre".to_owned(), "French".to_owned()),
      ("jpn".to_owned(), "Japanese".to_owned()),
    ])
  }
}

impl Default for ConfigProfile {
  fn default() -> Self {
    Self {
      name: Self::DEFAULT_NAME.to_owned(),
      select_video: false,
      select_audio: true,
      select_subtitle: true,
      select_chapters: true,
      select_attachments: false,
      video_languages_for_track_selection: String::new(),
      audio_languages_for_track_selection: Self::default_languages(),
      subtitle_languages_for_track_selection: Self::default_languages(),
      preferred_video_languages: Self::default_languages(),
      preferred_audio_languages: Self::default_languages(),
      preferred_subtitle_languages: Self::default_languages(),
      track_names_video: Self::default_track_names_video_audio(),
      track_names_audio: Self::default_track_names_video_audio(),
      track_names_subtitle: Self::default_track_names_subtitle(),
      automation: ConfigAutomation::default(),
    }
  }
}

impl ConfigProfile {
  /// A fresh profile called `name`: the Default profile's values, except that
  /// a profile with another name auto-selects no audio, subtitle or chapter
  /// tracks.
  pub fn named(name: &str) -> Self {
    let is_default = name == Self::DEFAULT_NAME;
    Self {
      name: name.to_owned(),
      select_audio: is_default,
      select_subtitle: is_default,
      select_chapters: is_default,
      ..Self::default()
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum DisplayMode {
  #[default]
  Auto,
  Light,
  Dark,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Theme {
  #[default]
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Language {
  #[serde(rename = "de")]
  De,
  #[default]
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

impl Language {
  fn detect_system() -> Self {
    let locales: Vec<String> = sys_locale::get_locales().collect();
    for locale in &locales {
      if let Some(language) = Self::from_locale_tag(locale) {
        log::debug!("Detected system language {:?} from locale {}.", language, locale);
        return language;
      }
    }
    if !locales.is_empty() {
      log::debug!("No supported app language found in system locales {:?}.", locales);
    }
    Self::default()
  }

  fn from_locale_tag(locale: &str) -> Option<Self> {
    let normalized = normalize_locale_tag(locale)?;
    let mut parts = normalized.split('-');
    let language = parts.next()?;
    let mut script: Option<&str> = None;
    let mut region: Option<&str> = None;
    for part in parts {
      if part.len() == 4 && script.is_none() {
        script = Some(part);
      } else if (part.len() == 2 || part.len() == 3) && region.is_none() {
        region = Some(part);
      }
    }

    match language {
      "de" => Some(Self::De),
      "en" => Some(Self::EnUS),
      "es" => Some(Self::Es),
      "fr" => Some(Self::Fr),
      "it" => Some(Self::It),
      "ja" => Some(Self::Ja),
      "zh" => Some(match (script, region) {
        (Some("hant"), Some("hk" | "mo")) => Self::ZhHK,
        (Some("hant"), _) => Self::ZhTW,
        (Some("hans"), _) => Self::ZhCN,
        (_, Some("tw")) => Self::ZhTW,
        (_, Some("hk" | "mo")) => Self::ZhHK,
        _ => Self::ZhCN,
      }),
      _ => None,
    }
  }
}

fn normalize_locale_tag(locale: &str) -> Option<String> {
  let locale = locale
    .split(':')
    .find(|part| !part.trim().is_empty())
    .unwrap_or(locale)
    .trim();
  if locale.eq_ignore_ascii_case("c") || locale.eq_ignore_ascii_case("posix") {
    return None;
  }
  let locale = locale
    .split('.')
    .next()
    .unwrap_or(locale)
    .split('@')
    .next()
    .unwrap_or(locale)
    .replace('_', "-")
    .to_ascii_lowercase();
  if locale.is_empty() || locale == "c" || locale == "posix" {
    None
  } else {
    Some(locale)
  }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ConfigWindow {
  /// The display the window was on, as the platform names it (a display
  /// UUID); empty when unknown. Displays at different scales can overlap in
  /// logical coordinates, so the position alone may not say which one it is.
  #[serde(default)]
  pub display: String,
  #[serde(default)]
  pub position: ConfigWindowPosition,
  #[serde(default)]
  pub size: ConfigWindowSize,
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
      let (config, problem) = Self::load(config_path_buf);
      if let Some(problem) = problem {
        let _ = UNREADABLE_CONFIG.set(problem);
      }
      config
    } else {
      log::debug!("Loading default config.");
      let config = Self::default();
      if let Err(err) = config.save(&config_path_buf) {
        log::error!("Couldn't save the default config because {:#}", err);
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

  /// The directory of the executable; the current directory when the
  /// executable's path is unknown.
  fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
      .ok()
      .and_then(|exe| exe.parent().map(Path::to_path_buf))
      .or_else(|| std::env::current_dir().ok())
      .unwrap_or_else(|| PathBuf::from("."))
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
    let exe_dir_string = exe_dir.to_string_lossy();
    let is_installed = ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
      .iter()
      .filter_map(|name| std::env::var(name).ok())
      .any(|root| is_within_directory(&exe_dir_string, &root));
    if is_installed {
      if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
          return PathBuf::from(appdata).join(APP_NAME);
        }
      }
    }
    exe_dir
  }

  /// Reads the config file at `path`. A file that cannot be read or holds
  /// invalid values never stops the app: it is kept as `*.json.bad` (see
  /// [`keep_aside`]), the config salvaged from it is saved in its place, and
  /// the problem is returned for the app to show.
  fn load(path: PathBuf) -> (Self, Option<UnreadableConfig>) {
    log::debug!("Loading config from {}.", path.display());
    let read = std::fs::read(&path);
    Self::load_read(path, read)
  }

  /// [`Self::load`] with the result of reading the file. Another instance
  /// of the app may have replaced the file since it was read.
  fn load_read(path: PathBuf, read: std::io::Result<Vec<u8>>) -> (Self, Option<UnreadableConfig>) {
    Self::load_from(path, read, MAX_RELOADS)
  }

  /// [`Self::load_read`], which may read the file again up to `reloads`
  /// times.
  fn load_from(path: PathBuf, read: std::io::Result<Vec<u8>>, reloads: u32) -> (Self, Option<UnreadableConfig>) {
    let parsed = match &read {
      Ok(bytes) => Self::parse(bytes),
      Err(error) => Parsed::Salvaged(Self::default(), format!("it could not be read: {error}")),
    };
    match parsed {
      Parsed::Loaded(mut config, has_language) => {
        let normalized = config.normalize_profiles();
        if !has_language || normalized {
          if let Some(newer) = Self::reload_if_replaced(&path, &read, reloads) {
            return newer;
          }
          log::debug!("Saving the completed config {}.", path.display());
          if let Err(err) = config.save(&path) {
            log::error!("Couldn't save the completed config because {:#}", err);
          }
        }
        (config, None)
      }
      Parsed::Salvaged(mut config, reason) => {
        // A file that could not be read is moved aside, and the move takes
        // whatever is there: settings another instance saved since the read
        // would go to the backup. A file that can be read now is loaded as
        // it is instead.
        if read.is_err()
          && let Some(newer) = Self::reload_if_replaced(&path, &read, reloads)
        {
          return newer;
        }
        config.normalize_profiles();
        log::error!("Config file {} is unusable: {}.", path.display(), reason);
        let backup = match keep_aside(&path, read.as_deref().ok()) {
          Ok(backup) => {
            log::warn!("Kept the unusable config file as {}.", backup.display());
            if let Some(newer) = Self::reload_if_replaced(&path, &read, reloads) {
              return newer;
            }
            if let Err(err) = config.save(&path) {
              log::error!("Couldn't save the repaired config because {:#}", err);
            }
            Some(backup)
          }
          Err(err) => {
            log::error!("Couldn't keep the unusable config file {}: {}", path.display(), err);
            None
          }
        };
        (config, Some(UnreadableConfig { reason, backup }))
      }
    }
  }

  /// Another instance of the app may have saved the config file since this
  /// one read it: newer settings, or its own repair or completion of the
  /// same file. A save made from what this instance read would replace
  /// them, so a load reads the file again just before it saves, and an
  /// unreadable file before it is moved aside. When the file no longer holds
  /// what was read, or can be read after a read that failed, it is loaded
  /// again as it is now, and that load's result is returned. `None` when the
  /// file still holds what was read, when it is gone or cannot be read
  /// (nothing newer to keep), or when no reload is left.
  fn reload_if_replaced(
    path: &Path,
    read: &std::io::Result<Vec<u8>>,
    reloads: u32,
  ) -> Option<(Self, Option<UnreadableConfig>)> {
    if reloads == 0 {
      return None;
    }
    let now = std::fs::read(path).ok()?;
    if read.as_ref().is_ok_and(|bytes| *bytes == now) {
      return None;
    }
    log::warn!(
      "Config file {} was replaced while it was being loaded; loading it again.",
      path.display()
    );
    Some(Self::load_from(path.to_path_buf(), Ok(now), reloads - 1))
  }

  /// Makes the profile list what the app relies on; `true` when it changed.
  /// A hand-edited file can break it: a profile name used twice is made
  /// unique (`Anime (2)`), a missing `Default` profile is added first, and an
  /// `activeProfile` that names no profile becomes `Default`.
  fn normalize_profiles(&mut self) -> bool {
    let mut changed = false;
    // A new name avoids every name in the file, so no later profile's own
    // name is taken by a rename.
    let taken: Vec<String> = self.profiles.iter().map(|profile| profile.name.clone()).collect();
    let mut names: Vec<String> = Vec::new();
    for profile in &mut self.profiles {
      if names.contains(&profile.name) {
        let base = profile.name.clone();
        let unique = (2..)
          .map(|n| format!("{base} ({n})"))
          .find(|name| !names.contains(name) && !taken.contains(name))
          .expect("an unused name");
        log::warn!("Renamed the second profile called {base:?} to {unique:?}.");
        profile.name = unique;
        changed = true;
      }
      names.push(profile.name.clone());
    }
    if !names.iter().any(|name| name == ConfigProfile::DEFAULT_NAME) {
      log::warn!("Added the missing {:?} profile.", ConfigProfile::DEFAULT_NAME);
      self.profiles.insert(0, ConfigProfile::default());
      names.push(ConfigProfile::DEFAULT_NAME.to_owned());
      changed = true;
    }
    if !names.contains(&self.active_profile) {
      log::warn!(
        "The active profile {:?} does not exist; using {:?}.",
        self.active_profile,
        ConfigProfile::DEFAULT_NAME
      );
      self.active_profile = ConfigProfile::DEFAULT_NAME.to_owned();
      changed = true;
    }
    changed
  }

  /// Parses a config file's contents.
  fn parse(bytes: &[u8]) -> Parsed {
    let value: Value = match serde_json::from_slice(bytes) {
      Ok(value) => value,
      Err(error) => return Parsed::Salvaged(Self::default(), format!("it is not valid JSON ({error})")),
    };
    let has_language = value.as_object().is_some_and(|object| object.contains_key("language"));
    match serde_json::from_value::<Self>(value.clone()) {
      Ok(config) => Parsed::Loaded(config, has_language),
      Err(error) => Parsed::Salvaged(Self::salvage(&value), format!("it holds an invalid value ({error})")),
    }
  }

  /// Builds a config from the defaults and every value of `value` that
  /// still deserialises: an unknown enum value written by a newer version, a
  /// wrong type from a hand edit or a broken profile costs only that value,
  /// not the whole file.
  fn salvage(value: &Value) -> Self {
    let Ok(mut document) = serde_json::to_value(Self::default()) else {
      return Self::default();
    };
    keep_valid_values(&mut document, &mut Vec::new(), value);
    serde_json::from_value(document).unwrap_or_default()
  }

  /// Writes the config to a new temporary file of its own next to `path`
  /// (`BatchMkvMerge.json.<process id>-<counter>.tmp`), flushes it to disk
  /// and renames it over `path`, so a crash or a full disk never leaves a
  /// truncated config file behind, and two instances of the app saving at
  /// once never write into the same file: the last rename wins with a whole
  /// config.
  fn save(&self, path: &Path) -> Result<()> {
    log::debug!("Saving config to {}.", path.display());
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let (temp, file) = crate::unique_file::create(|tag| path.with_file_name(format!("{name}.{tag}.tmp")))
      .with_context(|| format!("Couldn't write config file {}", path.display()))?;
    let write = || -> Result<()> {
      let mut writer = BufWriter::new(file);
      serde_json::to_writer_pretty(&mut writer, self)?;
      writer.flush()?;
      writer.get_ref().sync_all()?;
      Ok(())
    };
    if let Err(error) = write() {
      let _ = std::fs::remove_file(&temp);
      return Err(error.context(format!("Couldn't write config file {}", temp.display())));
    }
    if let Err(error) = std::fs::rename(&temp, path) {
      let _ = std::fs::remove_file(&temp);
      return Err(anyhow::Error::new(error).context(format!("Couldn't replace config file {}", path.display())));
    }
    Ok(())
  }
}

/// Keeps an unusable config file under a backup name next to it that no
/// file has: `BatchMkvMerge.json.bad`, else `BatchMkvMerge.json.2.bad`,
/// `.3.bad` and so on. A backup is never replaced, also not by another
/// instance of the app repairing the same file at the same time. The backup
/// holds `contents`, the bytes this instance read and could not use, even
/// when another instance has replaced the file since. A file that could not
/// be read at all, and still cannot be read just before, is moved there
/// instead. Returns the backup's path.
fn keep_aside(path: &Path, contents: Option<&[u8]>) -> std::io::Result<PathBuf> {
  let name = path.file_name().unwrap_or_default().to_string_lossy();
  let mut number = 1;
  let (backup, mut file) = loop {
    let backup = match number {
      1 => path.with_file_name(format!("{name}.bad")),
      n => path.with_file_name(format!("{name}.{n}.bad")),
    };
    match std::fs::File::options().write(true).create_new(true).open(&backup) {
      Ok(file) => break (backup, file),
      Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && number < MAX_BACKUPS => number += 1,
      Err(err) => return Err(err),
    }
  };
  let kept = match contents {
    Some(contents) => file.write_all(contents).and_then(|()| file.sync_all()),
    None => {
      // Replaces only the empty file just created for it.
      drop(file);
      std::fs::rename(path, &backup)
    }
  };
  if let Err(err) = kept {
    let _ = std::fs::remove_file(&backup);
    return Err(err);
  }
  Ok(backup)
}

/// What [`Config::parse`] made of a config file.
enum Parsed {
  /// Every value loaded; whether the file names a language.
  Loaded(Config, bool),
  /// What could be salvaged (see [`Config::salvage`]) and why the file did
  /// not load as it was.
  Salvaged(Config, String),
}

/// Copies into `document` (a serialised default [`Config`]) each value of
/// the object `value` at `path` with which the document still deserialises.
/// A rejected value that is an object or an array where the document has one
/// is salvaged member by member instead.
fn keep_valid_values(document: &mut Value, path: &mut Vec<String>, value: &Value) {
  let Some(fields) = value.as_object() else {
    return;
  };
  for (key, field) in fields {
    let Some(previous) = object_at(document, path).map(|object| object.insert(key.clone(), field.clone())) else {
      return;
    };
    if serde_json::from_value::<Config>(document.clone()).is_ok() {
      continue;
    }
    if let Some(object) = object_at(document, path) {
      match &previous {
        Some(previous) => object.insert(key.clone(), previous.clone()),
        None => object.remove(key),
      };
    }
    path.push(key.clone());
    match (&previous, field) {
      (Some(Value::Object(_)), Value::Object(_)) => keep_valid_values(document, path, field),
      (Some(Value::Array(_)), Value::Array(items)) => keep_valid_items(document, path, items),
      _ => {}
    }
    path.pop();
  }
}

/// Replaces the array at `path` with the items of `items` that keep the
/// document valid; an empty array when none does.
fn keep_valid_items(document: &mut Value, path: &[String], items: &[Value]) {
  let Some((key, parent)) = path.split_last() else {
    return;
  };
  let Some(object) = object_at(document, parent) else {
    return;
  };
  let previous = object.insert(key.clone(), Value::Array(Vec::new()));
  if serde_json::from_value::<Config>(document.clone()).is_err() {
    if let (Some(object), Some(previous)) = (object_at(document, parent), previous) {
      object.insert(key.clone(), previous);
    }
    return;
  }
  for item in items {
    let push = |document: &mut Value, item: Option<&Value>| {
      if let Some(Value::Array(array)) = object_at(document, parent).and_then(|object| object.get_mut(key)) {
        match item {
          Some(item) => array.push(item.clone()),
          None => {
            array.pop();
          }
        }
      }
    };
    push(document, Some(item));
    if serde_json::from_value::<Config>(document.clone()).is_err() {
      push(document, None);
    }
  }
}

/// The object at `path` (object keys from the root) inside `document`.
fn object_at<'a>(document: &'a mut Value, path: &[String]) -> Option<&'a mut serde_json::Map<String, Value>> {
  path
    .iter()
    .try_fold(document, |value, key| value.get_mut(key))
    .and_then(Value::as_object_mut)
}

/// Whether the directory `dir` is `root` or inside it, comparing whole path
/// components case-insensitively, as Windows paths compare; `\` and `/`
/// both separate components.
#[cfg(any(target_os = "windows", test))]
fn is_within_directory(dir: &str, root: &str) -> bool {
  let components = |path: &str| -> Vec<String> {
    path
      .split(['\\', '/'])
      .filter(|component| !component.is_empty())
      .map(str::to_lowercase)
      .collect()
  };
  let (dir, root) = (components(dir), components(root));
  !root.is_empty() && dir.len() >= root.len() && dir[..root.len()] == root[..]
}

/// The directory that holds the config file (and the app's log), created if
/// needed. Computed without reading the config.
pub fn config_dir() -> PathBuf {
  let dir = Config::get_config_dir();
  if let Err(err) = std::fs::create_dir_all(&dir) {
    log::warn!("Couldn't create config dir {}: {}", dir.display(), err);
  }
  dir
}

pub fn get_config() -> Config {
  CONFIG
    .get_or_init(|| RwLock::new(Config::new()))
    .read()
    .unwrap_or_else(PoisonError::into_inner)
    .clone()
}

/// Saves `config` and makes it the config [`get_config`] returns. The lock is
/// held across both, so concurrent calls leave the file and the in-memory
/// config with the same value.
pub fn set_config(config: Config) -> Result<()> {
  let mut current = CONFIG
    .get_or_init(|| RwLock::new(Config::new()))
    .write()
    .unwrap_or_else(PoisonError::into_inner);
  let result = config.save(&Config::get_path_buf());
  *current = config;
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
  fn language_from_locale_tag_maps_supported_locales() {
    assert!(matches!(Language::default(), Language::EnUS));
    assert!(matches!(Language::from_locale_tag("de-DE"), Some(Language::De)));
    assert!(matches!(Language::from_locale_tag("en_US.UTF-8"), Some(Language::EnUS)));
    assert!(matches!(Language::from_locale_tag("es-MX"), Some(Language::Es)));
    assert!(matches!(Language::from_locale_tag("fr-CA"), Some(Language::Fr)));
    assert!(matches!(Language::from_locale_tag("it-IT"), Some(Language::It)));
    assert!(matches!(Language::from_locale_tag("ja-JP"), Some(Language::Ja)));
    assert!(matches!(Language::from_locale_tag("zh-Hans-CN"), Some(Language::ZhCN)));
    assert!(matches!(Language::from_locale_tag("zh-Hant-TW"), Some(Language::ZhTW)));
    assert!(matches!(Language::from_locale_tag("zh-Hant-HK"), Some(Language::ZhHK)));
    assert!(matches!(Language::from_locale_tag("zh-MO"), Some(Language::ZhHK)));
    assert!(matches!(Language::from_locale_tag("C.UTF-8"), None));
  }

  #[test]
  fn config_round_trips_with_parser_block() {
    let mut cfg = Config::default();
    cfg.parser.timeout_ms = 2500;
    let json = serde_json::to_string(&cfg).unwrap();
    let back: Config = serde_json::from_str(&json).unwrap();
    assert_eq!(back.parser.timeout_ms, 2500);
  }

  /// Every field of the config with a value other than its default, as the
  /// gpui-kit app writes it.
  const FULL_CONFIG: &str = r#"{
    "displayMode": "Dark",
    "theme": "Forest",
    "language": "ja",
    "externalTools": { "mkvToolNixPath": "/opt/mkvtoolnix", "betterMediaInfoPath": "/opt/bmi" },
    "profiles": [
      {
        "name": "Default",
        "selectVideo": true,
        "selectAudio": false,
        "selectSubtitle": false,
        "selectChapters": false,
        "selectAttachments": true,
        "videoLanguagesForTrackSelection": "jpn",
        "audioLanguagesForTrackSelection": "eng, jpn",
        "subtitleLanguagesForTrackSelection": "chi",
        "preferredVideoLanguages": "jpn",
        "preferredAudioLanguages": "jpn, eng",
        "preferredSubtitleLanguages": "chi, eng",
        "trackNamesVideo": { "jpn": "Japanese" },
        "trackNamesAudio": { "jpn": "Japanese\nCommentary" },
        "trackNamesSubtitle": { "chi": "Simplified Chinese" },
        "automation": {
          "reset_und_language": { "enabled": true, "language": "ja" },
          "set_track_name": { "enabled": true },
          "reset_default_track": { "enabled": true },
          "reset_forced_display": { "enabled": true }
        }
      },
      {
        "name": "Anime",
        "selectVideo": false,
        "selectAudio": true,
        "selectSubtitle": true,
        "selectChapters": true,
        "selectAttachments": false,
        "videoLanguagesForTrackSelection": "",
        "audioLanguagesForTrackSelection": "jpn",
        "subtitleLanguagesForTrackSelection": "eng",
        "preferredVideoLanguages": "",
        "preferredAudioLanguages": "jpn",
        "preferredSubtitleLanguages": "eng",
        "trackNamesVideo": {},
        "trackNamesAudio": {},
        "trackNamesSubtitle": {},
        "automation": {
          "reset_und_language": { "enabled": false, "language": "en" },
          "set_track_name": { "enabled": false },
          "reset_default_track": { "enabled": false },
          "reset_forced_display": { "enabled": false }
        }
      }
    ],
    "activeProfile": "Anime",
    "window": {
      "display": "5f2a7c3e-0d4b-5e1a-9c6f-2b8d4e7a1c90",
      "position": { "x": 120, "y": 80 },
      "size": { "width": 1440, "height": 960 }
    },
    "update": { "checkInterval": "Monthly", "lastChecked": 1800000000, "lastVersion": "1.0.1", "ignoreVersion": "1.0.1" },
    "parser": { "timeoutMs": 2500, "subtitleCharset": "windows-1252" },
    "groupMode": "TrackCountAndLanguage",
    "groupByFileName": false,
    "formatting": {
      "video": { "bitRate": { "precision": "Zero", "unit": "K" }, "size": { "precision": "One", "unit": "KM" } },
      "audio": { "bitRate": { "precision": "One", "unit": "KMG" }, "size": { "precision": "Zero", "unit": "KMi" } },
      "subtitle": { "bitRate": { "precision": "Two", "unit": "KMiGi" }, "size": { "precision": "One", "unit": "KMiGiTi" } }
    }
  }"#;

  /// The object keys at every depth, as `a.b.c` paths; maps keyed by data
  /// (track names) and arrays are not entered.
  fn key_paths(value: &serde_json::Value, prefix: &str, paths: &mut Vec<String>) {
    if let serde_json::Value::Object(map) = value {
      for (key, value) in map {
        let path = format!("{prefix}{key}");
        paths.push(path.clone());
        if !key.starts_with("trackNames") {
          key_paths(value, &format!("{path}."), paths);
        }
      }
    }
  }

  #[test]
  fn a_config_with_every_field_round_trips() {
    let written: serde_json::Value = serde_json::from_str(FULL_CONFIG).unwrap();
    let config: Config = serde_json::from_str(FULL_CONFIG).unwrap();
    assert_eq!(serde_json::to_value(&config).unwrap(), written);
    // The fixture covers every field the config has.
    let profile = serde_json::to_value(ConfigProfile::default()).unwrap();
    let mut expected = Vec::new();
    key_paths(&serde_json::to_value(Config::default()).unwrap(), "", &mut expected);
    key_paths(&profile, "profiles.", &mut expected);
    let mut actual = Vec::new();
    key_paths(&written, "", &mut actual);
    key_paths(&written["profiles"][0], "profiles.", &mut actual);
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
  }

  #[test]
  fn a_named_profile_selects_audio_subtitles_and_chapters_only_as_default() {
    let default = ConfigProfile::named(ConfigProfile::DEFAULT_NAME);
    assert_eq!(
      serde_json::to_value(&default).unwrap(),
      serde_json::to_value(ConfigProfile::default()).unwrap()
    );
    let other = ConfigProfile::named("Anime");
    assert_eq!(other.name, "Anime");
    assert!(!other.select_video && !other.select_audio && !other.select_subtitle);
    assert!(!other.select_chapters && !other.select_attachments);
    assert_eq!(
      other.audio_languages_for_track_selection,
      ConfigProfile::DEFAULT_LANGUAGES
    );
    assert_eq!(other.preferred_subtitle_languages, ConfigProfile::DEFAULT_LANGUAGES);
    assert_eq!(
      other.track_names_subtitle,
      ConfigProfile::default().track_names_subtitle
    );
  }

  /// A directory under the system temp dir, removed on drop.
  struct ScratchDir(PathBuf);

  impl ScratchDir {
    fn new(name: &str) -> Self {
      let path = std::env::temp_dir().join(format!("bmm_config_{name}_{}", std::process::id()));
      let _ = std::fs::remove_dir_all(&path);
      std::fs::create_dir_all(&path).unwrap();
      Self(path)
    }

    fn file(&self) -> PathBuf {
      self.0.join("BatchMkvMerge.json")
    }
  }

  impl Drop for ScratchDir {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  fn salvaged(json: &str) -> Config {
    match Config::parse(json.as_bytes()) {
      Parsed::Loaded(..) => panic!("{json} should not load as it is"),
      Parsed::Salvaged(config, _) => config,
    }
  }

  #[test]
  fn a_truncated_config_file_is_kept_aside_and_replaced_by_defaults() {
    let dir = ScratchDir::new("truncated");
    std::fs::write(dir.file(), "{\"theme\": \"Rose\", \"wind").unwrap();
    let (config, problem) = Config::load(dir.file());
    assert_eq!(config.theme, Theme::default());
    let problem = problem.expect("the problem is reported");
    assert!(problem.reason.contains("not valid JSON"), "{}", problem.reason);
    assert_eq!(
      problem.backup.as_deref(),
      Some(dir.0.join("BatchMkvMerge.json.bad").as_path())
    );
    assert_eq!(
      std::fs::read_to_string(dir.0.join("BatchMkvMerge.json.bad")).unwrap(),
      "{\"theme\": \"Rose\", \"wind"
    );
    let rewritten: Value = serde_json::from_slice(&std::fs::read(dir.file()).unwrap()).unwrap();
    assert_eq!(rewritten["theme"], "Ocean");
  }

  /// Two instances of the app read the same broken file. The first keeps it
  /// as a backup, saves the repaired config, and then the user's newer
  /// settings. The second, holding the bytes it read before all that, keeps
  /// them under a new backup name, never replaces the first backup, and
  /// finds the file replaced: it loads the newer settings instead of saving
  /// its own repair over them.
  #[test]
  fn a_late_repair_replaces_neither_a_backup_nor_newer_settings() {
    let dir = ScratchDir::new("backups");
    let broken = b"{\"theme\": \"Rose\", \"wind".to_vec();
    std::fs::write(dir.file(), &broken).unwrap();
    let (repaired, first) = Config::load(dir.file());
    assert_eq!(repaired.theme, Theme::default());
    let newer = Config {
      theme: Theme::Forest,
      ..repaired
    };
    newer.save(&dir.file()).unwrap();
    let (config, second) = Config::load_read(dir.file(), Ok(broken.clone()));
    assert_eq!((config.theme, second), (Theme::Forest, None), "the newer settings");
    let backups = [
      dir.0.join("BatchMkvMerge.json.bad"),
      dir.0.join("BatchMkvMerge.json.2.bad"),
    ];
    assert_eq!(first.unwrap().backup.as_ref(), Some(&backups[0]));
    for backup in &backups {
      assert_eq!(std::fs::read(backup).unwrap(), broken, "{}", backup.display());
    }
    let (config, problem) = Config::load(dir.file());
    assert_eq!((config.theme, problem), (Theme::Forest, None), "still in the file");
  }

  /// The same for a file that loads but is completed and saved at once (no
  /// `language` yet, as 0.1.0 wrote it): a second instance that read it
  /// before the first saved newer settings loads those instead of saving
  /// its own completion over them.
  #[test]
  fn a_late_completion_does_not_replace_newer_settings() {
    let dir = ScratchDir::new("completion");
    let old = br#"{"theme": "Rose"}"#.to_vec();
    std::fs::write(dir.file(), &old).unwrap();
    let (completed, problem) = Config::load(dir.file());
    assert_eq!((completed.theme, problem), (Theme::Rose, None));
    let newer = Config {
      theme: Theme::Forest,
      ..completed
    };
    newer.save(&dir.file()).unwrap();
    let (config, problem) = Config::load_read(dir.file(), Ok(old));
    assert_eq!((config.theme, problem), (Theme::Forest, None), "the newer settings");
    let (config, _) = Config::load(dir.file());
    assert_eq!(config.theme, Theme::Forest, "still in the file");
  }

  /// A file that cannot be read is moved to its backup name.
  #[cfg(unix)]
  #[test]
  fn an_unreadable_config_file_is_moved_to_its_backup() {
    use std::os::unix::fs::PermissionsExt;
    let dir = ScratchDir::new("unreadable");
    std::fs::write(dir.file(), "held by another program").unwrap();
    std::fs::write(dir.0.join("BatchMkvMerge.json.bad"), "an older backup").unwrap();
    let mode = |path: &Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    mode(&dir.file(), 0o000);
    if std::fs::read(dir.file()).is_ok() {
      // Root reads every file.
      return;
    }
    let (_, problem) = Config::load(dir.file());
    let problem = problem.expect("the problem is reported");
    assert!(problem.reason.contains("could not be read"), "{}", problem.reason);
    let backup = dir.0.join("BatchMkvMerge.json.2.bad");
    assert_eq!(problem.backup.as_deref(), Some(backup.as_path()));
    mode(&backup, 0o600);
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "held by another program");
    assert_eq!(
      std::fs::read_to_string(dir.0.join("BatchMkvMerge.json.bad")).unwrap(),
      "an older backup"
    );
    let (_, problem) = Config::load(dir.file());
    assert_eq!(problem, None, "the defaults were saved in its place");
  }

  /// A read that failed, and the file can be read now: another instance has
  /// saved its settings there since. They are loaded where they are, not
  /// moved aside and replaced by the defaults.
  #[test]
  fn settings_saved_after_a_failed_read_are_loaded_where_they_are() {
    let dir = ScratchDir::new("read-again");
    let newer = Config {
      theme: Theme::Forest,
      ..Config::default()
    };
    newer.save(&dir.file()).unwrap();
    let saved = std::fs::read(dir.file()).unwrap();
    let unreadable = Err(std::io::Error::other("unreadable"));
    let (config, problem) = Config::load_read(dir.file(), unreadable);
    assert_eq!((config.theme, problem), (Theme::Forest, None), "the newer settings");
    assert_eq!(std::fs::read(dir.file()).unwrap(), saved, "left where they are");
    assert!(!dir.0.join("BatchMkvMerge.json.bad").exists(), "nothing moved aside");
  }

  #[test]
  fn an_unknown_or_mistyped_value_costs_only_that_value() {
    let config = salvaged(
      r#"{"theme": "Rose", "displayMode": "Sepia", "language": "ko",
          "window": {"size": {"width": "1200", "height": 700}, "position": {"x": 5, "y": 6}},
          "update": {"checkInterval": "Hourly", "ignoreVersion": "2.0.0"},
          "formatting": {"audio": {"size": {"precision": "Three", "unit": "KMi"}}},
          "groupByFileName": false}"#,
    );
    assert_eq!(config.theme, Theme::Rose);
    assert_eq!(config.display_mode, DisplayMode::Auto);
    assert_eq!((config.window.position.x, config.window.position.y), (5, 6));
    assert_eq!(config.window.size.height, 700);
    assert_eq!(config.window.size.width, ConfigWindowSize::default().width);
    assert_eq!(config.update.check_interval, UpdateCheckInterval::Weekly);
    assert_eq!(config.update.ignore_version, "2.0.0");
    assert_eq!(config.formatting.audio.size.precision, FormatPrecision::Two);
    assert_eq!(config.formatting.audio.size.unit, FormatUnit::KMi);
    assert!(!config.group_by_file_name);
  }

  #[test]
  fn a_broken_profile_is_dropped_and_the_others_are_kept() {
    let config = salvaged(
      r#"{"profiles": [{"name": "Default"}, {"name": "Broken", "selectVideo": "yes"}, {"name": "Anime"}],
          "activeProfile": "Anime"}"#,
    );
    let names: Vec<&str> = config.profiles.iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(names, ["Default", "Anime"]);
    assert_eq!(config.active_profile, "Anime");
  }

  #[test]
  fn a_config_file_that_is_not_an_object_loads_the_defaults() {
    let config = salvaged("[1, 2]");
    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.theme, Theme::default());
  }

  #[test]
  fn a_valid_config_file_loads_without_a_problem_and_without_a_backup() {
    let dir = ScratchDir::new("valid");
    std::fs::write(dir.file(), r#"{"theme": "Forest", "language": "fr"}"#).unwrap();
    let (config, problem) = Config::load(dir.file());
    assert_eq!(
      (config.theme, config.language, problem),
      (Theme::Forest, Language::Fr, None)
    );
    assert!(!dir.0.join("BatchMkvMerge.json.bad").exists());
  }

  #[test]
  fn saving_replaces_the_file_and_leaves_no_temporary_file() {
    let dir = ScratchDir::new("save");
    std::fs::write(dir.file(), "old contents").unwrap();
    let config = Config {
      theme: Theme::Amber,
      ..Config::default()
    };
    config.save(&dir.file()).unwrap();
    let (loaded, problem) = Config::load(dir.file());
    assert_eq!((loaded.theme, problem), (Theme::Amber, None));
    let names: Vec<String> = std::fs::read_dir(&dir.0)
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    assert_eq!(names, ["BatchMkvMerge.json"]);
  }

  /// Two instances of the app may save at the same time; every save must
  /// succeed and leave a whole config.
  #[test]
  fn concurrent_saves_each_write_a_temporary_file_of_their_own() {
    let dir = ScratchDir::new("concurrent");
    let themes = [Theme::Amber, Theme::Rose, Theme::Forest, Theme::Indigo];
    std::thread::scope(|scope| {
      for theme in themes {
        let file = dir.file();
        scope.spawn(move || {
          let config = Config {
            theme,
            ..Config::default()
          };
          for _ in 0..25 {
            config.save(&file).unwrap();
          }
        });
      }
    });
    let (loaded, problem) = Config::load(dir.file());
    assert_eq!(problem, None);
    assert!(themes.contains(&loaded.theme));
    let names: Vec<String> = std::fs::read_dir(&dir.0)
      .unwrap()
      .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
      .collect();
    assert_eq!(names, ["BatchMkvMerge.json"]);
  }

  #[test]
  fn saving_into_a_missing_directory_returns_the_error() {
    let dir = ScratchDir::new("missing");
    let error = Config::default().save(&dir.0.join("gone").join("BatchMkvMerge.json"));
    assert!(error.is_err());
  }

  #[test]
  fn install_roots_match_whole_path_components_case_insensitively() {
    assert!(is_within_directory(
      r"C:\Program Files\BatchMkvMerge",
      r"c:\program files"
    ));
    assert!(is_within_directory(r"C:\Program Files", r"C:\Program Files\"));
    assert!(is_within_directory(
      r"C:\Users\me\AppData\Local\BatchMkvMerge",
      r"C:/Users/me/AppData/Local"
    ));
    assert!(!is_within_directory(
      r"C:\Program Files Portable\BatchMkvMerge",
      r"C:\Program Files"
    ));
    assert!(!is_within_directory(r"D:\Tools", r"C:\Program Files"));
    assert!(!is_within_directory(r"C:\Program Files", ""));
  }

  #[test]
  fn profiles_are_made_unique_with_a_default_and_a_valid_active_one() {
    let dir = ScratchDir::new("profiles");
    std::fs::write(
      dir.file(),
      r#"{"language": "en-US", "activeProfile": "Gone",
          "profiles": [{"name": "Anime"}, {"name": "Anime"}, {"name": "Anime (2)"}, {"name": "Anime"}]}"#,
    )
    .unwrap();
    let (config, problem) = Config::load(dir.file());
    assert_eq!(problem, None);
    let names: Vec<&str> = config.profiles.iter().map(|profile| profile.name.as_str()).collect();
    assert_eq!(names, ["Default", "Anime", "Anime (3)", "Anime (2)", "Anime (4)"]);
    assert_eq!(config.active_profile, "Default");
    let (saved, _) = Config::load(dir.file());
    assert_eq!(saved.profiles.len(), 5, "the completed list was saved");
    let mut fine = Config::default();
    assert!(!fine.normalize_profiles(), "a default config needs nothing");
  }
}
