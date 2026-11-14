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

//! The file list's cards: [`FileCard`] (`MkvFileCard.tsx`, one merge unit)
//! and [`GroupCard`] (`GroupCard.tsx`, structurally identical units edited
//! in lock-step), with what both show: the queue status icon
//! (`FileStatusIcon.tsx`), the summary line (`CardSummary.tsx`), the merge
//! progress and the card drag (`cardDrag.ts`).
//!
//! A card is an entity that repaints itself when one of its files or queue
//! items changes ([`FileEvent`](crate::state::files::FileEvent),
//! [`QueueEvent`](crate::state::queue::QueueEvent)); the file list draws it
//! as a cached view at its measured height ([`CardHeight`]), so a progress
//! tick re-renders one card, not the list.

pub mod file_card;
pub mod group_card;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;
use std::time::SystemTime;

use bmm_core::media_track::MediaTrackCounts;
use gpui_kit::assets::IconName;
use gpui_kit::base::{ObservedTextElement, TestSupportExt as _, TestTextExt as _};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Disableable, Icon, Selectable, Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub use file_card::FileCard;
pub use group_card::GroupCard;

use crate::domain::format::{format_hms, format_track_counts};
use crate::domain::status::{QueueItem, QueueItemStatus};

/// Pixel size of the status icon (`FileStatusIcon`'s default 18).
const STATUS_ICON_SIZE: f32 = 18.0;
/// Trailing characters a middle-truncated path always shows.
const PATH_TAIL_CHARS: usize = 18;

/// A card being dragged onto another to merge into it (`cardDrag.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardDrag {
  /// The dragged card's root.
  pub root: String,
}

/// The chip next to the pointer during a card drag: the dragged file's name.
pub struct DragChip {
  label: SharedString,
  /// Where the pointer grabbed the handle; GPUI draws the preview that far
  /// up and left of the pointer.
  grab: Point<Pixels>,
}

impl DragChip {
  pub fn new(root: &str) -> Self {
    Self {
      label: bmm_core::merge_args::file_name(root).to_string().into(),
      grab: Point::default(),
    }
  }

  /// The chip for a drag grabbed at `offset` inside the handle.
  pub fn at(mut self, offset: Point<Pixels>) -> Self {
    self.grab = offset;
    self
  }
}

impl Render for DragChip {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    div()
      .ml(self.grab.x + px(12.0))
      .mt(self.grab.y + px(4.0))
      .px_2()
      .py_1()
      .rounded_sm()
      .bg(theme.popover)
      .text_color(theme.popover_foreground)
      .text_size(super::text::body2())
      .border_1()
      .border_color(theme.border)
      .shadow_md()
      .child(self.label.clone())
  }
}

/// The height a card measured while it last rendered, shared with the file
/// list. A cached view is laid out from a given style, not from its content,
/// so the list lays a card out at this height; a card whose height changes
/// asks for one more frame, in which the list uses the new height.
#[derive(Clone, Default)]
pub struct CardHeight(Rc<Cell<Option<Pixels>>>);

impl CardHeight {
  pub fn get(&self) -> Option<Pixels> {
    self.0.get()
  }

