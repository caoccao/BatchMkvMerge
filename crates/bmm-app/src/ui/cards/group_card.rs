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

//! The card of structurally identical units: the directory and summary, the
//! actions for every unit (output directory, Open in BetterMediaInfo, Copy All
//! Commands, Merge All, Clear All), and a split pane: the units' roots with
//! their status and progress on the left, resizable, and the representative
//! track table on the right, whose edits go to the same member position of
//! every unit.

use std::cell::Cell;
use std::rc::Rc;
use std::time::SystemTime;

use bmm_core::media_track::media_track_counts;
use bmm_core::merge_args::{file_name, parent_dir};
use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{CardHeight, ChildBadge, card_summary, icon_button, mono_caption, progress_bar, status_icon, time_label};
use crate::actions::{self, MergeRequest};
use crate::assets::BETTER_MEDIA_INFO_ICON;
use crate::domain::status::QueueItemStatus;
use crate::state::Stores;
use crate::state::files::FileEvent;
use crate::state::queue::QueueEvent;
use crate::ui::output_dialog::{OutputDialogRequest, OutputMode, OutputPathDialog};
use crate::ui::text_button;
use crate::ui::track_table::TrackTable;

/// The root list's starting width and its limits: at least 160 px, and at
/// least 240 px left for the table.
pub const ROOT_LIST_WIDTH: f32 = 240.0;
pub const ROOT_LIST_MIN_WIDTH: f32 = 160.0;
pub const TABLE_MIN_WIDTH: f32 = 240.0;
const SPLITTER_WIDTH: f32 = 6.0;

/// The root list's width for a pointer at `x` over a split pane spanning
/// `left..left + width`.
pub fn root_list_width(x: Pixels, left: Pixels, width: Pixels) -> Pixels {
  let min = px(ROOT_LIST_MIN_WIDTH);
  let max = (width - px(TABLE_MIN_WIDTH)).max(min);
  (x - left).max(min).min(max)
}

pub struct GroupCard {
  stores: Stores,
  units: Vec<Vec<String>>,
  table: Entity<TrackTable>,
  root_list_width: Pixels,
  resizing: bool,
  /// The split pane's bounds from the last frame.
  split: Rc<Cell<Bounds<Pixels>>>,
  height: CardHeight,
  renders: usize,
  _subscriptions: Vec<Subscription>,
}

