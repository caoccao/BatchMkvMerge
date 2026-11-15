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

//! The merge queue as the UI shows it: one [`QueueItem`] per enqueued root
//! file, in the order it was first enqueued, kept current by
//! `bmm_core::merge`'s events (see [`crate::merge_events`]).
//!
//! A change notifies observers only when it is structural (an item added or
//! removed, or a status changed): the toolbar, the title and the close guard
//! care about that. Every change, progress included, is also emitted as
//! [`QueueEvent::ItemChanged`] naming the file, so a card or a queue row can
//! repaint alone.
//!
//! A batch runs from the moment an item waits or merges until none does; its
//! end is emitted once as [`QueueEvent::Drained`], which sends the completion
//! notifications once per batch, not per finished file.
//!
//! An enqueue lists its files as waiting before it resolves their outputs on
//! the background executor ([`QueueStore::prepare`]), so Clear, Delete, Cancel
//! All and the close guard see them from the first moment; it hands each to
//! the merge queue only if it is still listed then ([`QueueStore::submit`]).
//! A file removed while the merge queue still has it stays, hidden, until its
//! `Finished` arrives, which then drops it: the item counts as active until
//! the merge really ended, and the late `Cancelled` lists nothing again.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use bmm_core::merge::{MergeEvent, MergeOutcome};
use gpui_kit::{Context, EventEmitter, Task};

use crate::domain::status::{QueueItem, QueueItemStatus, can_enqueue};
use crate::state::outputs::OutputReservations;

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
  /// The last waiting or merging item finished or went: the batch is over.
  Drained(QueueBatch),
}

/// What one batch of merges ended with: every merge that finished between
/// the queue becoming active and the last active item going.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueBatch {
  pub completed: usize,
  pub failed: usize,
  pub cancelled: usize,
  /// The batch's first merge start and its last end.
  pub started: Option<SystemTime>,
  pub ended: Option<SystemTime>,
  /// The file that finished last, with its own elapsed time.
  pub last: Option<(String, Duration)>,
}

impl QueueBatch {
  /// Merges that finished in the batch.
  pub fn finished(&self) -> usize {
    self.completed + self.failed + self.cancelled
  }

  /// Whether the batch is worth a notification: a merge completed or
  /// failed. A batch the user cancelled entirely is not.
  pub fn notifies(&self) -> bool {
    self.completed + self.failed > 0
  }

  /// From the first start to the last end; zero without either.
  pub fn elapsed(&self) -> Duration {
    match (self.started, self.ended) {
      (Some(started), Some(ended)) => ended.duration_since(started).unwrap_or_default(),
      _ => Duration::ZERO,
    }
  }

  fn start(&mut self, at: SystemTime) {
    self.started = Some(self.started.map_or(at, |started| started.min(at)));
  }

  fn finish(&mut self, file: &str, outcome: MergeOutcome, at: SystemTime, elapsed: Duration) {
    match outcome {
      MergeOutcome::Completed => self.completed += 1,
      MergeOutcome::Failed => self.failed += 1,
      MergeOutcome::Cancelled => self.cancelled += 1,
    }
    self.ended = Some(self.ended.map_or(at, |ended| ended.max(at)));
    self.last = Some((file.to_string(), elapsed));
  }
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

/// A merge the store listed as waiting until it ends.
#[derive(Clone, Copy, Debug)]
struct Job {
  /// The enqueue that listed it, and the key of its output reservation.
  ticket: u64,
  /// The merge queue has it, so a `Finished` will come.
  submitted: bool,
  /// Its file was removed while the merge queue had it: hidden, and dropped
  /// by its `Finished`.
  discarded: bool,
}

#[derive(Default)]
pub struct QueueStore {
  items: HashMap<String, QueueItem>,
  order: Vec<String>,
  /// The waiting and merging items' merges.
  jobs: HashMap<String, Job>,
  last_ticket: u64,
  reservations: OutputReservations,
  /// Runs while an item is merging and emits [`QueueEvent::Tick`].
  ticker: Option<Task<()>>,
  /// The batch running while an item is waiting or merging.
  batch: Option<QueueBatch>,
}

impl EventEmitter<QueueEvent> for QueueStore {}

impl QueueStore {
  pub fn new() -> Self {
    Self::default()
  }

