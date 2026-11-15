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

//! A checkbox with an indeterminate state, for select-all headers. gpui-kit's
//! checkbox has no such state; gpui-base's has, so this one is drawn like
//! gpui-kit's, with a minus mark.

use gpui_kit::assets::IconName;
use gpui_kit::base::{self as kit_base, CheckboxIndicator, CheckboxState};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// `on_change` gets `true` when the box becomes checked.
pub fn tri_state_checkbox(
  id: impl Into<ElementId>,
  state: CheckboxState,
  disabled: bool,
  label: SharedString,
  cx: &App,
  on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
  let theme = cx.theme();
  let on = state != CheckboxState::Unchecked;
  let fill = if disabled {
    theme.primary.opacity(0.5)
  } else {
    theme.primary
  };
  let border = if on {
    fill
  } else {
    theme.input.opacity(if disabled { 0.5 } else { 1.0 })
  };
  let mark = match state {
    CheckboxState::Checked => Some(IconName::Check),
    CheckboxState::Indeterminate => Some(IconName::Minus),
    CheckboxState::Unchecked => None,
  };
  let (mark_color, background) = (theme.primary_foreground, theme.background);
  kit_base::Checkbox::new(id)
    .state(state)
    .disabled(disabled)
    .accessibility_label(label)
    .on_change(move |state, _, window, cx| on_change(state == CheckboxState::Checked, window, cx))
    .child(
      CheckboxIndicator::new()
        .state(state)
        .size(rems(0.875))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .rounded(px(4.0))
        .border_color(border)
        .when(on, |indicator| indicator.bg(fill))
        .when(!on, |indicator| indicator.bg(background))
        .children(mark.map(|mark| Icon::new(mark).size(px(10.0)).text_color(mark_color))),
    )
}

/// The state of a select-all box over `count` of `total` checked rows.
pub fn select_all_state(count: usize, total: usize) -> CheckboxState {
  if total > 0 && count == total {
    CheckboxState::Checked
  } else if count > 0 {
    CheckboxState::Indeterminate
  } else {
    CheckboxState::Unchecked
  }
}
