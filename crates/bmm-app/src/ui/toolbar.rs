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

//! The toolbar: icon buttons with tooltips and their enabled states, and the
//! group-mode and profile menus, which F8 and F9 open too. A menu's open state
//! lives in [`crate::state::UiState`], so the shortcuts reach it; the menu
//! itself is a gpui-kit `PopupMenu` in a controlled `Popover`, built when it
//! opens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bmm_core::config::GroupMode;
use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, h_flex};
use gpui_kit::*;

use crate::actions;
use crate::state::ui::ToolbarMenu;
use crate::state::{Stores, TabType};
use crate::ui::output_dialog::{OutputDialogRequest, OutputMode, OutputPathDialog};

/// The group modes in menu order.
pub const GROUP_MODES: [GroupMode; 3] = [GroupMode::None, GroupMode::TrackCount, GroupMode::TrackCountAndLanguage];

/// The i18n key naming a group mode.
pub fn group_mode_label_key(mode: &GroupMode) -> &'static str {
  match mode {
    GroupMode::None => "groupMode.none",
    GroupMode::TrackCount => "groupMode.trackCount",
    GroupMode::TrackCountAndLanguage => "groupMode.trackCountAndLanguage",
  }
}

/// The open menus' entities; a menu is built when it opens and dropped when
/// it closes, so it always lists the current choices.
type Menus = Rc<RefCell<HashMap<ToolbarMenu, Entity<PopupMenu>>>>;

pub struct Toolbar {
  stores: Stores,
  menus: Menus,
  _subscriptions: Vec<Subscription>,
}

impl Toolbar {
  pub fn new(stores: Stores, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.observe(&stores.config, |_, _, cx| cx.notify()),
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      cx.observe(&stores.queue, |_, _, cx| cx.notify()),
      cx.observe(&stores.ui, |this, ui, cx| {
        let open = ui.read(cx).open_menu();
        this.menus.borrow_mut().retain(|menu, _| Some(*menu) == open);
        cx.notify();
      }),
    ];
    Self {
      stores,
      menus: Menus::default(),
      _subscriptions: subscriptions,
    }
  }

  /// The menu's entity while it is open (tests).
  pub fn menu(&self, menu: ToolbarMenu) -> Option<Entity<PopupMenu>> {
    self.menus.borrow().get(&menu).cloned()
  }
}

/// The choices of a toolbar menu, the current one checked.
fn build_menu(stores: &Stores, menu: ToolbarMenu, window: &mut Window, cx: &mut App) -> Entity<PopupMenu> {
  let config = stores.config.read(cx).config().clone();
  let store = stores.config.clone();
  PopupMenu::build(window, cx, move |popup, _, _| match menu {
    ToolbarMenu::GroupMode => GROUP_MODES.iter().fold(popup, |popup, mode| {
      let (mode, store) = (mode.clone(), store.clone());
      popup.item(
        PopupMenuItem::new(tr!(group_mode_label_key(&mode)))
          .checked(mode == config.group_mode)
          .on_click(move |_, _, cx| {
            let mode = mode.clone();
            store.update(cx, |store, cx| store.update(cx, |config| config.group_mode = mode));
          }),
      )
    }),
    ToolbarMenu::Profile => config.profiles.iter().fold(popup, |popup, profile| {
      let (name, store) = (profile.name.clone(), store.clone());
      popup.item(
        PopupMenuItem::new(profile.name.clone())
          .checked(profile.name == config.active_profile)
          .on_click(move |_, _, cx| store.update(cx, |store, cx| store.set_active_profile(&name, cx))),
      )
    }),
  })
}

