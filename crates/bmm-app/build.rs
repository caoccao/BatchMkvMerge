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

use std::fs;
use std::io;
use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD};

fn main() {
  println!("cargo:rerun-if-changed=build.rs");
  println!("cargo:rerun-if-changed=assets/toolbar");
  println!("cargo:rerun-if-changed=packaging/icons/icon.ico");
  if let Err(error) = wrap_toolbar_icons() {
    panic!("failed to embed the toolbar icons: {error}");
  }
  // CARGO_CFG_TARGET_OS, not cfg!(windows): the build script runs on the host.
  if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("packaging/icons/icon.ico");
    if let Err(error) = resource.compile() {
      panic!("failed to embed the Windows icon resource: {error}");
    }
  }
}

/// GPUI tints an SVG using its alpha mask and rasterizes at twice the device
/// size. Wrap the high-resolution artwork in a 32 px viewport so the toolbar
/// keeps its logical size and stays sharp on high-DPI displays.
fn wrap_toolbar_icons() -> io::Result<()> {
  let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("toolbar");
  fs::create_dir_all(&output)?;
  for entry in fs::read_dir("assets/toolbar")? {
    let entry = entry?;
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str().and_then(|name| name.strip_suffix("-256x256.png")) else {
      continue;
    };
    let image = STANDARD.encode(fs::read(entry.path())?);
    fs::write(
      output.join(format!("{name}.svg")),
      format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 32 32"><image width="32" height="32" href="data:image/png;base64,{image}"/></svg>"#
      ),
    )?;
  }
  Ok(())
}
