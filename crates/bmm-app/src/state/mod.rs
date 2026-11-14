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

//! Application state, split by how often it changes (one entity per row of
//! the plan's state table), so a change repaints only the views observing it.
//! The queue store arrives with the merge pipeline.

pub mod config;
pub mod files;
pub mod ui;

use bmm_core::config::Config;
use gpui_kit::{App, AppContext, Entity};

pub use config::{ConfigStore, Persistence};
pub use files::FileStore;
pub use ui::{TabType, UiState};

/// Handles to every store; cheap to clone and handed to every view.
#[derive(Clone)]
pub struct Stores {
  pub config: Entity<ConfigStore>,
  pub files: Entity<FileStore>,
  pub ui: Entity<UiState>,
}

impl Stores {
  pub fn new(config: Config, persistence: Persistence, cx: &mut App) -> Self {
    Self {
      config: cx.new(|cx| ConfigStore::new(config, persistence, cx)),
      files: cx.new(|_| FileStore::new()),
      ui: cx.new(|_| UiState::new()),
    }
  }

  /// Whether a merge is waiting or running. Always false until the merge
  /// queue is wired; the close guard and the toolbar already ask.
  pub fn has_active_merges(&self, _cx: &App) -> bool {
    false
  }
}