  /// The item of `file`; `None` too while a removed file's merge is ending.
  pub fn item(&self, file: &str) -> Option<&QueueItem> {
    self.items.get(file).filter(|_| self.shown(file))
  }

  /// The items in queue order.
  pub fn items(&self) -> impl Iterator<Item = &QueueItem> {
    self
      .order
      .iter()
      .filter(|file| self.shown(file))
      .filter_map(|file| self.items.get(file))
  }

  /// Whether `file` may be enqueued: it is not waiting or merging, nor a
  /// removed file whose merge is still ending.
  pub fn can_enqueue(&self, file: &str) -> bool {
    can_enqueue(self.items.get(file))
  }

  /// Whether a merge is waiting or running, a removed file's included.
  pub fn has_active(&self) -> bool {
    self.items.values().any(|item| item.status.is_active())
  }

  /// The waiting and merging files in queue order, removed files whose merge
  /// is still ending included.
  pub fn active_files(&self) -> Vec<String> {
    self
      .order
      .iter()
      .filter_map(|file| self.items.get(file))
      .filter(|item| item.status.is_active())
      .map(|item| item.file.clone())
      .collect()
  }

  pub fn summary(&self) -> QueueSummary {
    QueueSummary::of(self.items())
  }

  /// The outputs of the merges being prepared, waiting or merging.
  pub fn reservations(&self) -> &OutputReservations {
    &self.reservations
  }

  fn shown(&self, file: &str) -> bool {
    !self.jobs.get(file).is_some_and(|job| job.discarded)
  }

  /// The drives with their files, each in queue order; the drives in the order
  /// their first file was enqueued.
  pub fn drives(&self) -> Vec<(String, Vec<String>)> {
    let mut drives: Vec<(String, Vec<String>)> = Vec::new();
    for item in self.items() {
      match drives.iter_mut().find(|(drive, _)| *drive == item.drive) {
        Some((_, files)) => files.push(item.file.clone()),
        None => drives.push((item.drive.clone(), vec![item.file.clone()])),
      }
    }
    drives
  }

  /// The items of one drive, in queue order.
  pub fn items_in_drive<'a>(&'a self, drive: &'a str) -> impl Iterator<Item = &'a QueueItem> + 'a {
    self.items().filter(move |item| item.drive == drive)
  }

  /// Lists `file` as waiting while its enqueue resolves the output, and
  /// returns the enqueue's ticket for [`Self::submit`] and the output
  /// reservation. A file that is waiting or merging already is left alone
  /// (`None`); a finished one starts over in its old position.
  pub fn prepare(&mut self, file: &str, cx: &mut Context<Self>) -> Option<u64> {
    if !self.can_enqueue(file) {
      return None;
    }
    if self.items.insert(file.to_string(), QueueItem::waiting(file)).is_none() {
      self.order.push(file.to_string());
    }
    self.last_ticket += 1;
    let ticket = self.last_ticket;
    self.jobs.insert(
      file.to_string(),
      Job {
        ticket,
        submitted: false,
        discarded: false,
      },
    );
    self.changed(file, true, cx);
    Some(ticket)
  }

  /// Whether the enqueue `ticket` hands `file` to the merge queue now. Not
  /// when the file was removed or listed by a newer enqueue meanwhile (the
  /// ticket's reservation is freed), nor when a cancel was requested: it ends
  /// cancelled here, as a waiting merge does in the merge queue.
  pub fn submit(&mut self, file: &str, ticket: u64, cx: &mut Context<Self>) -> bool {
    let Some(job) = self.jobs.get_mut(file).filter(|job| job.ticket == ticket) else {
      self.reservations.release(ticket);
      return false;
    };
    if job.submitted {
      return false;
    }
    if self.items.get(file).is_some_and(|item| item.cancel_requested) {
      let now = SystemTime::now();
      let cancelled = MergeEvent::Finished {
        file: file.to_string(),
        outcome: MergeOutcome::Cancelled,
        error: None,
        at: now,
      };
      self.apply_event(&cancelled, now, cx);
      return false;
    }
    job.submitted = true;
    true
  }

