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

//! The File List tab (`FileList.tsx`): the Welcome page while it is empty,
//! otherwise one row per file. The rows become merge cards with the cards.

use gpui_kit::base::TestSupportExt;
use gpui_kit::component::v_flex;
use gpui_kit::*;

use super::welcome::Welcome;
use crate::state::Stores;

pub struct FileListView {
  stores: Stores,
  _subscriptions: Vec<Subscription>,
}

impl FileListView {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![cx.observe(&stores.files, |_, _, cx| cx.notify())];
    Self {
      stores,
      _subscriptions: subscriptions,
    }
  }
}

impl Render for FileListView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let files = self.stores.files.read(cx).files();
    if files.is_empty() {
      return Welcome::new(self.stores.clone()).into_any_element();
    }
    v_flex()
      .id("file-rows")
      .test_support()
      .gap(super::space(0.5))
      .text_size(super::text::body2())
      .children(
        files
          .iter()
          .enumerate()
          .map(|(index, file)| div().id(("file-row", index)).test_support().child(file.clone())),
      )
      .into_any_element()
  }
}
