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

//! Per-drive merge queue.
//!
//! Each drive (the Windows path prefix such as `C:`; a single `"default"`
//! bucket on Linux and macOS) runs at most one `mkvmerge` at a time; further
//! files for that drive wait in FIFO order. Different drives merge in
//! parallel. Every merge runs on its own plain thread.
//!
//! Progress and completion are pushed to a [`MergeEventSink`]. For each
//! enqueued file the sink receives [`MergeEvent::Started`], then any number of
//! [`MergeEvent::Progress`], then exactly one [`MergeEvent::Finished`]. A file
//! cancelled while still waiting receives only `Finished` with
//! [`MergeOutcome::Cancelled`]. One file's events come from one thread, in
//! order, and `Finished` of a file is sent before `Started` of the next file
//! on the same drive; events of different drives interleave.
//!
//! The process-wide queue behind [`enqueue`] and [`cancel`]
//! launches the real `mkvmerge` and forwards its events to the sink installed
//! by [`init`]. Tests build their own [`MergeQueue`] with a fake launcher.

use anyhow::Result;
use std::collections::HashMap;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use crate::mkvtoolnix;

const ERROR_TASK_MISSING: &str = "task missing";
const ERROR_WORKER_PANICKED: &str = "merge worker panicked";

/// How a merge ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
  Completed,
  Cancelled,
  Failed,
}

impl MergeOutcome {
  /// The outcome's name, the same as the queue status it ends in.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Completed => "Completed",
      Self::Cancelled => "Cancelled",
      Self::Failed => "Failed",
    }
  }
}

/// A change in one file's merge, pushed to the [`MergeEventSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeEvent {
  /// The file left the queue and its `mkvmerge` is being launched.
  Started { file: String, at: SystemTime },
  /// `mkvmerge` reported a new percentage; sent only when the value changes.
  Progress { file: String, percent: u32 },
  /// The merge ended. `error` is set when `outcome` is [`MergeOutcome::Failed`].
  Finished {
    file: String,
    outcome: MergeOutcome,
    error: Option<String>,
    at: SystemTime,
  },
}

impl MergeEvent {
  /// The source file the event is about.
  pub fn file(&self) -> &str {
    match self {
      Self::Started { file, .. } | Self::Progress { file, .. } | Self::Finished { file, .. } => file,
    }
  }
}

/// Receives every [`MergeEvent`]. Called from worker threads (and from the
/// thread calling [`cancel`] for a waiting file), never with the queue lock
/// held. It must not panic and should return quickly, e.g. by sending into a
/// channel.
pub type MergeEventSink = Box<dyn Fn(MergeEvent) + Send + Sync>;

/// A launched merge process.
pub struct MergeProcess {
  /// The process's stdout, scanned for `Progress: NN%` lines; `None` when it
  /// was not captured.
  pub output: Option<Box<dyn Read + Send>>,
  /// Kills and waits for the process.
  pub control: Box<dyn MergeProcessControl>,
}

/// Handle to a running merge process.
pub trait MergeProcessControl: Send {
  /// Stop the process; called on cancel while it is still running.
  fn kill(&mut self) -> std::io::Result<()>;
  /// Block until the process exits and return its exit code (`None` when it
  /// was ended by a signal). Called once, after its output reached EOF.
  fn wait(&mut self) -> std::io::Result<Option<i32>>;
}

/// Starts the merge process for `file` with the complete `mkvmerge` argument
/// list (output, per-file options and inputs).
pub type MergeLauncher = Box<dyn Fn(&str, &[String]) -> Result<MergeProcess> + Send + Sync>;

type SharedControl = Arc<Mutex<Box<dyn MergeProcessControl>>>;

enum WorkerOutcome {
  Completed,
  Cancelled,
  Failed(String),
}

enum TaskStatus {
  Queued,
  Merging,
}

struct TaskState {
  status: TaskStatus,
  progress: u32,
  args: Vec<String>,
  cancel_requested: bool,
}

