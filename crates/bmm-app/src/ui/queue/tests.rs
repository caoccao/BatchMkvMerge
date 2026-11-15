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

//! Headless tests of the Queue tab, the completion notifications, the
//! topmost window and the taskbar progress in the application shell.

use bmm_core::merge::{MergeEvent, MergeOutcome};
use core::prelude::v1::test;
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;

use super::*;
use crate::state::TabType;
use crate::ui::tests::{Shell, finished, movies, started};

const A: &str = "/media/a.mkv";
const B: &str = "/media/b.mkv";
const C: &str = "/media/c.mkv";
const D: &str = "/media/d.mkv";
const E: &str = "/media/e.mkv";

fn progress(file: &str, percent: u32) -> MergeEvent {
  MergeEvent::Progress {
    file: file.to_string(),
    percent,
  }
}

/// The shell on the Queue tab with `files` parsed and merged (Merge All).
fn queued(cx: &mut TestAppContext, files: &[&str]) -> Shell {
  let shell = Shell::with_files(cx, movies(files));
  shell.add_parsed(cx, files);
  shell.press(cx, "f3");
  shell
    .stores
    .ui
    .update(cx, |ui, cx| ui.set_active_tab(TabType::Queue, cx));
  cx.run_until_parked();
  shell
}

/// The texts painted inside the element `within` in a fresh frame.
fn texts(shell: &Shell, cx: &mut TestAppContext, within: impl Into<ElementId>) -> Vec<String> {
  let within = within.into();
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .filter(|text| text.path().contains(&within))
      .map(|text| text.text().to_string())
      .collect()
  })
}

fn row_texts(shell: &Shell, cx: &mut TestAppContext, file: &str) -> Vec<String> {
  texts(shell, cx, QueueRow::element_id(file))
}

/// Whether `id` is on screen inside `within`, and its disabled state.
fn control(shell: &Shell, cx: &mut TestAppContext, within: impl Into<ElementId>, id: &str) -> Option<bool> {
  let (within, id) = (within.into(), SharedString::from(id.to_string()));
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window
      .within(within)
      .try_find(id)
      .map(|control| control.disabled() == Some(true))
  })
}

fn click(shell: &Shell, cx: &mut TestAppContext, id: &str) {
  let id = SharedString::from(id.to_string());
  shell.window(cx, |window, cx| {
    window.render_frame(cx);
    window.click(id, cx);
  });
  cx.run_until_parked();
}

/// The drive key of the test paths: `default` except on Windows, where a
/// rooted path without a drive letter has no prefix either.
fn drive() -> String {
  bmm_core::merge::get_drive_key(A)
}

fn id(name: &str) -> String {
  format!("{name}:{}", drive())
}

fn statuses(shell: &Shell, cx: &mut TestAppContext) -> Vec<Option<QueueItemStatus>> {
  [A, B, C, D, E].iter().map(|file| shell.status(cx, file)).collect()
}

fn system_notifications(cx: &mut TestAppContext) -> Vec<(String, String)> {
  cx.shown_system_notifications()
    .into_iter()
    .map(|notification| (notification.title.to_string(), notification.body.to_string()))
    .collect()
}

fn topmost(shell: &Shell, cx: &mut TestAppContext) -> (usize, Option<AnyWindowHandle>) {
  shell
    .view
    .read_with(cx, |view, _| (view.topmost().opened(), view.topmost().handle()))
}

/// The texts of the topmost window, if it is open.
fn topmost_texts(cx: &mut TestAppContext, handle: AnyWindowHandle) -> Option<Vec<String>> {
  cx.update_window(handle, |_, window, cx| {
    window.render_frame(cx);
    gpui_kit::base::test_support::texts(window)
      .into_iter()
      .map(|text| text.text().to_string())
      .collect()
  })
  .ok()
}

