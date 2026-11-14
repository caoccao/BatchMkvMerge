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

//! The card of one merge unit (`MkvFileCard.tsx`): the status icon, the
//! root path with the child-count badge (a popover lists the other members,
//! each removable), the summary, the actions (output path, Open in
//! BetterMediaInfo, Copy Command, Merge, Delete, the drag handle), the
//! progress with `elapsed / ETA` and Cancel while active, and the track
//! table.

use std::rc::Rc;
use std::time::SystemTime;

use bmm_core::media_track::media_track_counts;
use bmm_core::merge_args::file_name;
use gpui_kit::assets::IconName;
use gpui_kit::base::{TestSupportExt as _, TestTextExt as _};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{CardDrag, CardHeight, ChildBadge, DragChip, card_summary, icon_button, middle_ellipsis, status_icon};
use crate::actions::{self, MergeRequest, OutputTarget, resolve_merge_output_for};
use crate::assets::BETTER_MEDIA_INFO_ICON;
use crate::domain::format::{format_encoded_date, format_file_size};
use crate::domain::status::QueueItemStatus;
use crate::state::Stores;
use crate::state::files::FileEvent;
use crate::state::queue::QueueEvent;
use crate::ui::output_dialog::{OutputDialogRequest, OutputMode, OutputPathDialog};
use crate::ui::track_table::TrackTable;

/// Rows the child list shows before it scrolls, and their height.
const CHILD_VISIBLE_ROWS: f32 = 5.0;
const CHILD_ROW_HEIGHT: f32 = 36.0;

pub struct FileCard {
  stores: Stores,
  /// The unit, root first.
  members: Vec<String>,
  table: Entity<TrackTable>,
  children_open: bool,
  height: CardHeight,
  renders: usize,
  _subscriptions: Vec<Subscription>,
}

impl FileCard {
  pub fn new(stores: Stores, members: Vec<String>, table: Entity<TrackTable>, cx: &mut Context<Self>) -> Self {
    let subscriptions = vec![
      // Structural changes: parsed files, BetterMediaInfo found.
      cx.observe(&stores.files, |_, _, cx| cx.notify()),
      cx.subscribe(&stores.files, |this, _, event: &FileEvent, cx| {
        let own = match event {
          FileEvent::Changed { files } => files.iter().any(|file| this.members.contains(file)),
          FileEvent::Highlight { ids } => ids.iter().any(|id| id == this.root()),
        };
        if own {
          cx.notify();
        }
      }),
      cx.subscribe(&stores.queue, |this, queue, event: &QueueEvent, cx| match event {
        QueueEvent::ItemChanged { file } if file == this.root() => cx.notify(),
        QueueEvent::Tick
          if queue
            .read(cx)
            .item(this.root())
            .is_some_and(|item| item.status == QueueItemStatus::Merging) =>
        {
          cx.notify()
        }
        _ => {}
      }),
    ];
    Self {
      stores,
      members,
      table,
      children_open: false,
      height: CardHeight::default(),
      renders: 0,
      _subscriptions: subscriptions,
    }
  }

  pub fn root(&self) -> &str {
    &self.members[0]
  }

  pub fn members(&self) -> &[String] {
    &self.members
  }

  pub fn table(&self) -> &Entity<TrackTable> {
    &self.table
  }

  pub fn height(&self) -> &CardHeight {
    &self.height
  }

  /// How many times the card rendered (the repaint check).
  pub fn render_count(&self) -> usize {
    self.renders
  }

  pub fn is_children_open(&self) -> bool {
    self.children_open
  }

  /// The unit gained or lost members.
  pub fn set_members(&mut self, members: Vec<String>, cx: &mut Context<Self>) {
    if self.members != members {
      if members.len() < 2 {
        self.children_open = false;
      }
      self.members = members;
      cx.notify();
    }
  }

  /// The element id of the card of `root`.
  pub fn element_id(root: &str) -> ElementId {
    ElementId::Name(format!("card:{root}").into())
  }