#[derive(Default)]
struct DriveState {
  merging: Option<String>,
  queued: Vec<String>,
}

#[derive(Default)]
struct MergeState {
  tasks: HashMap<String, TaskState>,
  drives: HashMap<String, DriveState>,
  children: HashMap<String, SharedControl>,
}

/// A per-drive merge queue with its own launcher and event sink.
pub struct MergeQueue {
  state: Mutex<MergeState>,
  launcher: MergeLauncher,
  sink: MergeEventSink,
}

impl MergeQueue {
  pub fn new(launcher: MergeLauncher, sink: MergeEventSink) -> Arc<Self> {
    Arc::new(Self {
      state: Mutex::new(MergeState::default()),
      launcher,
      sink,
    })
  }

  /// Queue `file` for merging with the complete `mkvmerge` argument list.
  /// It starts at once when its drive is idle. A file that is already queued
  /// or merging is ignored.
  pub fn enqueue(self: &Arc<Self>, file: String, args: Vec<String>) -> Result<()> {
    let drive_key = get_drive_key(&file);
    let should_start;
    {
      let mut st = self.lock_state();
      if st.tasks.contains_key(&file) {
        return Ok(());
      }
      st.tasks.insert(
        file.clone(),
        TaskState {
          status: TaskStatus::Queued,
          progress: 0,
          args,
          cancel_requested: false,
        },
      );
      let drive = st.drives.entry(drive_key).or_default();
      if drive.merging.is_none() {
        drive.merging = Some(file.clone());
        if let Some(t) = st.tasks.get_mut(&file) {
          t.status = TaskStatus::Merging;
        }
        should_start = true;
      } else {
        drive.queued.push(file.clone());
        should_start = false;
      }
    }
    if should_start {
      self.spawn_worker(file);
    }
    Ok(())
  }

  /// Cancel `file`. A waiting file is dropped from its drive queue and
  /// finishes as `Cancelled` right away; a running merge is killed and its
  /// worker reports `Cancelled`. Unknown files are ignored.
  pub fn cancel(&self, file: &str) -> Result<()> {
    let control = {
      let mut st = self.lock_state();
      let is_merging = match st.tasks.get(file) {
        Some(t) => matches!(t.status, TaskStatus::Merging),
        None => return Ok(()),
      };
      if !is_merging {
        st.tasks.remove(file);
        if let Some(drive) = st.drives.get_mut(&get_drive_key(file)) {
          drive.queued.retain(|f| f != file);
        }
        drop(st);
        self.emit_finished(file, &WorkerOutcome::Cancelled);
        return Ok(());
      }
      if let Some(t) = st.tasks.get_mut(file) {
        t.cancel_requested = true;
      }
      st.children.get(file).cloned()
    };
    if let Some(control) = control {
      kill(&control);
    }
    Ok(())
  }

  /// `(file, status, progress)` of the files that are waiting or merging.
  #[cfg(test)]
  fn entries(&self) -> Vec<(String, String, u32)> {
    let st = self.lock_state();
    st.tasks
      .iter()
      .map(|(file, task)| {
        let status = match task.status {
          TaskStatus::Queued => "Waiting",
          TaskStatus::Merging => "Merging",
        };
        (file.clone(), status.to_owned(), task.progress)
      })
      .collect()
  }

  fn emit(&self, event: MergeEvent) {
    (self.sink)(event);
  }

  fn emit_finished(&self, file: &str, outcome: &WorkerOutcome) {
    let (outcome, error) = match outcome {
      WorkerOutcome::Completed => (MergeOutcome::Completed, None),
      WorkerOutcome::Cancelled => (MergeOutcome::Cancelled, None),
      WorkerOutcome::Failed(msg) => (MergeOutcome::Failed, Some(msg.clone())),
    };
    self.emit(MergeEvent::Finished {
      file: file.to_owned(),
      outcome,
      error,
      at: SystemTime::now(),
    });
  }

