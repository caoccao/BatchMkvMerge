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

//! The link to `bmm_core::merge`: merge events in, enqueue and cancel out;
//! nothing polls.
//!
//! The core's worker threads push [`MergeEvent`]s into an unbounded channel;
//! one foreground task applies them to the queue store in the order they
//! were sent. [`MergeBackend`] is the other direction: enqueue and cancel.

use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use async_channel::Receiver;
use bmm_core::merge::{MergeArgs, MergeEvent, MergeEventSink, MergeQueue};
use gpui_kit::App;

use crate::state::Stores;

/// A sink for the core's events and the receiver [`start`] reads them from.
pub fn channel() -> (MergeEventSink, Receiver<MergeEvent>) {
  let (sender, receiver) = async_channel::unbounded();
  let sink: MergeEventSink = Box::new(move |event| {
    // Fails only once the receiver is gone, when the app is quitting.
    let _ = sender.try_send(event);
  });
  (sink, receiver)
}

type EnqueueFn = dyn Fn(String, MergeArgs) -> Result<()> + Send + Sync;
type CancelFn = dyn Fn(&str) -> Result<()> + Send + Sync;

/// Enqueues and cancels merges: the process-wide `mkvmerge` queue in the app,
/// a queue with a fake launcher (or a recorder) in tests.
#[derive(Clone)]
pub struct MergeBackend {
  enqueue: Arc<EnqueueFn>,
  cancel: Arc<CancelFn>,
}

impl MergeBackend {
  pub fn new(
    enqueue: impl Fn(String, MergeArgs) -> Result<()> + Send + Sync + 'static,
    cancel: impl Fn(&str) -> Result<()> + Send + Sync + 'static,
  ) -> Self {
    Self {
      enqueue: Arc::new(enqueue),
      cancel: Arc::new(cancel),
    }
  }

  /// The process-wide queue, which runs `mkvmerge`; `sink` receives its
  /// events (only the first call installs one).
  pub fn system(sink: MergeEventSink) -> Self {
    bmm_core::merge::init(sink);
    Self::new(bmm_core::merge::enqueue, |file| {
      bmm_core::merge::cancel(file.to_string())
    })
  }

  /// A queue of its own, e.g. with a fake launcher.
  pub fn queue(queue: Arc<MergeQueue>) -> Self {
    let cancel_queue = queue.clone();
    Self::new(
      move |file, args| queue.enqueue(file, args),
      move |file| cancel_queue.cancel(file),
    )
  }

  /// Queues `file` with its `mkvmerge` arguments.
  pub fn enqueue(&self, file: String, args: MergeArgs) -> Result<()> {
    (self.enqueue)(file, args)
  }

  /// Cancels `file`, waiting or running.
  pub fn cancel(&self, file: &str) -> Result<()> {
    (self.cancel)(file)
  }
}

/// Applies `events` to the queue store on the foreground, in order, and
/// starts what a finished merge tells the user ([`crate::notifications`]).
pub fn start(stores: &Stores, events: Receiver<MergeEvent>, cx: &mut App) {
  crate::notifications::watch(stores, cx);
  let queue = stores.queue.clone();
  cx.spawn(async move |cx| {
    while let Ok(event) = events.recv().await {
      queue.update(cx, |queue, cx| queue.apply_event(&event, SystemTime::now(), cx));
    }
  })
  .detach();
}
