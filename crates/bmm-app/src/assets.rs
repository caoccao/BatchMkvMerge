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

//! Application images, toolbar icons, and Lucide icons beyond gpui-kit's
//! default bundle.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::component::Icon;
use gpui_kit::{AssetSource, Result, SharedString};

/// Asset path of the application icon (256×256 PNG).
pub const APP_ICON: &str = "images/icon.png";
/// Logos of the sibling applications on the Welcome page.
pub const BETTER_MEDIA_INFO_ICON: &str = "images/bettermediainfo.png";
pub const BATCH_MKV_EXTRACT_ICON: &str = "images/batchmkvextract.png";
pub const BD_MASTER_ICON: &str = "images/bdmaster.png";

pub const OUTPUT_DIRECTORY_ICON_NAME: &str = "output-directory";
pub const SETTINGS_ICON_NAME: &str = "settings";

pub fn toolbar_icon(name: &str) -> Icon {
  Icon::default().path(format!("icons/toolbar/{name}.svg"))
}

const IMAGES: &[(&str, &[u8])] = &[
  (APP_ICON, include_bytes!("../assets/icon.png")),
  (BETTER_MEDIA_INFO_ICON, include_bytes!("../assets/bettermediainfo.png")),
  (BATCH_MKV_EXTRACT_ICON, include_bytes!("../assets/batchmkvextract.png")),
  (BD_MASTER_ICON, include_bytes!("../assets/bdmaster.png")),
];

macro_rules! toolbar_icons {
  ($($name:literal),+ $(,)?) => {
    const TOOLBAR_ICONS: &[(&str, &[u8])] = &[
      $((
        concat!("icons/toolbar/", $name, ".svg"),
        include_bytes!(concat!(env!("OUT_DIR"), "/toolbar/", $name, ".svg")),
      )),+
    ];
  };
}

toolbar_icons!(
  "about",
  "cancel-all",
  "clear-completed",
  "clear-list",
  "group-by-filename",
  "group-none",
  "group-track-count",
  "group-track-count-language",
  "merge-all",
  "output-directory",
  "profile",
  "resume-all",
  "settings",
  "system-notification",
  "topmost-notification",
);

// Lucide icons the UI needs that are not part of gpui-kit's default 106-icon
// bundle; every icon is in the `IconName` enum, only the SVGs are opt-in.
icon_assets!(
  ExtraIcons,
  [
    Blocks,
    CircleMinus,
    Clapperboard,
    Clock,
    Combine,
    Cpu,
    Files,
    Film,
    FolderCog,
    GripVertical,
    Images,
    ListFilter,
    RotateCcw,
    Settings2,
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
    if let Some((_, bytes)) = IMAGES.iter().chain(TOOLBAR_ICONS).find(|(name, _)| *name == path) {
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
      .chain(TOOLBAR_ICONS)
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

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use gpui_kit::{DevicePixels, SvgRenderer, SvgSize, size};

  use super::*;

  #[test]
  fn toolbar_icons_rasterize_with_transparent_backgrounds() {
    let renderer = SvgRenderer::new(Arc::new(AppAssets));
    for (path, _) in TOOLBAR_ICONS {
      let svg = AppAssets.load(path).unwrap().unwrap();
      let svg = renderer.parse_svg(&svg).unwrap();
      for side in [32, 64, 96, 128] {
        let dimensions = size(DevicePixels(side), DevicePixels(side));
        let image = renderer.render_parsed(&svg, SvgSize::ExactSize(dimensions)).unwrap();
        assert_eq!(image.size(0), dimensions, "{path}");
        let pixels = image.as_bytes(0).unwrap().chunks_exact(4);
        assert!(pixels.clone().any(|pixel| pixel[3] == 0), "{path} has no transparency");
        assert!(pixels.clone().any(|pixel| pixel[3] > 128), "{path} is blank");
      }
    }
  }
}