#[gpui_kit::test]
fn an_empty_queue_shows_one_card_with_its_actions(cx: &mut TestAppContext) {
  let shell = Shell::with_files(cx, movies(&[]));
  shell
    .stores
    .ui
    .update(cx, |ui, cx| ui.set_active_tab(TabType::Queue, cx));
  cx.run_until_parked();
  let shown = texts(&shell, cx, "queue");
  for expected in ["Queue", "File Path", "Status", "Start", "End", "Elapsed", "ETA"] {
    assert!(shown.iter().any(|text| text == expected), "{expected} in {shown:?}");
  }
  // The toggles are always available; the queue actions are not.
  assert_eq!(control(&shell, cx, "queue", "queue-system:"), Some(false));
  assert_eq!(control(&shell, cx, "queue", "queue-topmost:"), Some(false));
  for action in ["queue-resume:", "queue-cancel-all:", "queue-clear:"] {
    assert_eq!(control(&shell, cx, "queue", action), Some(true), "{action}");
  }
}

#[gpui_kit::test]
fn rows_show_each_status_with_its_label_and_times(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B, C, D, E]);
  shell.send(cx, started(A));
  shell.send(cx, progress(A, 17));
  for (file, outcome, error) in [
    (B, MergeOutcome::Completed, None),
    (C, MergeOutcome::Failed, Some("mkvmerge exited with code 2")),
    (D, MergeOutcome::Cancelled, None),
  ] {
    shell.send(cx, started(file));
    shell.send(cx, finished(file, outcome, error));
  }
  let shown = texts(&shell, cx, id("queue-drive"));
  let title = drive_label(&drive());
  assert!(shown.iter().any(|text| text.as_str() == title.as_ref()), "{shown:?}");
  let expected = [
    (A, "Merging 17%"),
    (B, "Completed"),
    (C, "Failed"),
    (D, "Cancelled"),
    (E, "Waiting"),
  ];
  for (file, status) in expected {
    let row = row_texts(&shell, cx, file);
    assert_eq!(row[0], file, "{row:?}");
    assert_eq!(row[1], status, "{row:?}");
  }
  // A waiting row knows no time; a merging one has a start but no end or
  // ETA before its progress means anything; a finished one has both ends.
  assert_eq!(row_texts(&shell, cx, E)[2..], ["--:--:--"; 4]);
  let merging = row_texts(&shell, cx, A);
  assert_ne!(merging[2], "--:--:--", "a start");
  assert_eq!(merging[3], "--:--:--", "no end");
  let completed = row_texts(&shell, cx, B);
  assert!(completed[2..5].iter().all(|time| time != "--:--:--"), "{completed:?}");
  assert_eq!(completed[5], "--:--:--", "no ETA once finished");
  // Cancel only on the merging row.
  assert_eq!(
    control(&shell, cx, QueueRow::element_id(A), "queue-cancel"),
    Some(false)
  );
  for file in [B, C, D, E] {
    assert_eq!(
      control(&shell, cx, QueueRow::element_id(file), "queue-cancel"),
      None,
      "{file}"
    );
  }
  click(&shell, cx, "queue-cancel");
  assert_eq!(shell.cancelled(), [A]);
}

#[gpui_kit::test]
fn one_card_per_drive(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B, C]);
  shell.stores.queue.update(cx, |queue, cx| {
    queue.move_to_drive(B, "D:", cx);
    queue.move_to_drive(C, "\\\\SERVER\\SHARE", cx);
  });
  cx.run_until_parked();
  let shown = texts(&shell, cx, "queue");
  for title in [
    drive_label(&drive()).to_string(),
    "D:".to_string(),
    "\\\\SERVER\\SHARE".to_string(),
  ] {
    assert_eq!(
      shown.iter().filter(|text| **text == title).count(),
      1,
      "{title} in {shown:?}"
    );
  }
  for (drive, file) in [
    (drive(), A),
    ("D:".to_string(), B),
    ("\\\\SERVER\\SHARE".to_string(), C),
  ] {
    let card = texts(&shell, cx, format!("queue-drive:{drive}"));
    assert!(card.iter().any(|text| text == file), "{file} in {card:?}");
    assert_eq!(card.iter().filter(|text| text.starts_with("/media/")).count(), 1);
  }
  // Cancel All of one drive cancels only its files.
  click(&shell, cx, "queue-cancel-all:D:");
  assert_eq!(shell.cancelled(), [B]);
}