  /// Runs the merge and decides its outcome. Everything a worker does between
  /// `Started` and `Finished` happens here, so [`Self::run_worker`] can turn a
  /// panic into `Failed` without losing the `Finished` event.
  fn execute(&self, file: &str) -> WorkerOutcome {
    let args = self.lock_state().tasks.get(file).map(|t| t.args.clone());
    let Some(args) = args else {
      return WorkerOutcome::Failed(ERROR_TASK_MISSING.to_owned());
    };
    let MergeProcess { output, control } = match (self.launcher)(file, &args) {
      Ok(process) => process,
      Err(err) => {
        log::error!("spawn_mkvmerge failed for {}: {}", file, err);
        return WorkerOutcome::Failed(err.to_string());
      }
    };
    let control: SharedControl = Arc::new(Mutex::new(control));
    let cancelled_while_launching = {
      let mut st = self.lock_state();
      st.children.insert(file.to_owned(), Arc::clone(&control));
      st.tasks.get(file).is_some_and(|t| t.cancel_requested)
    };
    if cancelled_while_launching {
      // `cancel` ran before the process was registered, so nothing killed it.
      kill(&control);
    }
    if let Some(output) = output {
      mkvtoolnix::read_mkvmerge_output(output, |line| {
        if let Some(percent) = mkvtoolnix::parse_mkvmerge_progress(line) {
          self.record_progress(file, percent);
        }
      });
    }
    let exit = match control.lock() {
      Ok(mut guard) => guard.wait(),
      Err(_) => Err(std::io::Error::other("child lock poisoned")),
    };
    let cancel_requested = self.lock_state().tasks.get(file).is_some_and(|t| t.cancel_requested);
    if cancel_requested {
      WorkerOutcome::Cancelled
    } else {
      match exit {
        Ok(Some(0)) => WorkerOutcome::Completed,
        Ok(code) => WorkerOutcome::Failed(format!("mkvmerge exited with code {:?}", code)),
        Err(e) => WorkerOutcome::Failed(e.to_string()),
      }
    }
  }

  /// Drop the task, emit `Finished` and start the next file of its drive.
  ///
  /// The task goes before `Finished` is sent, so an enqueue of the same file
  /// that answers it (a retry) is a new task rather than a duplicate of this
  /// one. The drive stays taken until `Finished` is out: the retry waits on
  /// it, and no file of the drive starts before the report.
  fn finish(self: &Arc<Self>, file: &str, outcome: WorkerOutcome) {
    {
      let mut st = self.lock_state();
      st.children.remove(file);
      st.tasks.remove(file);
    }
    self.emit_finished(file, &outcome);
    let next_file = {
      let mut st = self.lock_state();
      let drive_key = get_drive_key(file);
      let drive = st.drives.entry(drive_key).or_default();
      if drive.merging.as_deref() == Some(file) {
        drive.merging = None;
      }
      if drive.merging.is_none() && !drive.queued.is_empty() {
        let next = drive.queued.remove(0);
        drive.merging = Some(next.clone());
        if let Some(t) = st.tasks.get_mut(&next) {
          t.status = TaskStatus::Merging;
          t.progress = 0;
        }
        Some(next)
      } else {
        None
      }
    };
    if let Some(next) = next_file {
      self.spawn_worker(next);
    }
  }

  fn kill_child(&self, file: &str) {
    let control = self.lock_state().children.get(file).cloned();
    if let Some(control) = control {
      kill(&control);
    }
  }

  /// The queue state stays consistent across a worker panic (every update is
  /// a few field writes), so a poisoned lock is recovered rather than
  /// propagated.
  fn lock_state(&self) -> MutexGuard<'_, MergeState> {
    self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  fn record_progress(&self, file: &str, percent: u32) {
    let changed = {
      let mut st = self.lock_state();
      match st.tasks.get_mut(file) {
        Some(task) if task.progress != percent => {
          task.progress = percent;
          true
        }
        _ => false,
      }
    };
    if changed {
      self.emit(MergeEvent::Progress {
        file: file.to_owned(),
        percent,
      });
    }
  }