/// `trigger` with its menu below it, open while [`UiState`] says so; a
/// click on the trigger opens it, a choice, Escape or a click elsewhere
/// closes it.
///
/// [`UiState`]: crate::state::UiState
fn menu_popover(stores: &Stores, menus: &Menus, menu: ToolbarMenu, trigger: Button, cx: &App) -> Popover {
  let open = stores.ui.read(cx).open_menu() == Some(menu);
  let ui = stores.ui.clone();
  let (stores, menus) = (stores.clone(), menus.clone());
  Popover::new(SharedString::from(format!("toolbar-menu-{}", menu.name())))
    .appearance(false)
    .overlay_closable(false)
    .trigger(trigger)
    .open(open)
    .on_open_change(move |open, _, cx| {
      ui.update(cx, |ui, cx| {
        if *open {
          ui.show_menu(menu, cx);
        } else {
          ui.hide_menu(menu, cx);
        }
      })
    })
    .content(move |_, window, cx| {
      if let Some(popup) = menus.borrow().get(&menu).cloned() {
        return popup;
      }
      let popup = build_menu(&stores, menu, window, cx);
      popup.focus_handle(cx).focus(window, cx);
      let (ui, closed) = (stores.ui.clone(), menus.clone());
      window
        .subscribe(&popup, cx, move |_, _: &DismissEvent, _, cx| {
          closed.borrow_mut().remove(&menu);
          ui.update(cx, |ui, cx| ui.hide_menu(menu, cx));
        })
        .detach();
      menus.borrow_mut().insert(menu, popup.clone());
      popup
    })
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

impl Toolbar {
  /// The session's default output directory: the output dialog in directory
  /// mode; empty clears it.
  fn open_global_output_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let files = self.stores.files.clone();
    let request = OutputDialogRequest {
      title: tr!("toolbar.setGlobalOutputPath"),
      initial: files.read(cx).global_output_dir().unwrap_or_default().to_string(),
      mode: OutputMode::Directory,
      default_file_name: String::new(),
      on_confirm: Rc::new(move |value: String, cx: &mut App| {
        files.update(cx, |files, cx| files.set_global_output_dir(value, cx));
      }),
    };
    OutputPathDialog::open(request, window, cx);
  }
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
    let can_merge_all = actions::can_merge_all(&self.stores, cx);
    let group_color = match group_mode {
      GroupMode::None => muted.opacity(0.6),
      GroupMode::TrackCountAndLanguage => success,
      GroupMode::TrackCount => primary,
    };
    let highlight = |on: bool| on.then_some(primary);

    h_flex().id("toolbar").test_support().gap(super::space(1.0)).child(
      h_flex()
        .items_center()
        .child(
          icon_button(
            "toolbar-global-output",
            IconName::FolderCog,
            Some(if has_global_output { primary } else { muted }),
            tr!("toolbar.setGlobalOutputPath"),
            true,
          )
          .on_click(cx.listener(|this, _, window, cx| this.open_global_output_dialog(window, cx))),
        )
        .child(
          icon_button(
            "toolbar-merge-all",
            IconName::Combine,
            None,
            tr!("toolbar.mergeAll"),
            can_merge_all,
          )
          .on_click(cx.listener(|this, _, _, cx| actions::merge_all(&this.stores, cx).detach())),
        )
        .child(divider(cx))
        .child(
          icon_button(
            "toolbar-cancel-all",
            IconName::CircleX,
            Some(danger),
            tr!("toolbar.cancelAll"),
            has_active_jobs,
          )
          .on_click(cx.listener(|this, _, _, cx| actions::cancel_all(&this.stores, cx))),
        )
        .child(
          icon_button("toolbar-clear", IconName::Trash, None, tr!("toolbar.clear"), can_clear)
            .on_click(cx.listener(|this, _, _, cx| actions::clear_files(&this.stores, cx))),
        )
        .child(divider(cx))
        .child(menu_popover(
          &self.stores,
          &self.menus,
          ToolbarMenu::GroupMode,
          icon_button(
            "toolbar-group-mode",
            IconName::Images,
            Some(group_color),
            tr!(group_mode_label_key(&group_mode)),
            true,
          ),
          cx,
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
        .child(menu_popover(
          &self.stores,
          &self.menus,
          ToolbarMenu::Profile,
          icon_button(
            "toolbar-profile",
            IconName::User,
            None,
            tr!("toolbar.profile"),
            has_profiles,
          ),
          cx,
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
