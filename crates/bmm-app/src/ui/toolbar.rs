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

//! The toolbar (`Toolbar.tsx`): icon buttons with tooltips and the same
//! enabled states. Settings, About, Clear and Group by file name work; Merge
//! All, Cancel All, the global output path and the group-mode and profile
//! menus join with the features they drive.

use bmm_core::config::GroupMode;
use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Icon, h_flex};
use gpui_kit::*;

use super::actions;
use crate::state::{Stores, TabType};

/// The i18n key naming a group mode (`groupModeLabelKey`).
pub fn group_mode_label_key(mode: &GroupMode) -> &'static str {
  match mode {
    GroupMode::None => "groupMode.none",
    GroupMode::TrackCount => "groupMode.trackCount",
    GroupMode::TrackCountAndLanguage => "groupMode.trackCountAndLanguage",
  }
}

pub struct Toolbar {
  stores: Stores,
  _subscriptions: Vec<Subscription>,
}

impl Toolbar {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.observe(&stores.config, |_, _, cx| cx.notify()),
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      cx.observe(&stores.ui, |_, _, cx| cx.notify()),
    ];
    Self {
      stores,
      _subscriptions: subscriptions,
    }
  }
}

/// A 28 px ghost icon button (gpui-kit's default size at the 14 px base font);
/// `color` tints the icon while it is enabled.
fn icon_button(id: &'static str, icon: IconName, color: Option<Hsla>, tooltip: SharedString, enabled: bool) -> Button {
  let icon = match color.filter(|_| enabled) {
    Some(color) => Icon::new(icon).text_color(color),
    None => Icon::new(icon),
  };
  Button::new(id)
    .ghost()
    .icon(icon)
    .accessibility_label(tooltip.clone())
    .tooltip(tooltip)
    .disabled(!enabled)
}

fn divider(cx: &App) -> Div {
  div().w(px(1.0)).h(px(20.0)).mx(super::space(0.5)).bg(cx.theme().border)
}

impl Render for Toolbar {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let (primary, muted, success, danger) = (theme.primary, theme.muted_foreground, theme.success, theme.danger);
    let config = self.stores.config.read(cx).config();
    let group_mode = config.group_mode.clone();
    let group_by_file_name = config.group_by_file_name;
    let has_profiles = !config.profiles.is_empty();
    let has_global_output = self.stores.files.read(cx).global_output_dir().is_some();
    let active_tab = self.stores.ui.read(cx).active_tab();
    let has_active_jobs = self.stores.has_active_merges(cx);
    let can_clear = actions::can_clear(&self.stores, cx);
    // Merge All needs selected tracks, which the track table provides.
    let can_merge_all = false;
    let group_color = match group_mode {
      GroupMode::None => muted.opacity(0.6),
      GroupMode::TrackCountAndLanguage => success,
      GroupMode::TrackCount => primary,
    };
    let highlight = |on: bool| on.then_some(primary);

    h_flex().id("toolbar").test_support().gap(super::space(1.0)).child(
      h_flex()
        .items_center()
        .child(icon_button(
          "toolbar-global-output",
          IconName::FolderCog,
          Some(if has_global_output { primary } else { muted }),
          tr!("toolbar.setGlobalOutputPath"),
          true,
        ))
        .child(icon_button(
          "toolbar-merge-all",
          IconName::Combine,
          None,
          tr!("toolbar.mergeAll"),
          can_merge_all,
        ))
        .child(divider(cx))
        .child(icon_button(
          "toolbar-cancel-all",
          IconName::CircleX,
          Some(danger),
          tr!("toolbar.cancelAll"),
          has_active_jobs,
        ))
        .child(
          icon_button("toolbar-clear", IconName::Trash, None, tr!("toolbar.clear"), can_clear)
            .on_click(cx.listener(|this, _, _, cx| actions::clear_files(&this.stores, cx))),
        )
        .child(divider(cx))
        .child(icon_button(
          "toolbar-group-mode",
          IconName::Images,
          Some(group_color),
          tr!(group_mode_label_key(&group_mode)),
          true,
        ))
        .child(
          icon_button(
            "toolbar-group-by-file-name",
            IconName::Files,
            highlight(group_by_file_name),
            tr!("groupMode.byFileName"),
            true,
          )
          .on_click(cx.listener(|this, _, _, cx| {
            this.stores.config.update(cx, |config, cx| {
              config.update(cx, |config| config.group_by_file_name = !config.group_by_file_name)
            });
          })),
        )
        .child(icon_button(
          "toolbar-profile",
          IconName::User,
          None,
          tr!("toolbar.profile"),
          has_profiles,
        ))
        .child(
          icon_button(
            "toolbar-settings",
            IconName::Settings,
            highlight(active_tab == TabType::Settings),
            tr!("toolbar.settings"),
            true,
          )
          .on_click(cx.listener(|this, _, _, cx| this.stores.ui.update(cx, |ui, cx| ui.open_settings(cx)))),
        )
        .child(divider(cx))
        .child(
          icon_button(
            "toolbar-about",
            IconName::Info,
            highlight(active_tab == TabType::About),
            tr!("toolbar.about"),
            true,
          )
          .on_click(cx.listener(|this, _, _, cx| this.stores.ui.update(cx, |ui, cx| ui.open_about(cx)))),
        ),
    )
  }
}