impl GroupCard {
  pub fn new(stores: Stores, units: Vec<Vec<String>>, table: Entity<TrackTable>, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      cx.subscribe(&stores.files, |this, _, event: &FileEvent, cx| {
        let own = match event {
          FileEvent::Changed { files } => files
            .iter()
            .any(|file| this.units.iter().flatten().any(|own| own == file)),
          FileEvent::Highlight { ids } => ids.iter().any(|id| id == this.id()),
        };
        if own {
          cx.notify();
        }
      }),
      cx.subscribe(&stores.queue, |this, queue, event: &QueueEvent, cx| match event {
        QueueEvent::ItemChanged { file } if this.roots().any(|root| root == file) => cx.notify(),
        QueueEvent::Tick => {
          let queue = queue.read(cx);
          if this.roots().any(|root| {
            queue
              .item(root)
              .is_some_and(|item| item.status == QueueItemStatus::Merging)
          }) {
            cx.notify();
          }
        }
        _ => {}
      }),
    ];
    Self {
      stores,
      units,
      table,
      root_list_width: px(ROOT_LIST_WIDTH),
      resizing: false,
      split: Rc::default(),
      height: CardHeight::default(),
      renders: 0,
      _subscriptions: subscriptions,
    }
  }

  /// The card's id: its first root (the active-card id).
  pub fn id(&self) -> &str {
    &self.units[0][0]
  }

  pub fn units(&self) -> &[Vec<String>] {
    &self.units
  }

  pub fn roots(&self) -> impl Iterator<Item = &str> {
    self.units.iter().map(|unit| unit[0].as_str())
  }

  pub fn table(&self) -> &Entity<TrackTable> {
    &self.table
  }

  pub fn height(&self) -> &CardHeight {
    &self.height
  }

  pub fn render_count(&self) -> usize {
    self.renders
  }

  pub fn root_list_width(&self) -> Pixels {
    self.root_list_width
  }

  pub fn set_units(&mut self, units: Vec<Vec<String>>, cx: &mut Context<Self>) {
    if self.units != units {
      self.units = units;
      cx.notify();
    }
  }

  /// The element id of the group card whose first root is `id`.
  pub fn element_id(id: &str) -> ElementId {
    ElementId::Name(format!("group:{id}").into())
  }

  fn roots_vec(&self) -> Vec<String> {
    self.roots().map(str::to_string).collect()
  }

  /// One request per unit with a selection, in order.
  fn requests(&self, cx: &App) -> Vec<MergeRequest> {
    let files = self.stores.files.read(cx);
    self
      .units
      .iter()
      .filter_map(|unit| MergeRequest::group_unit(files, unit))
      .collect()
  }

  /// The output directory every root shares, if any.
  fn group_output_dir(&self, cx: &App) -> Option<String> {
    let files = self.stores.files.read(cx);
    let first = files.output_dir(self.id()).filter(|dir| !dir.is_empty())?;
    self
      .roots()
      .all(|root| files.output_dir(root) == Some(first))
      .then(|| first.to_string())
  }

  fn open_output_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let roots = self.roots_vec();
    let store = self.stores.files.clone();
    let initial = self
      .group_output_dir(cx)
      .unwrap_or_else(|| parent_dir(self.id()).to_string());
    let request = OutputDialogRequest {
      title: tr!("merge.setOutputPath"),
      initial,
      mode: OutputMode::Directory,
      default_file_name: String::new(),
      on_confirm: Rc::new(move |value: String, cx: &mut App| {
        store.update(cx, |files, cx| {
          if value.is_empty() {
            files.clear_group_output_dir(&roots, cx);
          } else {
            files.set_group_output_dir(&roots, &value, cx);
          }
        });
      }),
    };
    OutputPathDialog::open(request, window, cx);
  }

  fn resize_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
    let split = self.split.get();
    let width = root_list_width(x, split.left(), split.size.width);
    if width != self.root_list_width {
      self.root_list_width = width;
      cx.notify();
    }
  }

  fn end_resize(&mut self, cx: &mut Context<Self>) {
    if self.resizing {
      self.resizing = false;
      cx.notify();
    }
  }

  /// While resizing, the pointer may leave the splitter and the card.
  fn render_resize_listeners(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
    if !self.resizing {
      return None;
    }
    let entity = cx.entity();
    Some(
      canvas(
        |_, _, _| {},
        move |_, _, window, _| {
          let moves = entity.clone();
          window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
              moves.update(cx, |card, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                  card.resize_to(event.position.x, cx);
                } else {
                  card.end_resize(cx);
                }
              });
            }
          });
          let ups = entity.clone();
          window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
              ups.update(cx, |card, cx| card.end_resize(cx));
            }
          });
        },
      )
      .absolute()
      .top_0()
      .left_0()
      .size_full(),
    )
  }

  fn render_roots(&self, now: SystemTime, cx: &mut Context<Self>) -> impl IntoElement {
    let queue = self.stores.queue.read(cx);
    let rows: Vec<AnyElement> = self
      .units
      .iter()
      .enumerate()
      .map(|(index, unit)| {
        let root = unit[0].clone();
        let item = queue.item(&root).cloned();
        let status = item.as_ref().map(|item| item.status);
        let merging = status == Some(QueueItemStatus::Merging);
        let progress = item.as_ref().map_or(0, |item| item.progress);
        let danger = cx.theme().danger;
        let name = super::text(file_name(&root).to_string()).min_w_0();
        v_flex()
          .id(("group-unit", index))
          .test_support()
          .w_full()
          .py(crate::ui::space(0.5))
          .px(crate::ui::space(2.0))
          .gap(crate::ui::space(0.5))
          .child(
            h_flex()
              .w_full()
              .items_center()
              .gap(crate::ui::space(0.5))
              .children(status_icon(status, cx))
              .map(|row| {
                if unit.len() > 1 {
                  row
                    .child(name.mr_1())
                    .child(ChildBadge::new(("group-unit-children", index), unit.len() - 1).passive())
                } else {
                  row.child(name.flex_1())
                }
              }),
          )
          .when(merging, |row| {
            row
              .child(
                h_flex()
                  .w_full()
                  .items_center()
                  .gap(crate::ui::space(0.5))
                  .child(progress_bar(progress, cx))
                  .child(mono_caption(format!("{progress}%"), cx).min_w(px(34.0)).text_right())
                  .child(
                    icon_button(
                      ("group-cancel", index),
                      IconName::CircleX,
                      Some(danger),
                      tr!("merge.cancel"),
                      true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| actions::cancel_merge(&this.stores, &root, cx))),
                  ),
              )
              .child(mono_caption(time_label(item.as_ref(), now), cx).w_full())
          })
          .into_any_element()
      })
      .collect();
    v_flex().id("group-roots").test_support().w_full().children(rows)
  }
}

impl Render for GroupCard {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    self.renders += 1;
    let id = self.id().to_string();
    let files = self.stores.files.read(cx);
    let queue = self.stores.queue.read(cx);
    let statuses: Vec<Option<QueueItemStatus>> = self
      .roots()
      .map(|root| queue.item(root).map(|item| item.status))
      .collect();
    let any_active = statuses.iter().flatten().any(|status| status.is_active());
    let any_waiting = statuses.contains(&Some(QueueItemStatus::Waiting));
    let representative = &self.units[0];
    let has_selection = representative
      .iter()
      .any(|file| files.selected_ids(file).is_some_and(|ids| !ids.is_empty()));
    let counts = media_track_counts(representative.iter().filter_map(|file| files.tracks(file)).flatten());
    let better_media_info = files.better_media_info_available();
    let active_card = files.active_card() == Some(id.as_str());
    let output_dir = self.group_output_dir(cx);
    let summary = card_summary(Some(&counts), output_dir.as_deref(), None, None, None);
    let now = SystemTime::now();