#[gpui_kit::test]
fn resume_all_merges_again_only_cancelled_and_failed_roots(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B, C, D]);
  assert_eq!(control(&shell, cx, "queue", &id("queue-resume")), Some(true));
  shell.send(cx, started(A));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, finished(B, MergeOutcome::Cancelled, None));
  shell.send(cx, started(C));
  shell.send(cx, finished(C, MergeOutcome::Failed, Some("boom")));
  shell.send(cx, started(D));
  assert_eq!(control(&shell, cx, "queue", &id("queue-resume")), Some(false));
  click(&shell, cx, &id("queue-resume"));
  assert_eq!(
    shell.enqueued(),
    [A, B, C, D, B, C],
    "only B and C again, in queue order"
  );
  assert_eq!(
    statuses(&shell, cx)[..4],
    [
      Some(QueueItemStatus::Completed),
      Some(QueueItemStatus::Waiting),
      Some(QueueItemStatus::Waiting),
      Some(QueueItemStatus::Merging),
    ]
  );
  // A file no longer listed is not resumed.
  shell.send(cx, finished(B, MergeOutcome::Cancelled, None));
  cx.update(|cx| crate::actions::remove_file(&shell.stores, B, cx));
  shell.send(cx, finished(B, MergeOutcome::Cancelled, None));
  click(&shell, cx, &id("queue-resume"));
  assert_eq!(shell.enqueued().len(), 6, "B is gone, C still waits");
}

#[gpui_kit::test]
fn clear_completed_removes_only_finished_rows(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B, C, D]);
  assert_eq!(control(&shell, cx, "queue", &id("queue-clear")), Some(true));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, finished(B, MergeOutcome::Failed, Some("boom")));
  shell.send(cx, finished(C, MergeOutcome::Cancelled, None));
  click(&shell, cx, &id("queue-clear"));
  assert_eq!(
    statuses(&shell, cx)[..4],
    [None, None, None, Some(QueueItemStatus::Waiting)]
  );
  let shown = texts(&shell, cx, "queue");
  assert!(!shown.iter().any(|text| text == A), "{shown:?}");
  assert!(shown.iter().any(|text| text == D), "{shown:?}");
  assert_eq!(control(&shell, cx, "queue", &id("queue-clear")), Some(true));
}

/// With both notifications on, three queued merges send one OS notification
/// and open the topmost window once, after the third finishes; the in-app
/// toast stays per file.
#[gpui_kit::test]
fn three_merges_notify_once_after_the_last(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B, C]);
  click(&shell, cx, &id("queue-system"));
  click(&shell, cx, &id("queue-topmost"));
  shell.stores.ui.read_with(cx, |ui, _| {
    assert!(ui.system_notification() && ui.topmost_notification());
  });
  shell.send(cx, started(A));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, started(B));
  shell.send(cx, finished(B, MergeOutcome::Failed, Some("boom")));
  assert_eq!(system_notifications(cx), [], "two of three finished");
  assert_eq!(topmost(&shell, cx), (0, None));
  shell.send(cx, started(C));
  shell.send(cx, finished(C, MergeOutcome::Completed, None));
  assert_eq!(
    system_notifications(cx),
    [(
      "Merge complete.".to_string(),
      "2 completed, 1 failed, 0 cancelled\nFinished in 00:00:00".to_string()
    )]
  );
  let (opened, handle) = topmost(&shell, cx);
  assert_eq!(opened, 1);
  let handle = handle.expect("the topmost window");
  assert_eq!(
    topmost_texts(cx, handle).expect("open")[..3],
    [
      "Merge complete.",
      "2 completed, 1 failed, 0 cancelled",
      "Finished in 00:00:00"
    ]
  );
  assert_eq!(shell.toasts(cx), 3, "a toast per finished file");

  // The next batch of one file reuses the open window with the single-file
  // text.
  click(&shell, cx, &id("queue-resume"));
  shell.send(cx, started(B));
  shell.send(cx, finished(B, MergeOutcome::Completed, None));
  assert_eq!(system_notifications(cx).len(), 2);
  assert_eq!(topmost(&shell, cx), (1, Some(handle)), "reused");
  let shown = topmost_texts(cx, handle).expect("open");
  assert_eq!(shown[..3], ["Merge complete.", B, "Completed in 00:00:00"]);

  // Escape closes it; the next completion opens a new one.
  cx.update_window(handle, |_, window, cx| {
    window.render_frame(cx);
    window.press("escape", cx);
  })
  .unwrap();
  cx.run_until_parked();
  assert!(topmost_texts(cx, handle).is_none(), "closed");
  shell.press(cx, "f3");
  for file in [A, B, C] {
    shell.send(cx, started(file));
    shell.send(cx, finished(file, MergeOutcome::Completed, None));
  }
  let (opened, reopened) = topmost(&shell, cx);
  assert_eq!(opened, 2);
  assert_ne!(reopened, Some(handle));
}

