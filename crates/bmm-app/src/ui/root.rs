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

//! The window content (`Layout.tsx`): toolbar, tabs and footer, plus what
//! belongs to the whole window: OS file drop, toasts, geometry persistence,
//! the title with the queue count (`App.tsx`), live language and theme
//! changes, and the close request.

use std::time::Duration;

use bmm_core::config::{DisplayMode, Language, Theme as ThemeName};
use gpui_kit::base::TestSupportExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, WindowExt, v_flex};
use gpui_kit::*;

use super::footer::Footer;
use super::main_content::MainContent;
use super::toolbar::Toolbar;
use crate::platform::window as main_window;
use crate::state::ui::{NotificationKind, UiEvent};
use crate::state::{ConfigStore, Stores, files};

/// How long the window must stay still before its geometry is saved.
const GEOMETRY_SAVE_DELAY: Duration = Duration::from_millis(500);

pub struct AppView {
  stores: Stores,
  toolbar: Entity<Toolbar>,
  main: Entity<MainContent>,
  /// The applied language, theme and display mode, to act only on changes.
  language: Language,
  theme: (ThemeName, DisplayMode),
  /// The title last set on the window.
  title: String,
  save_geometry: Option<Task<()>>,
  _subscriptions: Vec<Subscription>,
}

impl AppView {
  pub fn new(stores: Stores, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let config = stores.config.read(cx).config().clone();
    let (language, theme) = (config.language, (config.theme, config.display_mode));
    let subscriptions = vec![
      cx.observe_in(&stores.config, window, Self::config_changed),
      cx.observe_window_appearance(window, |this, window, cx| {
        if this.theme.1 == DisplayMode::Auto {
          let config = this.stores.config.read(cx).config().clone();
          crate::theme::apply(&config, window.appearance(), cx);
        }
      }),
      cx.observe_window_bounds(window, |this, window, cx| this.schedule_geometry_save(window, cx)),
      // Done and total change only with a status, which notifies.
      cx.observe_in(&stores.queue, window, |this, queue, window, cx| {
        let title = main_window::title(&queue.read(cx).summary());
        if title != this.title {
          window.set_window_title(&title);
          this.title = title;
        }
      }),
      cx.subscribe_in(&stores.ui, window, |_, _, event: &UiEvent, window, cx| {
        let UiEvent::Toast(toast) = event;
        let notification = match toast.kind {
          NotificationKind::Success => Notification::success(toast.detail.clone()),
          NotificationKind::Error => Notification::error(toast.detail.clone()),
        };
        window.push_notification(notification.title(toast.file.clone()).placement(Anchor::TopCenter), cx);
      }),
    ];
    let close_stores = stores.clone();
    window.on_window_should_close(cx, move |window, cx| {
      main_window::close_requested(&close_stores, window, cx)
    });
    // The system appearance as this window sees it (more reliable on Linux
    // than the application-wide value used before the window existed).
    if theme.1 == DisplayMode::Auto {
      crate::theme::apply(&config, window.appearance(), cx);
    }
    Self {
      toolbar: cx.new(|cx| Toolbar::new(stores.clone(), cx)),
      main: cx.new(|cx| MainContent::new(stores.clone(), cx)),
      stores,
      language,
      theme,
      title: crate::window_title().to_string(),
      save_geometry: None,
      _subscriptions: subscriptions,
    }
  }

  pub fn stores(&self) -> &Stores {
    &self.stores
  }

  pub fn main(&self) -> &Entity<MainContent> {
    &self.main
  }

  /// Applies a changed language, theme or display mode at once.
  fn config_changed(&mut self, config: Entity<ConfigStore>, window: &mut Window, cx: &mut Context<Self>) {
    let config = config.read(cx).config().clone();
    if config.language != self.language {
      self.language = config.language;
      crate::i18n::set_language(config.language);
      cx.refresh_windows();
    }
    let theme = (config.theme, config.display_mode);
    if theme != self.theme {
      self.theme = theme;
      crate::theme::apply(&config, window.appearance(), cx);
    }
  }

  /// Saves the geometry once the window has been still for a moment.
  fn schedule_geometry_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.save_geometry = Some(cx.spawn_in(window, async move |this, cx| {
      cx.background_executor().timer(GEOMETRY_SAVE_DELAY).await;
      let _ = this.update_in(cx, |this, window, cx| {
        main_window::save_geometry(&this.stores, window, cx);
        this.save_geometry = None;
      });
    }));
  }
}

impl Render for AppView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    v_flex()
      .id("app-root")
      .test_support()
      .size_full()
      .px(super::space(1.0))
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
        let paths = paths
          .paths()
          .iter()
          .map(|path| path.to_string_lossy().into_owned())
          .collect();
        files::add_paths(&this.stores.files, paths, cx);
      }))
      .child(self.toolbar.clone())
      .child(div().flex_1().min_h_0().child(self.main.clone()))
      .child(Footer)
  }
}
