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

//! Application assets: the app's own images plus the Lucide icons the UI
//! uses, composed over gpui-kit's default icon bundle.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

/// Asset path of the application icon (256×256 PNG).
pub const APP_ICON: &str = "images/icon.png";

const IMAGES: &[(&str, &[u8])] = &[(APP_ICON, include_bytes!("../assets/icon.png"))];

// Lucide icons the UI needs that are not part of gpui-kit's default 106-icon
// bundle; every icon is in the `IconName` enum, only the SVGs are opt-in.
icon_assets!(
  ExtraIcons,
  [
    Blocks,
    BrushCleaning,
    CircleMinus,
    Clapperboard,
    Clock,
    Combine,
    Files,
    Film,
    FolderCog,
    GripVertical,
    Images,
    PictureInPicture2,
    RotateCcw,
    SlidersHorizontal,
    SunMoon,
    Tornado,
    Trash,
    X,
  ]
);

/// The asset source registered with the application.
pub struct AppAssets;

impl AssetSource for AppAssets {
  fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
    if let Some((_, bytes)) = IMAGES.iter().find(|(name, _)| *name == path) {
      return Ok(Some(Cow::Borrowed(bytes)));
    }
    if let Some(bytes) = ExtraIcons.load(path)? {
      return Ok(Some(bytes));
    }
    Assets.load(path)
  }

  fn list(&self, path: &str) -> Result<Vec<SharedString>> {
    let mut names: Vec<SharedString> = IMAGES
      .iter()
      .filter(|(name, _)| name.starts_with(path))
      .map(|(name, _)| SharedString::new_static(name))
      .collect();
    names.extend(ExtraIcons.list(path)?);
    names.extend(Assets.list(path)?);
    names.sort();
    names.dedup();
    Ok(names)
  }
}
