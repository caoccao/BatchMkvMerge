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

//! The views, one module per screen of the React app.
//!
//! Sizes follow the React layout: MUI spacing units are 8 px ([`space`]) and
//! the type scale is MUI's at `typography.fontSize: 12` ([`text`]).

pub mod about;
pub mod actions;
pub mod cards;
pub mod file_list;
pub mod footer;
pub mod main_content;
pub mod output_dialog;
pub mod root;
pub mod toolbar;
pub mod track_table;
pub mod welcome;

#[cfg(test)]
mod tests;

use gpui_kit::{Hsla, Pixels, Rgba, px};

/// `n` MUI spacing units (8 px each).
pub fn space(n: f32) -> Pixels {
  px(8.0 * n)
}

/// MUI's type scale at `typography.fontSize: 12` (every variant × 12/14).
pub mod text {
  use gpui_kit::{Pixels, px};

  pub fn h3() -> Pixels {
    px(41.0)
  }

  pub fn h4() -> Pixels {
    px(29.0)
  }

  pub fn h6() -> Pixels {
    px(17.0)
  }

  pub fn body2() -> Pixels {
    px(12.0)
  }

  pub fn caption() -> Pixels {
    px(10.0)
  }
}

/// A CSS `rgba(r, g, b, a)` colour.
pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Hsla {
  Rgba {
    r: f32::from(r) / 255.0,
    g: f32::from(g) / 255.0,
    b: f32::from(b) / 255.0,
    a,
  }
  .into()
}

/// A CSS `#rrggbb` colour.
pub fn hex(value: u32) -> Hsla {
  gpui_kit::rgb(value).into()
}