  fn request(&self, cx: &App) -> Option<MergeRequest> {
    MergeRequest::unit_selection(self.stores.files.read(cx), &self.members)
  }

  fn merge(&mut self, cx: &mut Context<Self>) {
    if let Some(request) = self.request(cx) {
      actions::enqueue(&self.stores, vec![request], cx).detach();
    }
  }

  fn copy_command(&mut self, cx: &mut Context<Self>) {
    let requests = self.request(cx).into_iter().collect();
    actions::copy_commands(&self.stores, requests, cx).detach();
  }

  /// Set Output Path: the dialog in file mode, starting from the override
  /// or the path Merge would write (Q&A: the same rule as Merge).
  fn open_output_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let root = self.root().to_string();
    let files = self.stores.files.read(cx);
    let current = files.output_path(&root).map(str::to_string);
    let target = OutputTarget::of(files, &root);
    let store = self.stores.files.clone();
    cx.spawn_in(window, async move |_, cx| {
      let initial = match current {
        Some(path) => path,
        None => {
          let root = root.clone();
          cx.background_spawn(async move { resolve_merge_output_for(&target, &root) })
            .await
        }
      };
      let on_confirm = {
        let root = root.clone();
        Rc::new(move |value: String, cx: &mut App| {
          store.update(cx, |files, cx| {
            if value.is_empty() {
              files.clear_file_output_path(&root, cx);
            } else {
              files.set_file_output_path(&root, value, cx);
            }
          });
        })
      };
      let request = OutputDialogRequest {
        title: tr!("merge.setOutputPath"),
        initial,
        mode: OutputMode::File,
        default_file_name: file_name(&root).to_string(),
        on_confirm,
      };
      let _ = cx.update(|window, cx| OutputPathDialog::open(request, window, cx));
    })
    .detach();
  }

  /// Takes a member out of the unit; the list stays open for the next one
  /// and closes with the last.
  fn remove_child(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.members.len() <= 2 {
      self.children_open = false;
    }
    self
      .stores
      .files
      .update(cx, |files, cx| files.detach_file_from_root(file, cx));
    cx.notify();
  }

  fn render_children(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let (border, muted, danger) = (theme.border, theme.muted_foreground, theme.danger);
    let viewport = window.viewport_size().width;
    let header = |text: SharedString| div().text_color(muted).child(text);
    let rows: Vec<AnyElement> = self.members[1..]
      .iter()
      .enumerate()
      .map(|(index, file)| {
        let path: SharedString = file.clone().into();
        let removed = file.clone();
        h_flex()
          .id(("child-row", index))
          .h(px(CHILD_ROW_HEIGHT))
          .border_t_1()
          .border_color(border)
          .child(
            super::text((index + 1).to_string())
              .w(px(40.0))
              .flex_none()
              .px_1()
              .text_right(),
          )
          .child(
            div()
              .id(("child-path", index))
              .flex_1()
              .min_w_0()
              .max_w((viewport * 0.8).min(px(680.0)))
              .px_2()
              .child(middle_ellipsis(file))
              .test_text(file.clone())
              .tooltip(move |window, cx| Tooltip::new(path.clone()).build(window, cx)),
          )
          .child(
            div().w(px(48.0)).flex_none().flex().justify_center().child(
              icon_button(
                ("child-remove", index),
                IconName::CircleMinus,
                Some(danger),
                tr!("merge.removeFromRoot"),
                true,
              )
              .on_click(cx.listener(move |this, _, _, cx| this.remove_child(&removed, cx))),
            ),
          )
          .into_any_element()
      })
      .collect();
    v_flex()
      .id("child-table")
      .test_support()
      .max_w((viewport * 0.9).min(px(760.0)))
      .text_size(crate::ui::text::body2())
      .child(
        h_flex()
          .h(px(CHILD_ROW_HEIGHT))
          .font_weight(FontWeight::MEDIUM)
          .child(
            div()
              .w(px(40.0))
              .flex_none()
              .px_1()
              .text_right()
              .child(header(tr!("merge.childTable.id"))),
          )
          .child(div().flex_1().px_2().child(header(tr!("merge.childTable.filePath"))))
          .child(
            div()
              .w(px(48.0))
              .flex_none()
              .flex()
              .justify_center()
              .child(header(tr!("merge.childTable.action"))),
          ),
      )
      .child(
        v_flex()
          .id("child-rows")
          .max_h(px(CHILD_VISIBLE_ROWS * CHILD_ROW_HEIGHT))
          .overflow_y_scroll()
          .children(rows),
      )
  }
}

