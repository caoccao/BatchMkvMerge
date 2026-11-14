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

//! The tabs (`MainContent.tsx`): File List and Queue, plus the Settings and
//! About tabs the toolbar opens, each with a close button. Every panel's view
//! is kept alive while hidden, as the React panels stayed mounted.

use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Sizable, v_flex};
use gpui_kit::*;

use super::about::About;
use super::file_list::FileListView;
use crate::state::{Stores, TabType};

pub struct MainContent {
  stores: Stores,
  file_list: Entity<FileListView>,
  _subscriptions: Vec<Subscription>,
}

impl MainContent {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![cx.observe(&stores.ui, |_, _, cx| cx.notify())];
    Self {
      file_list: cx.new(|cx| FileListView::new(stores.clone(), cx)),
      stores,
      _subscriptions: subscriptions,
    }
  }

  fn tab(&self, tab: TabType, cx: &mut Context<Self>) -> Tab {
    let label = Tab::new().label(tr!(tab.label_key()));
    if !tab.is_closable() {
      return label;
    }
    label.suffix(
      Button::new(SharedString::from(format!("tab-close-{}", tab.name())))
        .ghost()
        .xsmall()
        .icon(IconName::X)
        .accessibility_label(tr!("tabs.close"))
        .tooltip(tr!("tabs.close"))
        .on_click(cx.listener(move |this, _, _, cx| {
          // The click closes the tab without selecting it first.
          cx.stop_propagation();
          this.stores.ui.update(cx, |ui, cx| ui.close_tab(tab, cx));
        })),
    )
  }

  /// A tab whose screen is not built yet: its name, muted.
  fn pending(tab: TabType, cx: &App) -> AnyElement {
    v_flex()
      .flex_1()
      .items_center()
      .justify_center()
      .text_color(cx.theme().muted_foreground)
      .child(tr!(tab.label_key()))
      .into_any_element()
  }
}

impl Render for MainContent {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let ui = self.stores.ui.read(cx);
    let (tabs, active, index) = (ui.tabs(), ui.active_tab(), ui.active_index());
    let tab_elements: Vec<Tab> = tabs.iter().map(|tab| self.tab(*tab, cx)).collect();
    let content = match active {
      TabType::FileList => self.file_list.clone().into_any_element(),
      TabType::About => About.into_any_element(),
      TabType::Queue | TabType::Settings => Self::pending(active, cx),
    };
    let border = cx.theme().border;
    v_flex()
      .id("main-content")
      .test_support()
      .size_full()
      .overflow_hidden()
      .child(
        TabBar::new("main-tabs")
          .small()
          .selected_index(index)
          .children(tab_elements)
          .on_click(cx.listener(move |this, index: &usize, _, cx| {
            if let Some(tab) = tabs.get(*index).copied() {
              this.stores.ui.update(cx, |ui, cx| ui.set_active_tab(tab, cx));
            }
          })),
      )
      .child(
        v_flex()
          .id(SharedString::from(format!("panel-{}", active.name())))
          .flex_1()
          .min_h_0()
          .w_full()
          .p(super::space(1.0))
          .border_1()
          .border_t_0()
          .border_color(border)
          .rounded_b(px(4.0))
          .overflow_y_scroll()
          .child(content),
      )
  }
}