  fn run_worker(self: &Arc<Self>, file: String) {
    self.emit(MergeEvent::Started {
      file: file.clone(),
      at: SystemTime::now(),
    });
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| self.execute(&file))).unwrap_or_else(|_| {
      log::error!("Merge worker for {} panicked", file);
      self.kill_child(&file);
      WorkerOutcome::Failed(ERROR_WORKER_PANICKED.to_owned())
    });
    self.finish(&file, outcome);
  }

  fn spawn_worker(self: &Arc<Self>, file: String) {
    let queue = Arc::clone(self);
    std::thread::spawn(move || {
      queue.run_worker(file);
    });
  }
}

fn kill(control: &SharedControl) {
  let mut guard = control.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let _ = guard.kill();
}

/// The per-drive queue a file belongs to: the Windows path prefix (`C:`,
/// `\\server\share`) uppercased, else `default`. One drive merges one file at a
/// time. The app groups its queue rows by the same key.
pub fn get_drive_key(file: &str) -> String {
  #[cfg(target_os = "windows")]
  {
    let path = std::path::Path::new(file);
    if let Some(first) = path.components().next() {
      if let std::path::Component::Prefix(prefix) = first {
        return prefix.as_os_str().to_string_lossy().to_ascii_uppercase();
      }
    }
  }
  #[cfg(not(target_os = "windows"))]
  {
    let _ = file;
  }
  "default".to_string()
}

struct ChildControl(std::process::Child);

impl MergeProcessControl for ChildControl {
  fn kill(&mut self) -> std::io::Result<()> {
    self.0.kill()
  }

  fn wait(&mut self) -> std::io::Result<Option<i32>> {
    self.0.wait().map(|status| status.code())
  }
}

fn launch_mkvmerge(file: &str, args: &[String]) -> Result<MergeProcess> {
  let mut child = mkvtoolnix::spawn_mkvmerge(file, args)?;
  let output = child
    .stdout
    .take()
    .map(|stdout| Box::new(stdout) as Box<dyn Read + Send>);
  Ok(MergeProcess {
    output,
    control: Box::new(ChildControl(child)),
  })
}

static SINK: OnceLock<MergeEventSink> = OnceLock::new();
static QUEUE: OnceLock<Arc<MergeQueue>> = OnceLock::new();

fn global() -> &'static Arc<MergeQueue> {
  QUEUE.get_or_init(|| {
    MergeQueue::new(
      Box::new(launch_mkvmerge),
      Box::new(|event| {
        if let Some(sink) = SINK.get() {
          sink(event);
        }
      }),
    )
  })
}

/// Install the event sink of the process-wide queue. Only the first call
/// counts; events sent before it are dropped.
pub fn init(sink: MergeEventSink) {
  if SINK.set(sink).is_err() {
    log::warn!("merge::init called more than once; keeping the first sink");
  }
}

/// [`MergeQueue::enqueue`] on the process-wide queue, which runs `mkvmerge`.
pub fn enqueue(file: String, args: Vec<String>) -> Result<()> {
  global().enqueue(file, args)
}