impl Render for FileCard {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    self.renders += 1;
    let root = self.root().to_string();
    let files = self.stores.files.read(cx);
    let item = self.stores.queue.read(cx).item(&root).cloned();
    let status = item.as_ref().map(|item| item.status);
    let merging = status == Some(QueueItemStatus::Merging);
    let queued = status == Some(QueueItemStatus::Waiting);
    let active = merging || queued;
    let has_selection = self
      .members
      .iter()
      .any(|file| files.selected_ids(file).is_some_and(|ids| !ids.is_empty()));
    let counts = media_track_counts(self.members.iter().filter_map(|file| files.tracks(file)).flatten());
    let metadata = files.metadata(&root);
    let size = metadata.map(|metadata| format_file_size(metadata.file_size as f64));
    let encoded = format_encoded_date(metadata.and_then(|metadata| metadata.container.properties.date_utc.as_deref()));
    let title = metadata.and_then(|metadata| metadata.container.properties.title.clone());
    let output = files.output_path(&root).map(str::to_string);
    let summary = card_summary(
      Some(&counts),
      output.as_deref(),
      size.as_deref(),
      encoded.as_deref(),
      title.as_deref(),
    );
    let drop_target = files.drop_target_root() == Some(root.as_str());
    let active_card = files.active_card() == Some(root.as_str());
    let better_media_info = files.better_media_info_available();
    let child_count = self.members.len() - 1;

    let theme = cx.theme();
    let (border, primary, muted, danger) = (theme.border, theme.primary, theme.muted_foreground, theme.danger);
    let background = if drop_target {
      theme.drop_target
    } else if queued {
      theme.list_hover
    } else {
      theme.background
    };

