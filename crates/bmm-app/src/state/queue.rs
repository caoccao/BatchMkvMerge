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

//! The merge queue as the UI shows it (`queueItems` / `queueOrder` of the
//! React store): one [`QueueItem`] per enqueued root file, in the order it was
//! first enqueued, kept current by `bmm_core::merge`'s events (see
//! [`crate::merge_events`]).
//!
//! A change notifies observers only when it is structural (an item added or
//! removed, or a status changed): the toolbar, the title and the close guard
//! care about that. Every change, progress included, is also emitted as
//! [`QueueEvent::ItemChanged`] naming the file, so a card or a queue row can
//! repaint alone.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use bmm_core::merge::{MergeEvent, MergeOutcome};
use gpui_kit::{Context, EventEmitter, Task};

use crate::domain::status::{QueueItem, QueueItemStatus, can_enqueue};

/// How often the elapsed and ETA labels move on while a merge runs: the
/// one-second resolution of `format_hms`.
pub const TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueEvent {
  /// The item of `file` changed (its progress only, too) or was removed.
  ItemChanged { file: String },
  /// A merge ended; `elapsed` runs from its start to its end (zero when it
  /// never started).
  Finished {
    file: String,
    outcome: MergeOutcome,
    error: Option<String>,
    elapsed: Duration,
  },
  /// A second passed while at least one merge runs.
  Tick,
}

/// The queue in numbers, for the window title and the taskbar progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueSummary {
  /// Completed, cancelled or failed items.
  pub done: usize,
  pub total: usize,
  /// Whether an item is waiting or merging.
  pub active: bool,
  /// The whole queue's progress, 0–100: a finished item counts 100, a merging
  /// one its progress, a waiting one 0; the mean is rounded down.
  pub percent: u32,
}

impl QueueSummary {
  fn of<'a>(items: impl IntoIterator<Item = &'a QueueItem>) -> Self {
    let mut summary = Self::default();
    let mut progress_sum: u64 = 0;
    for item in items {
      summary.total += 1;
      match item.status {
        QueueItemStatus::Waiting => summary.active = true,
        QueueItemStatus::Merging => {
          summary.active = true;
          progress_sum += u64::from(item.progress);
        }
        QueueItemStatus::Completed | QueueItemStatus::Cancelled | QueueItemStatus::Failed => {
          summary.done += 1;
          progress_sum += 100;
        }
      }
    }
    if summary.total > 0 {
      summary.percent = (progress_sum / summary.total as u64).min(100) as u32;
    }
    summary
  }
}

#[derive(Default)]
pub struct QueueStore {
  items: HashMap<String, QueueItem>,
  order: Vec<String>,
  /// Runs while an item is merging and emits [`QueueEvent::Tick`].
  ticker: Option<Task<()>>,
}

impl EventEmitter<QueueEvent> for QueueStore {}

impl QueueStore {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn item(&self, file: &str) -> Option<&QueueItem> {
    self.items.get(file)
  }

  /// The items in queue order.
  pub fn items(&self) -> impl Iterator<Item = &QueueItem> {
    self.order.iter().filter_map(|file| self.items.get(file))
  }

  /// Whether `file` may be enqueued: it is not waiting or merging.
  pub fn can_enqueue(&self, file: &str) -> bool {
    can_enqueue(self.items.get(file))
  }

  /// Whether a merge is waiting or running.
  pub fn has_active(&self) -> bool {
    self.items.values().any(|item| item.status.is_active())
  }

  /// The waiting and merging files, in queue order.
  pub fn active_files(&self) -> Vec<String> {
    self
      .items()
      .filter(|item| item.status.is_active())
      .map(|item| item.file.clone())
      .collect()
  }

  pub fn summary(&self) -> QueueSummary {
    QueueSummary::of(self.items.values())
  }

  /// Lists `file` as waiting (`addToQueue`). A file that is waiting or merging
  /// already is left alone (returns `false`); a finished one starts over in
  /// its old position.
  pub fn add_to_queue(&mut self, file: &str, cx: &mut Context<Self>) -> bool {
    if !self.can_enqueue(file) {
      return false;
    }
    if self.items.insert(file.to_string(), QueueItem::waiting(file)).is_none() {
      self.order.push(file.to_string());
    }
    self.changed(file, true, cx);
    true
  }