/// [`MergeQueue::cancel`] on the process-wide queue.
pub fn cancel(file: String) -> Result<()> {
  global().cancel(&file)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::{PipeWriter, Write};
  use std::sync::atomic::{AtomicBool, Ordering};
  use std::sync::mpsc::{self, Receiver, Sender};
  use std::time::{Duration, Instant};

  const TIMEOUT: Duration = Duration::from_secs(10);
  const A: &str = "/media/a.mkv";
  const B: &str = "/media/b.mkv";
  const C: &str = "/media/c.mkv";

  /// An event without its timestamp, for comparisons.
  #[derive(Debug, PartialEq, Eq)]
  enum Seen {
    Started(String),
    Progress(String, u32),
    Finished(String, MergeOutcome, Option<String>),
  }

  fn started(file: &str) -> Seen {
    Seen::Started(file.to_owned())
  }

  fn progress(file: &str, percent: u32) -> Seen {
    Seen::Progress(file.to_owned(), percent)
  }

  fn finished(file: &str, outcome: MergeOutcome, error: Option<&str>) -> Seen {
    Seen::Finished(file.to_owned(), outcome, error.map(str::to_owned))
  }

  /// How the fake launcher treats a file.
  enum Launch {
    Run,
    /// Wait for [`Harness::release_launch`] before starting the process.
    Gated,
    Fail(&'static str),
    Panic,
  }

  /// Shared between the fake process (worker side) and the test.
  struct FakeShared {
    writer: Mutex<Option<PipeWriter>>,
    /// `None` while running; `Some(code)` once ended (`Some(None)` = killed).
    exit: Mutex<Option<Option<i32>>>,
    killed: AtomicBool,
  }

  impl FakeShared {
    fn end(&self, code: Option<i32>) {
      {
        let mut exit = self.exit.lock().unwrap();
        if exit.is_none() {
          *exit = Some(code);
        }
      }
      // Closing the pipe is what lets the worker reach `wait`.
      self.writer.lock().unwrap().take();
    }
  }

  /// Test side of one fake `mkvmerge` run.
  struct FakeProcess {
    file: String,
    args: Vec<String>,
    shared: Arc<FakeShared>,
  }

  impl FakeProcess {
    fn report(&self, percent: u32) {
      let mut writer = self.shared.writer.lock().unwrap();
      let writer = writer.as_mut().expect("process still running");
      write!(writer, "Progress: {percent}%\r").unwrap();
      writer.flush().unwrap();
    }

    fn exit(&self, code: i32) {
      self.shared.end(Some(code));
    }

    fn was_killed(&self) -> bool {
      self.shared.killed.load(Ordering::SeqCst)
    }
  }

  struct FakeControl(Arc<FakeShared>);

  impl MergeProcessControl for FakeControl {
    fn kill(&mut self) -> std::io::Result<()> {
      self.0.killed.store(true, Ordering::SeqCst);
      self.0.end(None);
      Ok(())
    }

    fn wait(&mut self) -> std::io::Result<Option<i32>> {
      Ok(self.0.exit.lock().unwrap().expect("waited on before the process ended"))
    }
  }

  struct Harness {
    queue: Arc<MergeQueue>,
    events: Receiver<MergeEvent>,
    launched: Receiver<FakeProcess>,
    gate: Sender<()>,
  }

  impl Harness {
    fn new(plan: impl Fn(&str) -> Launch + Send + Sync + 'static) -> Self {
      let (event_tx, events) = mpsc::channel();
      let (launch_tx, launched) = mpsc::channel();
      let (gate, gate_rx) = mpsc::channel::<()>();
      let gate_rx = Mutex::new(gate_rx);
      let launcher: MergeLauncher = Box::new(move |file, args| {
        match plan(file) {
          Launch::Run => {}
          Launch::Gated => {
            gate_rx.lock().unwrap().recv_timeout(TIMEOUT).expect("launch released");
          }
          Launch::Fail(message) => anyhow::bail!(message),
          Launch::Panic => panic!("fake launcher panicked"),
        }
        let (reader, writer) = std::io::pipe()?;
        let shared = Arc::new(FakeShared {
          writer: Mutex::new(Some(writer)),
          exit: Mutex::new(None),
          killed: AtomicBool::new(false),
        });
        launch_tx
          .send(FakeProcess {
            file: file.to_owned(),
            args: args.to_vec(),
            shared: Arc::clone(&shared),
          })
          .unwrap();
        Ok(MergeProcess {
          output: Some(Box::new(reader)),
          control: Box::new(FakeControl(shared)),
        })
      });
      let sink: MergeEventSink = Box::new(move |event| {
        let _ = event_tx.send(event);
      });
      Self {
        queue: MergeQueue::new(launcher, sink),
        events,
        launched,
        gate,
      }
    }

    fn enqueue(&self, file: &str) {
      self.queue.enqueue(file.to_owned(), args_for(file)).unwrap();
    }

    fn next(&self) -> Seen {
      match self.events.recv_timeout(TIMEOUT).expect("a merge event") {
        MergeEvent::Started { file, .. } => Seen::Started(file),
        MergeEvent::Progress { file, percent } => Seen::Progress(file, percent),
        MergeEvent::Finished {
          file, outcome, error, ..
        } => Seen::Finished(file, outcome, error),
      }
    }

    fn launch(&self) -> FakeProcess {
      self.launched.recv_timeout(TIMEOUT).expect("a launched process")
    }

    fn release_launch(&self) {
      self.gate.send(()).unwrap();
    }

    fn assert_quiet(&self) {
      if let Ok(event) = self.events.recv_timeout(Duration::from_millis(100)) {
        panic!("unexpected event {event:?}");
      }
    }

    /// `(file, status, progress)` sorted by file.
    fn entries(&self) -> Vec<(String, String, u32)> {
      let mut entries = self.queue.entries();
      entries.sort();
      entries
    }

    /// The drive is freed after `Finished` is sent, so wait for that.
    fn wait_until_idle(&self) {
      let deadline = Instant::now() + TIMEOUT;
      while !self.queue.entries().is_empty() {
        assert!(Instant::now() < deadline, "queue never became idle");
        std::thread::sleep(Duration::from_millis(5));
      }
    }
  }

  fn args_for(file: &str) -> Vec<String> {
    vec!["-o".to_owned(), format!("{file}.out.mkv"), file.to_owned()]
  }

  fn entry(file: &str, status: &str, progress: u32) -> (String, String, u32) {
    (file.to_owned(), status.to_owned(), progress)
  }

  #[test]
  fn events_arrive_as_started_progress_finished() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    assert_eq!(a.file, A);
    assert_eq!(a.args, args_for(A));
    a.report(10);
    a.report(10);
    a.report(55);
    assert_eq!(h.next(), progress(A, 10));
    assert_eq!(h.next(), progress(A, 55));
    a.exit(0);
    assert_eq!(h.next(), finished(A, MergeOutcome::Completed, None));
    h.wait_until_idle();
    h.assert_quiet();
  }

  #[test]
  fn the_queue_lists_merging_and_waiting_files_with_progress() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    h.enqueue(B);
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    a.report(40);
    assert_eq!(h.next(), progress(A, 40));
    assert_eq!(h.entries(), vec![entry(A, "Merging", 40), entry(B, "Waiting", 0)]);
    a.exit(0);
    assert_eq!(h.next(), finished(A, MergeOutcome::Completed, None));
    assert_eq!(h.next(), started(B));
    h.launch().exit(0);
    assert_eq!(h.next(), finished(B, MergeOutcome::Completed, None));
    h.wait_until_idle();
  }

  #[test]
  fn one_drive_merges_one_file_at_a_time_in_fifo_order() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    h.enqueue(B);
    h.enqueue(C);
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    h.assert_quiet();
    a.exit(0);
    assert_eq!(h.next(), finished(A, MergeOutcome::Completed, None));
    assert_eq!(h.next(), started(B));
    let b = h.launch();
    assert_eq!(b.file, B);
    b.exit(3);
    assert_eq!(
      h.next(),
      finished(B, MergeOutcome::Failed, Some("mkvmerge exited with code Some(3)"))
    );
    assert_eq!(h.next(), started(C));
    h.launch().exit(0);
    assert_eq!(h.next(), finished(C, MergeOutcome::Completed, None));
    h.wait_until_idle();
    h.assert_quiet();
  }

  #[test]
  fn enqueueing_a_queued_file_again_is_ignored() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    h.queue.enqueue(A.to_owned(), vec!["other".to_owned()]).unwrap();
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    assert_eq!(a.args, args_for(A));
    a.exit(0);
    assert_eq!(h.next(), finished(A, MergeOutcome::Completed, None));
    h.wait_until_idle();
    h.assert_quiet();
  }

  /// The app may enqueue a file again as soon as its `Finished` arrives; the
  /// sink here does it while the event is being sent. The retry merges.
  #[test]
  fn a_file_enqueued_again_on_its_finished_event_merges_again() {
    struct Done;
    impl MergeProcessControl for Done {
      fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
      }
      fn wait(&mut self) -> std::io::Result<Option<i32>> {
        Ok(Some(0))
      }
    }
    let launches = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let recorder = Arc::clone(&launches);
    let launcher: MergeLauncher = Box::new(move |_, args| {
      recorder.lock().unwrap().push(args.to_vec());
      Ok(MergeProcess {
        output: None,
        control: Box::new(Done),
      })
    });
    let queue_cell: Arc<OnceLock<std::sync::Weak<MergeQueue>>> = Arc::new(OnceLock::new());
    let retry_queue = Arc::clone(&queue_cell);
    let retried = AtomicBool::new(false);
    let (event_tx, events) = mpsc::channel();
    let sink: MergeEventSink = Box::new(move |event| {
      if matches!(event, MergeEvent::Finished { .. }) && !retried.swap(true, Ordering::SeqCst) {
        let queue = retry_queue.get().and_then(std::sync::Weak::upgrade).expect("the queue");
        queue.enqueue(A.to_owned(), vec!["retry".to_owned()]).unwrap();
      }
      let _ = event_tx.send(event);
    });
    let queue = MergeQueue::new(launcher, sink);
    queue_cell.set(Arc::downgrade(&queue)).unwrap();
    queue.enqueue(A.to_owned(), args_for(A)).unwrap();
    let mut seen = Vec::new();
    for _ in 0..4 {
      let event = events.recv_timeout(TIMEOUT).expect("a merge event");
      seen.push(match event {
        MergeEvent::Started { file, .. } => started(&file),
        MergeEvent::Finished { file, outcome, .. } => finished(&file, outcome, None),
        MergeEvent::Progress { file, percent } => progress(&file, percent),
      });
    }
    let done = finished(A, MergeOutcome::Completed, None);
    assert_eq!(
      seen,
      [started(A), finished(A, MergeOutcome::Completed, None), started(A), done]
    );
    assert_eq!(*launches.lock().unwrap(), [args_for(A), vec!["retry".to_owned()]]);
  }

  #[test]
  fn cancelling_a_running_merge_kills_it() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    a.report(20);
    assert_eq!(h.next(), progress(A, 20));
    h.queue.cancel(A).unwrap();
    assert_eq!(h.next(), finished(A, MergeOutcome::Cancelled, None));
    assert!(a.was_killed());
    h.wait_until_idle();
  }

  #[test]
  fn cancelling_a_waiting_file_finishes_it_without_starting_it() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(A);
    h.enqueue(B);
    assert_eq!(h.next(), started(A));
    let a = h.launch();
    h.queue.cancel(B).unwrap();
    assert_eq!(h.next(), finished(B, MergeOutcome::Cancelled, None));
    assert_eq!(h.entries(), vec![entry(A, "Merging", 0)]);
    a.exit(0);
    assert_eq!(h.next(), finished(A, MergeOutcome::Completed, None));
    h.wait_until_idle();
    h.assert_quiet();
  }

  #[test]
  fn cancelling_while_the_process_is_launching_still_kills_it() {
    let h = Harness::new(|_| Launch::Gated);
    h.enqueue(A);
    assert_eq!(h.next(), started(A));
    h.queue.cancel(A).unwrap();
    h.release_launch();
    let a = h.launch();
    assert_eq!(h.next(), finished(A, MergeOutcome::Cancelled, None));
    assert!(a.was_killed());
    h.wait_until_idle();
  }

  #[test]
  fn cancelling_an_unknown_file_does_nothing() {
    let h = Harness::new(|_| Launch::Run);
    h.queue.cancel(A).unwrap();
    h.assert_quiet();
  }

  #[test]
  fn launch_failure_finishes_as_failed_and_starts_the_next_file() {
    let h = Harness::new(|file| {
      if file == A {
        Launch::Fail("MKVMERGE_NOT_AVAILABLE:/opt/mkvmerge: not found")
      } else {
        Launch::Run
      }
    });
    h.enqueue(A);
    h.enqueue(B);
    assert_eq!(h.next(), started(A));
    assert_eq!(
      h.next(),
      finished(
        A,
        MergeOutcome::Failed,
        Some("MKVMERGE_NOT_AVAILABLE:/opt/mkvmerge: not found")
      )
    );
    assert_eq!(h.next(), started(B));
    h.launch().exit(0);
    assert_eq!(h.next(), finished(B, MergeOutcome::Completed, None));
    h.wait_until_idle();
  }

  #[test]
  fn panicking_worker_finishes_as_failed_and_starts_the_next_file() {
    let h = Harness::new(|file| if file == A { Launch::Panic } else { Launch::Run });
    h.enqueue(A);
    h.enqueue(B);
    assert_eq!(h.next(), started(A));
    assert_eq!(h.next(), finished(A, MergeOutcome::Failed, Some(ERROR_WORKER_PANICKED)));
    assert_eq!(h.next(), started(B));
    h.launch().exit(0);
    assert_eq!(h.next(), finished(B, MergeOutcome::Completed, None));
    h.wait_until_idle();
  }

  #[cfg(target_os = "windows")]
  #[test]
  fn different_drives_merge_in_parallel() {
    let h = Harness::new(|_| Launch::Run);
    h.enqueue(r"C:\media\a.mkv");
    h.enqueue(r"D:\media\b.mkv");
    let mut first = vec![h.next(), h.next()];
    first.sort_by_key(|seen| format!("{seen:?}"));
    assert_eq!(first, vec![started(r"C:\media\a.mkv"), started(r"D:\media\b.mkv")]);
    let one = h.launch();
    let two = h.launch();
    one.exit(0);
    two.exit(0);
    h.next();
    h.next();
    h.wait_until_idle();
  }

  #[test]
  fn drive_key_groups_files_per_drive() {
    if cfg!(target_os = "windows") {
      assert_eq!(get_drive_key(r"c:\media\a.mkv"), "C:");
      assert_eq!(get_drive_key(r"\\nas\share\a.mkv"), r"\\NAS\SHARE");
    } else {
      assert_eq!(get_drive_key("/Volumes/one/a.mkv"), "default");
    }
    assert_eq!(get_drive_key("relative/a.mkv"), "default");
  }

  #[test]
  fn outcome_wire_names_match_queue_statuses() {
    assert_eq!(MergeOutcome::Completed.as_str(), "Completed");
    assert_eq!(MergeOutcome::Cancelled.as_str(), "Cancelled");
    assert_eq!(MergeOutcome::Failed.as_str(), "Failed");
  }

  #[test]
  fn event_file_names_the_source_file() {
    let at = SystemTime::now();
    assert_eq!(MergeEvent::Started { file: A.into(), at }.file(), A);
    assert_eq!(
      MergeEvent::Progress {
        file: B.into(),
        percent: 1
      }
      .file(),
      B
    );
    let finished = MergeEvent::Finished {
      file: C.into(),
      outcome: MergeOutcome::Completed,
      error: None,
      at,
    };
    assert_eq!(finished.file(), C);
  }
}