    let path = super::text(root.clone()).id("card-path").test_support().min_w_0();
    let title_row = h_flex()
      .items_center()
      .gap(crate::ui::space(0.5))
      .children(status_icon(status, cx))
      .map(|row| {
        if child_count == 0 {
          return row.child(path);
        }
        let label = tr!("merge.childCount", count = child_count);
        let children = cx.entity().downgrade();
        row
          .child(
            path
              .mr_1()
              .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx)),
          )
          .child(
            Popover::new("card-children-popover")
              .open(self.children_open)
              .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.children_open = *open;
                cx.notify();
              }))
              .trigger(ChildBadge::new("card-children", child_count))
              .content(move |_, window, cx| {
                children
                  .upgrade()
                  .map(|card| card.update(cx, |card, cx| card.render_children(window, cx).into_any_element()))
                  .unwrap_or_else(|| div().into_any_element())
              }),
          )
      });

    let output_color = if output.is_some() { primary } else { muted };
    let actions = h_flex()
      .flex_none()
      .items_center()
      .gap(crate::ui::space(0.5))
      .child(
        icon_button(
          "card-output",
          IconName::FolderOpen,
          Some(output_color),
          tr!("merge.setOutputPath"),
          !active,
        )
        .on_click(cx.listener(|this, _, window, cx| this.open_output_dialog(window, cx))),
      )
      .when(better_media_info, |actions| {
        actions.child(
          Button::new("card-bmi")
            .ghost()
            .small()
            .accessibility_label(tr!("merge.openInBetterMediaInfo"))
            .tooltip(tr!("merge.openInBetterMediaInfo"))
            .child(img(BETTER_MEDIA_INFO_ICON).size(px(20.0)))
            .on_click(cx.listener(|this, _, _, cx| {
              let roots = vec![this.root().to_string()];
              actions::open_in_better_media_info(&this.stores, roots, cx).detach();
            })),
        )
      })
      .child(
        icon_button(
          "card-copy",
          IconName::Copy,
          None,
          tr!("merge.copyCommand"),
          has_selection && !active,
        )
        .on_click(cx.listener(|this, _, _, cx| this.copy_command(cx))),
      )
      .child(
        Button::new("card-merge")
          .outline()
          .small()
          .icon(IconName::Combine)
          .label(tr!("merge.merge"))
          .disabled(!has_selection || active)
          .on_click(cx.listener(|this, _, _, cx| this.merge(cx))),
      )
      .child(
        icon_button(
          "card-delete",
          IconName::Trash,
          Some(danger),
          tr!("list.delete"),
          !merging,
        )
        .on_click(cx.listener(|this, _, _, cx| {
          let members = this.members.clone();
          actions::delete_unit(&this.stores, &members, cx);
        })),
      )
      .child(drag_handle(&root, active, cx));

    let progress = active.then(|| {
      let now = SystemTime::now();
      h_flex()
        .id("card-progress")
        .test_support()
        .items_center()
        .gap(crate::ui::space(1.0))
        .px(crate::ui::space(2.0))
        .pb(crate::ui::space(1.0))
        .mt(crate::ui::space(1.0))
        .map(|row| {
          if merging {
            row
              .child(super::progress_bar(item.as_ref().map_or(0, |item| item.progress), cx))
              .child(super::mono_caption(super::time_label(item.as_ref(), now), cx))
          } else {
            row.child(div().flex_1())
          }
        })
        .child(
          icon_button(
            "card-cancel",
            IconName::CircleX,
            Some(danger),
            tr!("merge.cancel"),
            true,
          )
          .on_click(cx.listener(|this, _, _, cx| {
            let root = this.root().to_string();
            actions::cancel_merge(&this.stores, &root, cx);
          })),
        )
    });

    v_flex()
      .id(Self::element_id(&root))
      .test_support()
      .relative()
      .w_full()
      .border_1()
      .rounded(px(4.0))
      .border_color(if drop_target || active_card { primary } else { border })
      .bg(background)
      .text_size(crate::ui::text::body2())
      .child(
        h_flex()
          .items_start()
          .gap(crate::ui::space(1.0))
          .px(crate::ui::space(2.0))
          .pt(crate::ui::space(2.0))
          .pb(crate::ui::space(if active { 0.0 } else { 1.0 }))
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .gap(px(2.0))
              .child(title_row)
              .children(summary.map(|summary| {
                super::text(summary)
                  .id("card-summary")
                  .test_support()
                  .text_size(crate::ui::text::caption())
                  .text_color(muted)
              })),
          )
          .child(actions),
      )
      .children(progress)
      .child(
        div()
          .px(crate::ui::space(2.0))
          .pb(crate::ui::space(2.0))
          .child(self.table.clone()),
      )
      .when(drop_target, |card| {
        card.child(
          div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .rounded(px(4.0))
            .border_2()
            .border_dashed()
            .border_color(primary),
        )
      })
      .child(self.height.probe())
  }
}

/// The drag handle (`DragIndicator`): dragging it onto another file card
/// merges this card into that one. Inactive while the card merges or waits.
fn drag_handle(root: &str, active: bool, cx: &App) -> impl IntoElement {
  let theme = cx.theme();
  let color = if active {
    theme.muted_foreground.opacity(0.5)
  } else {
    theme.muted_foreground
  };
  let label = tr!("merge.dragToMerge");
  let handle = div()
    .id("card-drag")
    .test_support()
    .flex()
    .items_center()
    .justify_center()
    .size(px(24.0))
    .rounded(px(4.0))
    .child(Icon::new(IconName::GripVertical).small().text_color(color))
    .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx));
  if active {
    return handle.into_any_element();
  }
  let hover = theme.secondary_hover;
  handle
    .cursor_grab()
    .hover(move |style| style.bg(hover))
    .on_drag(CardDrag { root: root.to_string() }, |drag, offset, _, cx| {
      cx.new(|_| DragChip::new(&drag.root).at(offset))
    })
    .into_any_element()
}