  /// Drops the item of `file` (`removeFromQueue`).
  pub fn remove_from_queue(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.items.remove(file).is_some() {
      self.order.retain(|listed| listed != file);
      self.changed(file, true, cx);
    }
  }

  /// Flags a cancel request on a waiting or merging item
  /// (`markCancelRequested`); the outcome still comes from the merge events.
  pub fn mark_cancel_requested(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.items.get_mut(file).is_some_and(QueueItem::request_cancel) {
      self.changed(file, false, cx);
    }
  }

  /// Applies one merge event (`recordFinishedOutcome` and the per-entry part
  /// of `applyMergeSnapshot`). An event for a file that is not listed lists
  /// it. `now` stands in for a time the event does not carry.
  pub fn apply_event(&mut self, event: &MergeEvent, now: SystemTime, cx: &mut Context<Self>) {
    let file = event.file();
    let structural = match self.items.get_mut(file) {
      Some(item) => {
        let status = item.status;
        if !item.apply(event, now) {
          return;
        }
        item.status != status
      }
      None => {
        self.items.insert(file.to_string(), QueueItem::from_event(event, now));
        self.order.push(file.to_string());
        true
      }
    };
    if let MergeEvent::Finished { outcome, error, at, .. } = event {
      let elapsed = self.items[file].elapsed(*at).unwrap_or_default();
      cx.emit(QueueEvent::Finished {
        file: file.to_string(),
        outcome: *outcome,
        error: error.clone(),
        elapsed,
      });
    }
    self.changed(file, structural, cx);
  }

  /// Removes the finished items of one drive (`clearCompletedInDrive`).
  pub fn clear_completed_in_drive(&mut self, drive: &str, cx: &mut Context<Self>) {
    let removed: Vec<String> = self
      .items()
      .filter(|item| item.drive == drive && item.status.is_terminal())
      .map(|item| item.file.clone())
      .collect();
    for file in &removed {
      self.items.remove(file);
    }
    self.order.retain(|file| !removed.contains(file));
    for file in removed {
      self.changed(&file, true, cx);
    }
  }

  /// Removes every item (the queue part of `clearFiles`).
  pub fn clear(&mut self, cx: &mut Context<Self>) {
    let removed = std::mem::take(&mut self.order);
    self.items.clear();
    for file in removed {
      self.changed(&file, true, cx);
    }
  }

  fn changed(&mut self, file: &str, structural: bool, cx: &mut Context<Self>) {
    cx.emit(QueueEvent::ItemChanged { file: file.to_string() });
    if structural {
      cx.notify();
    }
    self.update_ticker(cx);
  }

  /// Starts the ticker when a merge runs and stops it when none does.
  fn update_ticker(&mut self, cx: &mut Context<Self>) {
    let merging = self.items.values().any(|item| item.status == QueueItemStatus::Merging);
    if !merging {
      self.ticker = None;
    } else if self.ticker.is_none() {
      self.ticker = Some(cx.spawn(async move |this, cx| {
        loop {
          cx.background_executor().timer(TICK).await;
          if this.update(cx, |_, cx| cx.emit(QueueEvent::Tick)).is_err() {
            break;
          }
        }
      }));
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use core::prelude::v1::test;
  use gpui_kit::{AppContext, Entity, TestAppContext};
  use std::cell::RefCell;
  use std::rc::Rc;

  const A: &str = "/media/a.mkv";
  const B: &str = "/media/b.mkv";

  fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + seconds)
  }

  fn started(file: &str, seconds: u64) -> MergeEvent {
    MergeEvent::Started {
      file: file.to_string(),
      at: at(seconds),
    }
  }

  fn progress(file: &str, percent: u32) -> MergeEvent {
    MergeEvent::Progress {
      file: file.to_string(),
      percent,
    }
  }

  fn finished(file: &str, outcome: MergeOutcome, seconds: u64) -> MergeEvent {
    MergeEvent::Finished {
      file: file.to_string(),
      outcome,
      error: (outcome == MergeOutcome::Failed).then(|| "mkvmerge exited with code Some(2)".to_string()),
      at: at(seconds),
    }
  }

  /// The store, the events it emitted and how often it notified.
  struct Harness {
    queue: Entity<QueueStore>,
    events: Rc<RefCell<Vec<QueueEvent>>>,
    notified: Rc<RefCell<usize>>,
    _subscriptions: Vec<gpui_kit::Subscription>,
  }

