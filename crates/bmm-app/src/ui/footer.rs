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

//! The footer: the donation link, copyright and author links.

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::link::Link;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::*;

const DONATE_URL: &str = "https://paypal.me/caoccao?locale.x=en_US";
const AUTHOR_URL: &str = "https://github.com/caoccao";
const HOMEPAGE_URL: &str = "https://www.caoccao.com/";

#[derive(IntoElement)]
pub struct Footer;

impl RenderOnce for Footer {
  fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let link = cx.theme().link;
    v_flex()
      .id("footer")
      .test_support()
      .my(super::space(1.5))
      .items_center()
      .gap(super::space(0.5))
      .text_size(super::text::caption())
      .text_color(cx.theme().muted_foreground)
      .child(
        Link::new("footer-donate")
          .href(DONATE_URL)
          .text_color(link)
          .child(tr!("footer.donate")),
      )
      .child(
        h_flex()
          .gap_1()
          .child(tr!("footer.copyright"))
          .child(
            Link::new("footer-author")
              .href(AUTHOR_URL)
              .text_color(link)
              .child("Sam Cao"),
          )
          .child(
            Link::new("footer-homepage")
              .href(HOMEPAGE_URL)
              .text_color(link)
              .child("caoccao.com"),
          ),
      )
  }
}
