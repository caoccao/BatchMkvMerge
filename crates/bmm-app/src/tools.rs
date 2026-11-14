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

//! The two external tools of the Integration settings (`isMkvtoolnixFound`,
//! `detectBetterMediaInfo`): where MKVToolNix and BetterMediaInfo are.
//! Detection never writes the config; the caller stores a found path through
//! the config store, as `useToolPathDetection` did.

use std::sync::Arc;

use bmm_core::config::{Config, ConfigExternalTools};

/// An external tool with a path in `config.externalTools`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
  MkvToolNix,
  BetterMediaInfo,
}

impl Tool {
  /// A stable name for element ids.
  pub fn name(self) -> &'static str {
    match self {
      Self::MkvToolNix => "mkvtoolnix",
      Self::BetterMediaInfo => "better-media-info",
    }
  }

  /// The configured path.
  pub fn path(self, config: &Config) -> &str {
    match self {
      Self::MkvToolNix => &config.external_tools.mkv_toolnix_path,
      Self::BetterMediaInfo => &config.external_tools.better_media_info_path,
    }
  }

  pub fn set_path(self, config: &mut Config, path: String) {
    match self {
      Self::MkvToolNix => config.external_tools.mkv_toolnix_path = path,
      Self::BetterMediaInfo => config.external_tools.better_media_info_path = path,
    }
  }

  /// The path of a fresh config (the platform's MKVToolNix directory; no
  /// BetterMediaInfo path).
  pub fn default_path(self) -> String {
    let defaults = ConfigExternalTools::default();
    match self {
      Self::MkvToolNix => defaults.mkv_toolnix_path,
      Self::BetterMediaInfo => defaults.better_media_info_path,
    }
  }
}

/// The outcome of a detection: whether the tool is there, and the directory
/// to keep (it can differ from the given path: a running application's
/// directory, a versioned macOS bundle, the folder of a given executable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
  pub found: bool,
  pub path: String,
}

type Detect = dyn Fn(Tool, &str, bool) -> ToolStatus + Send + Sync;

/// Finds the tools: the core's detection in the app, a stand-in in tests.
/// Runs on the background executor.
#[derive(Clone)]
pub struct ToolDetector(Arc<Detect>);

impl ToolDetector {
  pub fn new(detect: impl Fn(Tool, &str, bool) -> ToolStatus + Send + Sync + 'static) -> Self {
    Self(Arc::new(detect))
  }

  /// The core's detection, which looks at the file system and, with
  /// `check_running`, at the running processes first.
  pub fn system() -> Self {
    Self::new(|tool, path, check_running| match tool {
      Tool::MkvToolNix => {
        let status = bmm_core::mkvtoolnix::detect_mkvtoolnix(path, check_running);
        ToolStatus {
          found: status.found,
          path: status.mkv_toolnix_path,
        }
      }
      Tool::BetterMediaInfo => match bmm_core::services::detect_better_media_info(path.to_string(), check_running) {
        Ok(status) => ToolStatus {
          found: status.found,
          path: status.path,
        },
        Err(error) => {
          log::warn!("BetterMediaInfo detection failed: {error}");
          ToolStatus {
            found: false,
            path: String::new(),
          }
        }
      },
    })
  }

  /// Looks for `tool` at `path` (trimmed by the caller).
  pub fn detect(&self, tool: Tool, path: &str, check_running: bool) -> ToolStatus {
    (self.0)(tool, path, check_running)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;

  #[test]
  fn each_tool_reads_and_writes_its_own_path() {
    let mut config = Config::default();
    Tool::MkvToolNix.set_path(&mut config, "/opt/mkv".into());
    Tool::BetterMediaInfo.set_path(&mut config, "/opt/bmi".into());
    assert_eq!(Tool::MkvToolNix.path(&config), "/opt/mkv");
    assert_eq!(Tool::BetterMediaInfo.path(&config), "/opt/bmi");
    assert_eq!(
      Tool::MkvToolNix.default_path(),
      ConfigExternalTools::default().mkv_toolnix_path
    );
    assert!(Tool::BetterMediaInfo.default_path().is_empty());
  }

  #[test]
  fn the_system_detector_finds_nothing_in_an_empty_directory() {
    let dir = std::env::temp_dir().join(format!("bmm_tools_empty_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.to_string_lossy().to_string();
    let status = ToolDetector::system().detect(Tool::MkvToolNix, &path, false);
    assert!(!status.found);
    std::fs::remove_dir_all(&dir).unwrap();
  }
}
