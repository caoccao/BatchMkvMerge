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

//! Launcher progress through the `com.canonical.Unity.LauncherEntry.Update`
//! session-bus signal, which KDE Plasma, GNOME's Dash to Dock / Ubuntu Dock and
//! Unity honour. The signal names the application by its desktop file, so a
//! run whose desktop id does not match an installed `.desktop` file (an
//! AppImage, `cargo run`) shows nothing. That is best effort by design.
//!
//! D-Bus calls run on a dedicated thread so a slow or missing session bus
//! never blocks the UI thread.

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::thread;

use raw_window_handle::HasWindowHandle;
use zbus::zvariant::Value;

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
const OBJECT_PATH: &str = "/com/canonical/unity/launcherentry/batchmkvmerge";

pub(super) struct Progress {
  sender: Option<Sender<Option<u8>>>,
}

impl Progress {
  pub(super) fn new(_window: &impl HasWindowHandle, desktop_id: &str) -> Self {
    let app_uri = format!("application://{desktop_id}.desktop");
    let (sender, receiver) = mpsc::channel::<Option<u8>>();
    let spawned = thread::Builder::new().name("launcher-progress".into()).spawn(move || {
      let connection = match zbus::blocking::Connection::session() {
        Ok(connection) => connection,
        Err(error) => {
          log::warn!("Launcher progress disabled, no session bus: {error}");
          return;
        }
      };
      while let Ok(percent) = receiver.recv() {
        if let Err(error) = emit_update(&connection, &app_uri, percent) {
          log::warn!("Failed to emit launcher progress: {error}");
        }
      }
    });
    match spawned {
      Ok(_) => Self { sender: Some(sender) },
      Err(error) => {
        log::warn!("Failed to start the launcher progress thread: {error}");
        Self { sender: None }
      }
    }
  }

  pub(super) fn set(&mut self, percent: Option<u8>) {
    if let Some(sender) = &self.sender {
      let _ = sender.send(percent);
    }
  }
}

fn emit_update(connection: &zbus::blocking::Connection, app_uri: &str, percent: Option<u8>) -> zbus::Result<()> {
  let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
  properties.insert("progress", Value::from(f64::from(percent.unwrap_or(0)) / 100.0));
  properties.insert("progress-visible", Value::from(percent.is_some()));
  connection.emit_signal(None::<&str>, OBJECT_PATH, INTERFACE, "Update", &(app_uri, properties))
}
