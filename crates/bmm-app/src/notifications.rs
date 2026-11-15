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

//! What a finished merge tells the user: the in-app toast of every finished
//! file, and the OS notification and topmost window once the queue drains.
//!
//! The OS notification and the topmost window are sent once per batch (the
//! queue ran until nothing waited or merged), not per file, and only when a
//! merge of the batch completed or failed.

use bmm_core::merge::MergeOutcome;
use gpui_kit::App;

use crate::domain::format::format_hms;
use crate::state::Stores;
use crate::state::queue::{QueueBatch, QueueEvent};
use crate::state::ui::{CompletionNotice, NotificationKind};

/// The notice of a drained batch; `None` when it does not notify (every merge
/// was cancelled). A batch of one completed file names the file and its own
/// time. Any other gives the counts and the time from the first start to the
/// last end.
pub fn completion_notice(batch: &QueueBatch) -> Option<CompletionNotice> {
  if !batch.notifies() {
    return None;
  }
  let title = rust_i18n::t!("merge.mergeComplete").into_owned();
  let failed = batch.failed > 0;
  if let (1, 1, Some((file, elapsed))) = (batch.finished(), batch.completed, batch.last.as_ref()) {
    return Some(CompletionNotice {
      title,
      line: file.clone(),
      detail: rust_i18n::t!("notification.completedIn", elapsed = format_hms(Some(*elapsed))).into_owned(),
      failed,
    });
  }
  Some(CompletionNotice {
    title,
    line: rust_i18n::t!(
      "notification.summary",
      completed = batch.completed,
      failed = batch.failed,
      cancelled = batch.cancelled
    )
    .into_owned(),
    detail: rust_i18n::t!("notification.finishedIn", elapsed = format_hms(Some(batch.elapsed()))).into_owned(),
    failed,
  })
}

/// Shows the toast of every finished merge (a success with its time, a
/// failure with its error; nothing for a cancelled one), and hands the
/// notice of a drained batch to the UI state, which delivers it through the
/// enabled channels.
pub fn watch(stores: &Stores, cx: &mut App) {
  let ui = stores.ui.clone();
  cx.subscribe(&stores.queue, move |_, event: &QueueEvent, cx| match event {
    QueueEvent::Finished {
      file,
      outcome,
      error,
      elapsed,
    } => {
      let toast = match outcome {
        MergeOutcome::Completed => Some((
          NotificationKind::Success,
          rust_i18n::t!("notification.completedIn", elapsed = format_hms(Some(*elapsed))).into_owned(),
        )),
        MergeOutcome::Failed => Some((
          NotificationKind::Error,
          error
            .clone()
            .unwrap_or_else(|| rust_i18n::t!("notification.unknownError").into_owned()),
        )),
        MergeOutcome::Cancelled => None,
      };
      if let Some((kind, detail)) = toast {
        ui.update(cx, |ui, cx| ui.show_toast(kind, file.clone(), detail, cx));
      }
    }
    QueueEvent::Drained(batch) => {
      if let Some(notice) = completion_notice(batch) {
        ui.update(cx, |ui, cx| ui.show_completion(notice, cx));
      }
    }
    QueueEvent::ItemChanged { .. } | QueueEvent::Tick => {}
  })
  .detach();
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::i18n::tests::LOCALE_LOCK;
  use bmm_core::config::Language;
  use std::time::{Duration, SystemTime};

  fn batch(completed: usize, failed: usize, cancelled: usize) -> QueueBatch {
    let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    QueueBatch {
      completed,
      failed,
      cancelled,
      started: Some(started),
      ended: Some(started + Duration::from_secs(754)),
      last: Some(("/media/Last.mkv".to_string(), Duration::from_secs(62))),
    }
  }

  #[test]
  fn a_batch_of_one_completed_file_names_the_file() {
    let _locale = LOCALE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::i18n::set_language(Language::EnUS);
    let notice = completion_notice(&batch(1, 0, 0)).expect("notifies");
    assert_eq!(
      notice,
      CompletionNotice {
        title: "Merge complete.".into(),
        line: "/media/Last.mkv".into(),
        detail: "Completed in 00:01:02".into(),
        failed: false,
      }
    );
  }

  #[test]
  fn other_batches_show_counts_and_the_batch_time() {
    let _locale = LOCALE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::i18n::set_language(Language::EnUS);
    let notice = completion_notice(&batch(3, 1, 1)).expect("notifies");
    assert_eq!(notice.title, "Merge complete.");
    assert_eq!(notice.line, "3 completed, 1 failed, 1 cancelled");
    assert_eq!(notice.detail, "Finished in 00:12:34");
    assert!(notice.failed);
    // One file that failed does not get the single-file success text.
    let notice = completion_notice(&batch(0, 1, 0)).expect("a failure notifies");
    assert_eq!(notice.line, "0 completed, 1 failed, 0 cancelled");
    assert!(notice.failed);
    // One completed and one cancelled file are two files.
    let notice = completion_notice(&batch(1, 0, 1)).expect("notifies");
    assert_eq!(notice.line, "1 completed, 0 failed, 1 cancelled");
    assert!(!notice.failed);
  }

  #[test]
  fn a_batch_cancelled_entirely_sends_nothing() {
    assert_eq!(completion_notice(&batch(0, 0, 2)), None);
    assert_eq!(completion_notice(&QueueBatch::default()), None);
  }
}