  /// An invisible element covering the card that records its height while
  /// the card prepaints.
  pub fn probe(&self) -> impl IntoElement {
    let height = self.0.clone();
    canvas(
      move |bounds, window, _| {
        if height.get() != Some(bounds.size.height) {
          height.set(Some(bounds.size.height));
          window.request_animation_frame();
        }
      },
      |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
  }
}

/// The queue status icon (`FileStatusIcon`): a spinner while merging, a
/// clock while waiting, a check when completed, an alert when failed,
/// nothing when cancelled or never queued.
pub fn status_icon(status: Option<QueueItemStatus>, cx: &App) -> Option<AnyElement> {
  let theme = cx.theme();
  let size = px(STATUS_ICON_SIZE);
  let icon = |name: IconName, color: Hsla| Icon::new(name).size(size).flex_none().text_color(color);
  Some(match status? {
    QueueItemStatus::Merging => div()
      .flex()
      .flex_none()
      .items_center()
      .justify_center()
      .size(size)
      .child(Spinner::new().xsmall())
      .into_any_element(),
    QueueItemStatus::Waiting => icon(IconName::Clock, theme.muted_foreground).into_any_element(),
    QueueItemStatus::Completed => icon(IconName::CircleCheck, theme.success).into_any_element(),
    QueueItemStatus::Failed => icon(IconName::CircleAlert, theme.danger).into_any_element(),
    QueueItemStatus::Cancelled => return None,
  })
}

/// The summary under a card's path (`CardSummary`): the track counts, the
/// output override, then the file's size, encoded date and title; `None`
/// when there is nothing to say.
pub fn card_summary(
  counts: Option<&MediaTrackCounts>,
  output: Option<&str>,
  size: Option<&str>,
  encoded: Option<&str>,
  title: Option<&str>,
) -> Option<String> {
  let mut pieces = Vec::new();
  if let Some(counts) = counts {
    let text = format_track_counts(counts);
    if !text.is_empty() {
      pieces.push(text);
    }
  }
  if let Some(path) = output.filter(|path| !path.is_empty()) {
    pieces.push(tr!("card.summary.toPath", path = path).to_string());
  }
  let details: Vec<String> = [
    size.map(|size| tr!("card.summary.size", size = size)),
    encoded.map(|date| tr!("card.summary.encoded", date = date)),
    title.map(|title| tr!("card.summary.title", title = title)),
  ]
  .into_iter()
  .flatten()
  .map(|text| text.to_string())
  .collect();
  if !details.is_empty() {
    pieces.push(details.join(", "));
  }
  (!pieces.is_empty()).then(|| pieces.join(" "))
}

/// A merging card's `elapsed / ETA`, each `--:--:--` while unknown.
pub fn time_label(item: Option<&QueueItem>, now: SystemTime) -> String {
  format!(
    "{} / {}",
    format_hms(item.and_then(|item| item.card_elapsed(now))),
    format_hms(item.and_then(|item| item.eta(now)))
  )
}

/// A merge's progress bar: 6 px, the success colour on a muted track.
pub fn progress_bar(progress: u32, cx: &App) -> impl IntoElement {
  let theme = cx.theme();
  let fraction = progress.min(100) as f32 / 100.0;
  div()
    .flex_1()
    .h(px(6.0))
    .rounded(px(4.0))
    .bg(theme.muted)
    .overflow_hidden()
    .child(div().h_full().w(relative(fraction)).bg(theme.success))
}

/// Monospace caption text (times, percentages).
pub fn mono_caption(text: impl Into<SharedString>, cx: &App) -> ObservedTextElement<Div> {
  let theme = cx.theme();
  let text = text.into();
  div()
    .flex_none()
    .font_family(theme.mono_font_family.clone())
    .text_size(super::text::caption())
    .text_color(theme.muted_foreground)
    .child(text.clone())
    .test_text(text)
}

/// A text element that tests can read back.
pub fn text(text: impl Into<SharedString>) -> ObservedTextElement<Div> {
  let text = text.into();
  div().child(text.clone()).test_text(text)
}

/// A path that keeps its start and its last 18 characters and loses the
/// middle to an ellipsis when it does not fit.
pub fn middle_ellipsis(path: &str) -> impl IntoElement {
  let split = path
    .char_indices()
    .rev()
    .nth(PATH_TAIL_CHARS - 1)
    .map(|(index, _)| index)
    .unwrap_or(0);
  let (head, tail) = path.split_at(split);
  h_flex()
    .w_full()
    .min_w_0()
    .child(div().flex_shrink(1.0).min_w_0().truncate().child(head.to_string()))
    .child(div().flex_none().whitespace_nowrap().child(tail.to_string()))
}

/// A small icon button with a tooltip that also names it for screen
/// readers and tests. The icon takes `color` only while the button is
/// enabled, so a disabled one dims like the others (the toolbar's rule).
pub fn icon_button(
  id: impl Into<ElementId>,
  icon: IconName,
  color: Option<Hsla>,
  label: SharedString,
  enabled: bool,
) -> Button {
  let icon = match color.filter(|_| enabled) {
    Some(color) => Icon::new(icon).text_color(color),
    None => Icon::new(icon),
  };
  Button::new(id)
    .ghost()
    .small()
    .icon(icon)
    .accessibility_label(label.clone())
    .tooltip(label)
    .disabled(!enabled)
}

/// The child-count badge of a multi-member unit (MUI's static `Badge`): a
/// primary pill. Selectable so that it can open a popover.
#[derive(IntoElement)]
pub struct ChildBadge {
  id: ElementId,
  count: usize,
  selected: bool,
  interactive: bool,
}

impl ChildBadge {
  pub fn new(id: impl Into<ElementId>, count: usize) -> Self {
    Self {
      id: id.into(),
      count,
      selected: false,
      interactive: true,
    }
  }

  /// A badge that only shows the count (the group card's root list).
  pub fn passive(mut self) -> Self {
    self.interactive = false;
    self
  }
}

impl Selectable for ChildBadge {
  fn selected(mut self, selected: bool) -> Self {
    self.selected = selected;
    self
  }

  fn is_selected(&self) -> bool {
    self.selected
  }
}

impl RenderOnce for ChildBadge {
  fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let theme = cx.theme();
    let (bg, hover) = (theme.primary, theme.primary_hover);
    let label = if self.count > 999 {
      "999+".to_string()
    } else {
      self.count.to_string()
    };
    div()
      .id(self.id)
      .test_support()
      .flex()
      .flex_none()
      .items_center()
      .justify_center()
      .h(px(15.0))
      .min_w(px(15.0))
      .px(px(4.0))
      .rounded_full()
      .bg(if self.selected { hover } else { bg })
      .when(self.interactive, |badge| {
        badge.cursor_pointer().hover(move |style| style.bg(hover))
      })
      .text_color(theme.primary_foreground)
      .text_size(px(8.0))
      .child(label)
  }
}