    let theme = cx.theme();
    let (border, primary, muted, danger, divider, divider_hover) = (
      theme.border,
      theme.primary,
      theme.muted_foreground,
      theme.danger,
      theme.border,
      theme.muted,
    );
    let background = if any_waiting {
      theme.list_hover
    } else {
      theme.background
    };
    let output_color = if output_dir.is_some() { primary } else { muted };

    let header = h_flex()
      .items_center()
      .gap(crate::ui::space(1.0))
      .mb(crate::ui::space(1.0))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .ml(crate::ui::space(2.0))
          .gap(px(2.0))
          .child(
            super::text(parent_dir(&id).to_string())
              .id("group-dir")
              .test_support()
              .text_color(muted),
          )
          .children(summary.map(|summary| {
            super::text(summary)
              .id("group-summary")
              .test_support()
              .text_size(crate::ui::text::caption())
              .text_color(muted)
          })),
      )
      .child(
        icon_button(
          "group-output",
          IconName::FolderOpen,
          Some(output_color),
          tr!("merge.setOutputPath"),
          !any_active,
        )
        .on_click(cx.listener(|this, _, window, cx| this.open_output_dialog(window, cx))),
      )
      .when(better_media_info, |header| {
        header.child(
          Button::new("group-bmi")
            .ghost()
            .small()
            .accessibility_label(tr!("merge.openInBetterMediaInfo"))
            .tooltip(tr!("merge.openInBetterMediaInfo"))
            .child(img(BETTER_MEDIA_INFO_ICON).size(px(20.0)))
            .on_click(cx.listener(|this, _, _, cx| {
              let roots = this.roots_vec();
              actions::open_in_better_media_info(&this.stores, roots, cx).detach();
            })),
        )
      })
      .child(
        icon_button(
          "group-copy",
          IconName::Copy,
          None,
          tr!("group.copyAllCommands"),
          has_selection,
        )
        .on_click(cx.listener(|this, _, _, cx| {
          let requests = this.requests(cx);
          actions::copy_commands(&this.stores, requests, cx).detach();
        })),
      )
      .child(
        text_button("group-merge")
          .outline()
          .small()
          .icon(IconName::Combine)
          .label(tr!("group.mergeAll"))
          .tooltip(tr!("group.mergeAll"))
          .disabled(!has_selection || any_active)
          .on_click(cx.listener(|this, _, _, cx| {
            let requests = this.requests(cx);
            actions::enqueue(&this.stores, requests, cx);
          })),
      )
      .child(
        icon_button(
          "group-clear",
          IconName::Trash,
          Some(danger),
          tr!("group.clearAll"),
          !any_active,
        )
        .on_click(cx.listener(|this, _, _, cx| {
          let units = this.units.clone();
          actions::clear_units(&this.stores, &units, cx);
        })),
      );

    let split = self.split.clone();
    let body = h_flex()
      .id("group-split")
      .relative()
      .w_full()
      .items_start()
      .child(
        div()
          .flex_none()
          .w(self.root_list_width)
          .overflow_hidden()
          .child(self.render_roots(now, cx)),
      )
      .child(
        div()
          .id("group-splitter")
          .test_support()
          .flex_none()
          .self_stretch()
          .w(px(SPLITTER_WIDTH))
          .cursor_col_resize()
          .bg(if self.resizing { divider_hover } else { divider })
          .hover(move |style| style.bg(divider_hover))
          .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
              this.resizing = true;
              cx.stop_propagation();
              cx.notify();
            }),
          ),
      )
      .child(
        div()
          .flex_1()
          .min_w_0()
          .ml(crate::ui::space(1.0))
          .child(self.table.clone()),
      )
      .child(
        canvas(move |bounds, _, _| split.set(bounds), |_, _, _, _| {})
          .absolute()
          .top_0()
          .left_0()
          .size_full(),
      );

    v_flex()
      .id(Self::element_id(&id))
      .test_support()
      .relative()
      .w_full()
      .p(crate::ui::space(1.0))
      .border_1()
      .rounded(px(4.0))
      .border_color(if active_card { primary } else { border })
      .bg(background)
      .text_size(crate::ui::text::body2())
      // A quick release before the window listeners start.
      .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_resize(cx)))
      .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_resize(cx)))
      .child(header)
      .child(body)
      .children(self.render_resize_listeners(cx))
      .child(self.height.probe())
  }
}