  impl Harness {
    fn new(cx: &mut TestAppContext) -> Self {
      let queue = cx.new(|_| QueueStore::new());
      let events = Rc::new(RefCell::new(Vec::new()));
      let notified = Rc::new(RefCell::new(0));
      let (event_sink, notify_sink) = (events.clone(), notified.clone());
      let subscriptions = cx.update(|cx| {
        vec![
          cx.subscribe(&queue, move |_, event: &QueueEvent, _| {
            event_sink.borrow_mut().push(event.clone())
          }),
          cx.observe(&queue, move |_, _| *notify_sink.borrow_mut() += 1),
        ]
      });
      Self {
        queue,
        events,
        notified,
        _subscriptions: subscriptions,
      }
    }

    fn apply(&self, cx: &mut TestAppContext, event: MergeEvent) {
      self
        .queue
        .update(cx, |queue, cx| queue.apply_event(&event, at(1_000), cx));
    }

    fn status(&self, cx: &mut TestAppContext, file: &str) -> Option<QueueItemStatus> {
      self
        .queue
        .read_with(cx, |queue, _| queue.item(file).map(|item| item.status))
    }

    fn take_events(&self) -> Vec<QueueEvent> {
      std::mem::take(&mut self.events.borrow_mut())
    }

    fn take_notified(&self) -> usize {
      std::mem::take(&mut self.notified.borrow_mut())
    }
  }

  fn changed(file: &str) -> QueueEvent {
    QueueEvent::ItemChanged { file: file.to_string() }
  }

  #[gpui_kit::test]
  fn a_scripted_event_sequence_drives_each_outcome(cx: &mut TestAppContext) {
    for outcome in [MergeOutcome::Completed, MergeOutcome::Cancelled, MergeOutcome::Failed] {
      let harness = Harness::new(cx);
      harness.queue.update(cx, |queue, cx| assert!(queue.add_to_queue(A, cx)));
      assert_eq!(harness.status(cx, A), Some(QueueItemStatus::Waiting));
      harness.apply(cx, started(A, 10));
      assert_eq!(harness.status(cx, A), Some(QueueItemStatus::Merging));
      harness.apply(cx, progress(A, 40));
      harness.apply(cx, finished(A, outcome, 70));
      let item = harness.queue.read_with(cx, |queue, _| queue.item(A).cloned().unwrap());
      assert_eq!(item.status, QueueItemStatus::from(outcome));
      assert_eq!(item.progress, if outcome == MergeOutcome::Completed { 100 } else { 40 });
      assert_eq!(item.error.is_some(), outcome == MergeOutcome::Failed);
      // A terminal state is never overwritten by a late progress event.
      harness.apply(cx, progress(A, 80));
      assert_eq!(harness.status(cx, A), Some(QueueItemStatus::from(outcome)));
      let events = harness.take_events();
      assert!(events.contains(&QueueEvent::Finished {
        file: A.to_string(),
        outcome,
        error: item.error.clone(),
        elapsed: Duration::from_secs(60),
      }));
      // Waiting, merging, finished: three structural changes; progress is not.
      assert_eq!(harness.take_notified(), 3);
    }
  }