  /// Takes `file` off the queue after the merge queue refused it.
  pub fn abandon(&mut self, file: &str, cx: &mut Context<Self>) {
    self.end_job(file);
    self.drop_item(file, cx);
  }

  /// Lists `file` as waiting in the merge queue, as an enqueue does once it
  /// resolved the output (`false` while it is waiting or merging).
  #[cfg(test)]
  pub(crate) fn add_to_queue(&mut self, file: &str, cx: &mut Context<Self>) -> bool {
    let Some(ticket) = self.prepare(file, cx) else {
      return false;
    };
    self.submit(file, ticket, cx)
  }

  /// Drops the item of `file`. While the merge queue still has its merge, the
  /// item is only hidden: it counts as active until its `Finished` arrives,
  /// which drops it without a row, a toast or a place in the batch.
  pub fn remove_from_queue(&mut self, file: &str, cx: &mut Context<Self>) {
    let Some(item) = self.items.get(file) else {
      return;
    };
    let active = item.status.is_active();
    if let Some(job) = self.jobs.get_mut(file).filter(|job| job.submitted && active) {
      if !job.discarded {
        job.discarded = true;
        self.changed(file, true, cx);
      }
      return;
    }
    // A merge still being prepared goes now; its submit finds it gone and
    // frees the reservation.
    self.jobs.remove(file);
    self.drop_item(file, cx);
  }

  fn drop_item(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.items.remove(file).is_some() {
      self.order.retain(|listed| listed != file);
      self.changed(file, true, cx);
    }
  }

  /// The merge of `file` ended: its output is written or given up.
  fn end_job(&mut self, file: &str) {
    if let Some(job) = self.jobs.remove(file) {
      self.reservations.release(job.ticket);
    }
  }

  /// Flags a cancel request on a waiting or merging item; the outcome still
  /// comes from the merge events.
  pub fn mark_cancel_requested(&mut self, file: &str, cx: &mut Context<Self>) {
    if self.items.get_mut(file).is_some_and(QueueItem::request_cancel) {
      self.changed(file, false, cx);
    }
  }

  /// Applies one merge event. An event for a file that is not listed lists it.
  /// `now` stands in for a time the event does not carry.
  pub fn apply_event(&mut self, event: &MergeEvent, now: SystemTime, cx: &mut Context<Self>) {
    let file = event.file();
    let finished = matches!(event, MergeEvent::Finished { .. });
    if !self.shown(file) {
      // A removed file: its merge's end drops it.
      if finished {
        self.end_job(file);
        self.drop_item(file, cx);
      }
      return;
    }
    if finished {
      self.end_job(file);
    }
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
    if let MergeEvent::Started { at, .. } = event {
      self.batch.get_or_insert_default().start(*at);
    }
    if let MergeEvent::Finished { outcome, error, at, .. } = event {
      let elapsed = self.items[file].elapsed(*at).unwrap_or_default();
      // A finish while nothing was active (a late `Cancelled` for a removed
      // item) belongs to no batch.
      if let Some(batch) = self.batch.as_mut() {
        batch.finish(file, *outcome, *at, elapsed);
      }
      cx.emit(QueueEvent::Finished {
        file: file.to_string(),
        outcome: *outcome,
        error: error.clone(),
        elapsed,
      });
    }
    self.changed(file, structural, cx);
  }

  /// Removes the finished items of one drive.
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

  /// Moves the item of `file` to `drive`, as if it were on another drive
  /// (drive keys come from Windows paths only).
  #[cfg(test)]
  pub(crate) fn move_to_drive(&mut self, file: &str, drive: &str, cx: &mut Context<Self>) {
    if let Some(item) = self.items.get_mut(file) {
      item.drive = drive.to_string();
      self.changed(file, true, cx);
    }
  }

  /// Removes every item (the queue part of Clear), as
  /// [`Self::remove_from_queue`] does.
  pub fn clear(&mut self, cx: &mut Context<Self>) {
    for file in self.order.clone() {
      self.remove_from_queue(&file, cx);
    }
  }