#[gpui_kit::test]
fn nothing_notifies_with_the_toggles_off_or_a_cancelled_batch(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B]);
  shell.send(cx, started(A));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, finished(B, MergeOutcome::Completed, None));
  assert_eq!(system_notifications(cx), [], "both toggles are off");
  assert_eq!(topmost(&shell, cx).0, 0);
  click(&shell, cx, &id("queue-system"));
  click(&shell, cx, &id("queue-topmost"));
  click(&shell, cx, &id("queue-resume"));
  assert_eq!(shell.enqueued().len(), 2, "nothing to resume");
  shell.press(cx, "f3");
  shell.send(cx, finished(A, MergeOutcome::Cancelled, None));
  shell.send(cx, finished(B, MergeOutcome::Cancelled, None));
  assert_eq!(system_notifications(cx), [], "the user cancelled everything");
  assert_eq!(topmost(&shell, cx).0, 0);
}

#[gpui_kit::test]
fn closing_mid_merge_sends_no_completion(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B]);
  click(&shell, cx, &id("queue-system"));
  click(&shell, cx, &id("queue-topmost"));
  shell.send(cx, started(A));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, started(B));
  let stores = shell.stores.clone();
  shell.window(cx, |window, cx| {
    crate::platform::window::close_requested(&stores, window, cx)
  });
  cx.run_until_parked();
  click(&shell, cx, "exit-yes");
  assert_eq!(shell.cancelled(), [B]);
  shell.send(cx, finished(B, MergeOutcome::Cancelled, None));
  assert_eq!(system_notifications(cx), []);
  assert_eq!(topmost(&shell, cx).0, 0);
}

#[gpui_kit::test]
fn the_taskbar_follows_the_whole_queue(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B]);
  shell.send(cx, started(A));
  shell.send(cx, progress(A, 50));
  shell.send(cx, finished(A, MergeOutcome::Completed, None));
  shell.send(cx, started(B));
  shell.send(cx, progress(B, 40));
  shell.send(cx, progress(B, 41));
  shell.send(cx, finished(B, MergeOutcome::Failed, Some("boom")));
  // Each value once: 0 while both wait, the mean of both progresses, then
  // hidden.
  assert_eq!(
    shell.taskbar.borrow().as_slice(),
    [Some(0), Some(25), Some(50), Some(70), None]
  );
}

#[gpui_kit::test]
fn only_the_merging_row_repaints(cx: &mut TestAppContext) {
  let shell = queued(cx, &[A, B]);
  let draw = |cx: &mut TestAppContext| shell.window(cx, |window, cx| window.draw(cx).clear(cx));
  let rows = shell.view.read_with(cx, |view, cx| {
    let queue = view.main().read(cx).queue().read(cx);
    (queue.row(A).cloned().unwrap(), queue.row(B).cloned().unwrap())
  });
  let counts = |cx: &mut TestAppContext| cx.read(|cx| (rows.0.read(cx).render_count(), rows.1.read(cx).render_count()));
  draw(cx);
  draw(cx);
  let before = counts(cx);
  draw(cx);
  assert_eq!(counts(cx), before, "an idle frame reuses both rows");
  shell.send(cx, started(B));
  draw(cx);
  draw(cx);
  for event in [progress(B, 10), progress(B, 20)] {
    let (a_before, b_before) = counts(cx);
    shell.send(cx, event);
    draw(cx);
    draw(cx);
    let (a_after, b_after) = counts(cx);
    assert!(b_after > b_before, "the merging row repaints");
    assert_eq!(a_after, a_before, "the waiting row does not");
  }
  // The clock tick repaints only the merging row too.
  let (a_before, b_before) = counts(cx);
  cx.executor().advance_clock(crate::state::queue::TICK);
  cx.run_until_parked();
  draw(cx);
  let (a_after, b_after) = counts(cx);
  assert!(b_after > b_before);
  assert_eq!(a_after, a_before);
}