  #[gpui_kit::test]
  fn progress_is_emitted_without_notifying(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| queue.add_to_queue(A, cx));
    harness.apply(cx, started(A, 0));
    harness.take_events();
    harness.take_notified();
    harness.apply(cx, progress(A, 5));
    harness.apply(cx, progress(A, 6));
    assert_eq!(harness.take_events(), [changed(A), changed(A)]);
    assert_eq!(harness.take_notified(), 0);
  }

  #[gpui_kit::test]
  fn add_to_queue_skips_active_files_and_restarts_finished_ones(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      assert!(queue.add_to_queue(A, cx));
      assert!(queue.add_to_queue(B, cx));
      assert!(!queue.add_to_queue(A, cx), "already waiting");
    });
    harness.apply(cx, started(A, 0));
    harness
      .queue
      .update(cx, |queue, cx| assert!(!queue.add_to_queue(A, cx), "merging"));
    harness.apply(cx, finished(A, MergeOutcome::Failed, 5));
    harness.queue.update(cx, |queue, cx| {
      assert!(queue.add_to_queue(A, cx), "a failed file can merge again");
      let item = queue.item(A).unwrap();
      assert_eq!(item.status, QueueItemStatus::Waiting);
      assert_eq!((item.error.as_deref(), item.merge_started_at), (None, None));
      let order: Vec<&str> = queue.items().map(|item| item.file.as_str()).collect();
      assert_eq!(order, [A, B], "it keeps its position");
    });
  }

  #[gpui_kit::test]
  fn an_event_for_an_unlisted_file_lists_it(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.apply(cx, finished(B, MergeOutcome::Cancelled, 3));
    harness.apply(cx, started(A, 4));
    harness.queue.read_with(cx, |queue, _| {
      let order: Vec<(&str, QueueItemStatus)> = queue.items().map(|item| (item.file.as_str(), item.status)).collect();
      assert_eq!(order, [(B, QueueItemStatus::Cancelled), (A, QueueItemStatus::Merging)]);
    });
  }

  #[gpui_kit::test]
  fn cancel_requests_flag_only_active_items(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      queue.add_to_queue(A, cx);
      queue.mark_cancel_requested(A, cx);
      queue.mark_cancel_requested(B, cx);
      assert!(queue.item(A).unwrap().cancel_requested);
      assert!(queue.item(B).is_none());
    });
    harness.apply(cx, finished(A, MergeOutcome::Cancelled, 1));
    harness.take_events();
    harness.queue.update(cx, |queue, cx| queue.mark_cancel_requested(A, cx));
    assert_eq!(harness.take_events(), [], "a finished item is left alone");
  }

  #[gpui_kit::test]
  fn removing_and_clearing(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      queue.add_to_queue(A, cx);
      queue.add_to_queue(B, cx);
      queue.remove_from_queue(A, cx);
      queue.remove_from_queue(A, cx);
      assert!(queue.item(A).is_none());
      assert_eq!(queue.items().count(), 1);
      queue.clear(cx);
      assert_eq!(queue.items().count(), 0);
      assert!(!queue.has_active());
    });
  }

  #[gpui_kit::test]
  fn clear_completed_removes_only_finished_items_of_the_drive(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    let c = "/media/c.mkv";
    harness.queue.update(cx, |queue, cx| {
      for file in [A, B, c] {
        queue.add_to_queue(file, cx);
      }
    });
    harness.apply(cx, finished(A, MergeOutcome::Completed, 1));
    harness.apply(cx, finished(c, MergeOutcome::Failed, 2));
    let drive = bmm_core::merge::get_drive_key(A);
    harness.queue.update(cx, |queue, cx| {
      queue.clear_completed_in_drive("another drive", cx);
      assert_eq!(queue.items().count(), 3);
      queue.clear_completed_in_drive(&drive, cx);
      let left: Vec<&str> = queue.items().map(|item| item.file.as_str()).collect();
      assert_eq!(left, [B], "the waiting item stays");
      assert_eq!(queue.active_files(), [B]);
    });
  }

  #[gpui_kit::test]
  fn the_summary_counts_finished_items_and_averages_progress(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    assert_eq!(
      harness.queue.read_with(cx, |queue, _| queue.summary()),
      QueueSummary::default()
    );
    let c = "/media/c.mkv";
    harness.queue.update(cx, |queue, cx| {
      for file in [A, B, c] {
        queue.add_to_queue(file, cx);
      }
    });
    harness.apply(cx, finished(A, MergeOutcome::Cancelled, 1));
    harness.apply(cx, started(B, 1));
    harness.apply(cx, progress(B, 51));
    let summary = harness.queue.read_with(cx, |queue, _| queue.summary());
    assert_eq!(
      summary,
      QueueSummary {
        done: 1,
        total: 3,
        active: true,
        // (100 + 51 + 0) / 3, rounded down.
        percent: 50,
      }
    );
    harness.apply(cx, finished(B, MergeOutcome::Completed, 2));
    harness.apply(cx, finished(c, MergeOutcome::Completed, 2));
    let summary = harness.queue.read_with(cx, |queue, _| queue.summary());
    assert_eq!((summary.done, summary.active, summary.percent), (3, false, 100));
  }

  #[gpui_kit::test]
  fn the_clock_ticks_only_while_a_merge_runs(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| queue.add_to_queue(A, cx));
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    harness.take_events();
    harness.apply(cx, started(A, 0));
    harness.take_events();
    cx.executor().advance_clock(Duration::from_millis(2_500));
    cx.run_until_parked();
    assert_eq!(harness.take_events(), [QueueEvent::Tick, QueueEvent::Tick]);
    harness.apply(cx, finished(A, MergeOutcome::Completed, 3));
    harness.take_events();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert_eq!(harness.take_events(), []);
  }
}
