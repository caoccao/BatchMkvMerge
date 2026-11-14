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

pub mod config;
pub mod files;
pub mod queue;
pub mod ui;

use bmm_core::config::Config;
use gpui_kit::{App, AppContext, Entity};

pub use config::{ConfigStore, Persistence};
pub use files::{FileStore, MetadataSource};
pub use queue::QueueStore;
pub use ui::{TabType, UiState};

use crate::merge_events::MergeBackend;

/// What the stores talk to outside the UI: the config file, the parser and
/// the merge queue. The app uses the real ones; tests pass stand-ins.
#[derive(Clone)]
pub struct Services {
  pub persistence: Persistence,
  pub metadata: MetadataSource,
  pub merges: MergeBackend,
}

/// Handles to every store; cheap to clone and handed to every view.
#[derive(Clone)]
pub struct Stores {
  pub config: Entity<ConfigStore>,
  pub files: Entity<FileStore>,
  pub queue: Entity<QueueStore>,
  pub ui: Entity<UiState>,
  /// Enqueues and cancels merges; their events reach `queue` through
  /// [`crate::merge_events::start`].
  pub merges: MergeBackend,
}

impl Stores {
  pub fn new(config: Config, services: Services, cx: &mut App) -> Self {
    let Services {
      persistence,
      metadata,
      merges,
    } = services;
    let config = cx.new(|cx| ConfigStore::new(config, persistence, cx));
    Self {
      files: cx.new(|cx| FileStore::new(&config, metadata, cx)),
      config,
      queue: cx.new(|_| QueueStore::new()),
      ui: cx.new(|_| UiState::new()),
      merges,
    }
  }

  /// Whether a merge is waiting or running.
  pub fn has_active_merges(&self, cx: &App) -> bool {
    self.queue.read(cx).has_active()
  }
}