  fn changed(&mut self, file: &str, structural: bool, cx: &mut Context<Self>) {
    cx.emit(QueueEvent::ItemChanged { file: file.to_string() });
    if structural {
      cx.notify();
    }
    self.update_ticker(cx);
    self.update_batch(cx);
  }

  /// Starts a batch when an item waits or merges, and ends it (emitting
  /// [`QueueEvent::Drained`] once) when none does any more.
  fn update_batch(&mut self, cx: &mut Context<Self>) {
    if self.has_active() {
      self.batch.get_or_insert_default();
    } else if let Some(batch) = self.batch.take() {
      cx.emit(QueueEvent::Drained(batch));
    }
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
      // The merge queue still has both merges.
      assert!(queue.has_active());
      assert_eq!(queue.active_files(), [A, B]);
      assert!(!queue.can_enqueue(A), "not before its merge ended");
    });
    harness.apply(cx, started(B, 1));
    harness.apply(cx, finished(A, MergeOutcome::Cancelled, 1));
    harness.apply(cx, finished(B, MergeOutcome::Cancelled, 2));
    harness.queue.read_with(cx, |queue, _| {
      assert_eq!(queue.items().count(), 0, "their ends list nothing");
      assert!(!queue.has_active());
      assert!(queue.can_enqueue(A));
    });
  }

  #[gpui_kit::test]
  fn a_prepared_merge_is_submitted_only_while_it_is_still_listed(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    let reserve = |queue: &QueueStore, ticket: u64, output: &str| {
      queue
        .reservations()
        .reserve(ticket, |_| crate::state::outputs::Resolution {
          output: output.to_string(),
          automatic: true,
          value: (),
        });
    };
    harness.queue.update(cx, |queue, cx| {
      let first = queue.prepare(A, cx).expect("listed");
      assert_eq!(queue.prepare(A, cx), None, "already waiting");
      assert_eq!(queue.item(A).unwrap().status, QueueItemStatus::Waiting);
      assert!(queue.has_active());
      reserve(queue, first, "/out/a.mkv");
      // Removed while its output was resolved, then listed again.
      queue.remove_from_queue(A, cx);
      assert!(queue.item(A).is_none());
      let second = queue.prepare(A, cx).expect("listed again");
      reserve(queue, second, "/out/a (1).mkv");
      assert!(!queue.submit(A, first, cx), "an older enqueue");
      assert!(!queue.reservations().taken().contains("/out/a.mkv"));
      assert!(queue.submit(A, second, cx));
      assert!(!queue.submit(A, second, cx), "submitted once");
      assert!(queue.reservations().taken().contains("/out/a (1).mkv"));
      // Cancelled before it reached the merge queue.
      let ticket = queue.prepare(B, cx).expect("listed");
      reserve(queue, ticket, "/out/b.mkv");
      queue.mark_cancel_requested(B, cx);
      assert!(!queue.submit(B, ticket, cx));
      assert_eq!(queue.item(B).unwrap().status, QueueItemStatus::Cancelled);
      assert!(!queue.reservations().taken().contains("/out/b.mkv"));
      // Refused by the merge queue.
      queue.abandon(A, cx);
      assert!(queue.item(A).is_none());
      assert!(!queue.reservations().taken().contains("/out/a (1).mkv"));
      assert!(!queue.has_active());
    });
    assert!(harness.take_events().contains(&QueueEvent::Finished {
      file: B.to_string(),
      outcome: MergeOutcome::Cancelled,
      error: None,
      elapsed: Duration::ZERO,
    }));
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

  fn drained(events: &[QueueEvent]) -> Vec<QueueBatch> {
    events
      .iter()
      .filter_map(|event| match event {
        QueueEvent::Drained(batch) => Some(batch.clone()),
        _ => None,
      })
      .collect()
  }

  #[gpui_kit::test]
  fn a_batch_drains_once_after_its_last_merge(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    let c = "/media/c.mkv";
    harness.queue.update(cx, |queue, cx| {
      for file in [A, B, c] {
        queue.add_to_queue(file, cx);
      }
    });
    harness.apply(cx, started(A, 10));
    harness.apply(cx, finished(A, MergeOutcome::Completed, 40));
    harness.apply(cx, started(B, 40));
    harness.apply(cx, finished(B, MergeOutcome::Failed, 45));
    assert_eq!(drained(&harness.take_events()), [], "a merge still waits");
    harness.apply(cx, started(c, 45));
    harness.apply(cx, finished(c, MergeOutcome::Completed, 100));
    let batches = drained(&harness.take_events());
    assert_eq!(
      batches,
      [QueueBatch {
        completed: 2,
        failed: 1,
        cancelled: 0,
        started: Some(at(10)),
        ended: Some(at(100)),
        last: Some((c.to_string(), Duration::from_secs(55))),
      }]
    );
    assert_eq!(batches[0].elapsed(), Duration::from_secs(90));
    assert_eq!(batches[0].finished(), 3);
    assert!(batches[0].notifies());
    // A late event of a finished item starts no batch.
    harness.apply(cx, progress(c, 50));
    assert_eq!(drained(&harness.take_events()), []);
  }

  #[gpui_kit::test]
  fn the_next_batch_starts_afresh(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| queue.add_to_queue(A, cx));
    harness.apply(cx, started(A, 0));
    harness.apply(cx, finished(A, MergeOutcome::Completed, 5));
    harness.take_events();
    harness.queue.update(cx, |queue, cx| queue.add_to_queue(B, cx));
    harness.apply(cx, started(B, 20));
    harness.apply(cx, finished(B, MergeOutcome::Completed, 23));
    let batches = drained(&harness.take_events());
    assert_eq!(batches.len(), 1);
    assert_eq!(
      (batches[0].completed, batches[0].elapsed()),
      (1, Duration::from_secs(3))
    );
    assert_eq!(batches[0].last, Some((B.to_string(), Duration::from_secs(3))));
  }

  #[gpui_kit::test]
  fn a_batch_cancelled_entirely_does_not_notify(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      queue.add_to_queue(A, cx);
      queue.add_to_queue(B, cx);
    });
    harness.apply(cx, started(A, 0));
    harness.apply(cx, finished(A, MergeOutcome::Cancelled, 2));
    harness.apply(cx, finished(B, MergeOutcome::Cancelled, 2));
    let batches = drained(&harness.take_events());
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].cancelled, 2);
    assert!(!batches[0].notifies());
  }

  #[gpui_kit::test]
  fn removing_the_last_waiting_item_ends_the_batch_with_its_cancel(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      queue.add_to_queue(A, cx);
      queue.add_to_queue(B, cx);
    });
    harness.apply(cx, started(A, 0));
    harness.apply(cx, finished(A, MergeOutcome::Completed, 8));
    harness.take_events();
    harness.queue.update(cx, |queue, cx| queue.remove_from_queue(B, cx));
    assert_eq!(drained(&harness.take_events()), [], "the merge queue still has B");
    // The late `Cancelled` of the removed item drops it: no row, no toast, no count.
    harness.apply(cx, finished(B, MergeOutcome::Cancelled, 9));
    let events = harness.take_events();
    assert!(!events.iter().any(|event| matches!(event, QueueEvent::Finished { .. })));
    let batches = drained(&events);
    assert_eq!(batches.len(), 1);
    assert_eq!((batches[0].completed, batches[0].cancelled), (1, 0));
    assert_eq!(harness.status(cx, B), None);
  }

  #[gpui_kit::test]
  fn drives_keep_the_order_of_their_first_file(cx: &mut TestAppContext) {
    let harness = Harness::new(cx);
    harness.queue.update(cx, |queue, cx| {
      queue.add_to_queue(A, cx);
      queue.add_to_queue(B, cx);
      let drive = bmm_core::merge::get_drive_key(A);
      assert_eq!(queue.drives(), [(drive.clone(), vec![A.to_string(), B.to_string()])]);
      let files: Vec<&str> = queue.items_in_drive(&drive).map(|item| item.file.as_str()).collect();
      assert_eq!(files, [A, B]);
      assert_eq!(queue.items_in_drive("elsewhere").count(), 0);
    });
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
