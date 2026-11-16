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

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmm_core::config::{Config, GroupMode};
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track::TrackType;
use bmm_core::merge::{MergeArgs, MergeEvent, MergeOutcome, MergeProcess, MergeProcessControl, MergeQueue};
use core::prelude::v1::test;
use gpui_kit::{Entity, TestAppContext};

use super::*;
use crate::domain::status::QueueItemStatus;
use crate::merge_events::{self, MergeBackend};
use crate::platform::taskbar::Taskbar;
use crate::state::files::tests::{TempDir, fixture_source, metadata, movie};
use crate::state::files::{FileSettings, FileStore};
use crate::state::ui::UiEvent;
use crate::state::{Persistence, Services};

fn s(values: &[&str]) -> Vec<String> {
  values.iter().map(|value| (*value).to_string()).collect()
}

/// A file store with `fixtures` added in `order` and parsed, without group
/// cards.
fn loaded(cx: &mut TestAppContext, order: &[&str], fixtures: HashMap<String, MediaMetadata>) -> Entity<FileStore> {
  let (source, _) = fixture_source(fixtures);
  let settings = FileSettings {
    group_mode: GroupMode::None,
    ..FileSettings::from_config(&Config::default())
  };
  let store = cx.new(|_| FileStore::with_settings(settings, source));
  store.update(cx, |store, cx| store.add_files(s(order), cx));
  cx.run_until_parked();
  store
}

#[test]
fn the_output_follows_the_overrides() {
  let dir = TempDir::new("output-target");
  let source = dir.file("Movie.mkv");
  let out = TempDir::new("output-target-out");
  let out_dir = out.path().to_string_lossy().into_owned();
  let join = |dir: &std::path::Path, name: &str| dir.join(name).to_string_lossy().into_owned();
  let resolve = |target: &OutputTarget| resolve_merge_output_for(target, &source, &TakenOutputs::default());

  // In place: the source itself is taken, so the name gets a counter.
  assert_eq!(resolve(&OutputTarget::default()), Ok(join(dir.path(), "Movie (1).mkv")));
  // The global directory, then a card's directory win over that.
  let mut target = OutputTarget {
    global_dir: Some(out_dir.clone()),
    ..OutputTarget::default()
  };
  assert_eq!(resolve(&target), Ok(join(out.path(), "Movie.mkv")));
  out.file("Movie.mkv");
  assert_eq!(resolve(&target), Ok(join(out.path(), "Movie (1).mkv")));
  target.dir = Some(String::new());
  assert_eq!(
    resolve(&target),
    Ok(join(out.path(), "Movie (1).mkv")),
    "an empty override counts as unset"
  );
  // A path naming a directory gets the input's name, numbered like the rest.
  target.path = Some(out_dir.clone());
  assert_eq!(resolve(&target), Ok(join(out.path(), "Movie (1).mkv")));
  // A file path is used as given, and refused when a file is there.
  target.path = Some("/somewhere/else.mkv".to_string());
  assert_eq!(resolve(&target), Ok("/somewhere/else.mkv".to_string()));
  let existing = join(out.path(), "Movie.mkv");
  target.path = Some(existing.clone());
  assert_eq!(resolve(&target), Err(existing));
}

/// Resume writes the partial file its stopped run left again, though it is
/// there; only a running merge that writes the file stops it. Without one,
/// it chooses as a new merge: the next free name, or a set file path only
/// while no file is there.
#[test]
fn a_resume_replaces_only_the_partial_file_of_its_stopped_run() {
  let dir = TempDir::new("resume-output");
  let source = dir.file("Movie.mkv");
  let out = TempDir::new("resume-output-out");
  let join = |name: &str| out.path().join(name).to_string_lossy().into_owned();
  let target = OutputTarget {
    global_dir: Some(out.path().to_string_lossy().into_owned()),
    ..OutputTarget::default()
  };
  let none = TakenOutputs::default();
  // The stopped run wrote `Movie (1).mkv`, as another `Movie.mkv` was there.
  out.file("Movie.mkv");
  let partial = out.file("Movie (1).mkv");
  let resume = |target: &OutputTarget, partial: Option<&str>, taken: &TakenOutputs| {
    resolve_resume_output_for(target, &source, partial, taken)
  };
  assert_eq!(resume(&target, Some(&partial), &none), Ok(partial.clone()));
  assert_eq!(
    resume(&target, None, &none),
    Ok(join("Movie (2).mkv")),
    "as a new merge"
  );
  let set = OutputTarget {
    path: Some(join("Movie.mkv")),
    ..target.clone()
  };
  assert_eq!(
    resume(&set, None, &none),
    Err(join("Movie.mkv")),
    "a file that is not the run's own"
  );
  assert_eq!(resume(&set, Some(&join("Movie.mkv")), &none), Ok(join("Movie.mkv")));
  let mut taken = TakenOutputs::default();
  taken.insert(&partial);
  assert_eq!(resume(&target, Some(&partial), &taken), Err(partial));
}

/// A process that has exited with its code.
struct ExitedWith(i32);

impl MergeProcessControl for ExitedWith {
  fn kill(&mut self) -> std::io::Result<()> {
    Ok(())
  }

  fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>> {
    Ok(Some(Some(self.0)))
  }
}

/// The stores over the core's merge queue, which starts a fake mkvmerge: its
/// n-th start writes the n-th of `runs`' contents to its output (`-o`) and
/// exits with that run's code.
struct QueueRuns {
  stores: Stores,
  sink: bmm_core::merge::MergeEventSink,
  relayed: std::sync::mpsc::Receiver<MergeEvent>,
  launched: Arc<Mutex<Vec<String>>>,
}

impl QueueRuns {
  fn new(cx: &mut TestAppContext, files: &[&str], runs: &'static [(&'static str, i32)]) -> Self {
    let launched = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorder = launched.clone();
    let (sink, events) = merge_events::channel();
    let (relay, relayed) = std::sync::mpsc::channel();
    let relay = Mutex::new(relay);
    let queue = MergeQueue::new(
      Box::new(move |_, args| {
        let mut launched = recorder.lock().unwrap();
        let (contents, code) = runs[launched.len()];
        std::fs::write(&args[1], contents).unwrap();
        launched.push(args[1].clone());
        Ok(MergeProcess {
          output: None,
          control: Box::new(ExitedWith(code)),
        })
      }),
      Box::new(move |event| {
        let _ = relay.lock().unwrap().send(event);
      }),
    );
    let fixtures = files.iter().map(|file| (file.to_string(), movie())).collect();
    let (metadata, _) = fixture_source(fixtures);
    let stores = cx.update(|cx| {
      let services = Services {
        persistence: Persistence::new(|_| {}),
        metadata,
        merges: MergeBackend::queue(queue),
        taskbar: Taskbar::none(),
        tools: crate::tools::ToolDetector::system(),
      };
      let stores = Stores::new(Config::default(), services, cx);
      merge_events::start(&stores, events, cx);
      stores
    });
    add(&stores, cx, files);
    Self {
      stores,
      sink,
      relayed,
      launched,
    }
  }

  /// Relays the queue's events, through the test thread, until `file` is
  /// `wanted`.
  fn run_until(&self, cx: &mut TestAppContext, file: &str, wanted: QueueItemStatus) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
      while let Ok(event) = self.relayed.try_recv() {
        (self.sink)(event);
      }
      cx.run_until_parked();
      if status(&self.stores, cx, file) == Some(wanted) {
        break;
      }
      assert!(Instant::now() < deadline, "the merge did not end as {wanted:?}");
      std::thread::sleep(Duration::from_millis(5));
    }
  }

  /// The outputs mkvmerge was started with, in order.
  fn launched(&self) -> Vec<String> {
    self.launched.lock().unwrap().clone()
  }
}

/// A merge is stopped and leaves part of its output, which plays. Resume
/// writes the same file again, and mkvmerge replaces it: with an automatic
/// name, the one the stopped run chose (`a (1).mkv`, as another program's
/// `a.mkv` was there), not a new one.
#[gpui_kit::test]
fn resume_replaces_the_partial_file_of_the_stopped_run(cx: &mut TestAppContext) {
  let out = TempDir::new("resume-replaces");
  let a = "/m/a.mkv";
  // The first run writes part of the file and fails; the resumed run writes
  // it whole.
  let runs = QueueRuns::new(cx, &[a], &[("partial", 2), ("merged", 0)]);
  let stores = &runs.stores;
  let join = |name: &str| out.path().join(name).to_string_lossy().into_owned();
  std::fs::write(join("a.mkv"), b"another program's file").unwrap();
  stores.files.update(cx, |files, cx| {
    files.set_global_output_dir(out.path().to_string_lossy().into_owned(), cx);
  });
  cx.update(|cx| merge_all(stores, cx));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  let partial = join("a (1).mkv");
  assert_eq!(
    std::fs::read(&partial).unwrap(),
    b"partial",
    "left for the user to play"
  );
  cx.update(|cx| resume_merge(stores, a, cx));
  runs.run_until(cx, a, QueueItemStatus::Completed);
  assert_eq!(runs.launched(), [partial.clone(), partial.clone()]);
  assert_eq!(std::fs::read(&partial).unwrap(), b"merged");
  assert_eq!(std::fs::read(join("a.mkv")).unwrap(), b"another program's file");
}

/// A merge stops part-way at its set file path, and its job is cleared from
/// the queue. A fresh merge to that path is a new job: it fails without
/// starting mkvmerge, as the file is there, and its Resume fails the same way
/// instead of replacing the earlier job's partial file.
#[gpui_kit::test]
fn a_fresh_merge_does_not_resume_onto_an_earlier_jobs_partial_file(cx: &mut TestAppContext) {
  let out = TempDir::new("resume-new-job");
  let a = "/m/a.mkv";
  let runs = QueueRuns::new(cx, &[a], &[("partial", 2)]);
  let stores = &runs.stores;
  let set = out.path().join("Chosen.mkv").to_string_lossy().into_owned();
  stores
    .files
    .update(cx, |files, cx| files.set_file_output_path(a, set.clone(), cx));
  cx.update(|cx| merge_all(stores, cx));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  stores.queue.update(cx, |queue, cx| queue.clear_completed(cx));
  assert_eq!(status(stores, cx, a), None);
  cx.update(|cx| merge_all(stores, cx));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  cx.update(|cx| resume_merge(stores, a, cx));
  assert_eq!(status(stores, cx, a), Some(QueueItemStatus::Waiting));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  let error = stores
    .queue
    .read_with(cx, |queue, _| queue.item(a).and_then(|item| item.error.clone()));
  assert_eq!(
    error,
    Some(bmm_core::merge::MergeError::Other(output_exists_message(&set)))
  );
  assert_eq!(runs.launched(), [set.clone()], "started only for the first job");
  assert_eq!(std::fs::read(&set).unwrap(), b"partial");
}

/// A merge whose set file path is there fails without starting mkvmerge, so
/// the file there is not its own: Resume fails the same way and leaves it,
/// here the output of an earlier merge that completed.
#[gpui_kit::test]
fn resume_does_not_replace_a_file_its_run_did_not_write(cx: &mut TestAppContext) {
  let out = TempDir::new("resume-keeps");
  let a = "/m/a.mkv";
  let runs = QueueRuns::new(cx, &[a], &[("merged", 0)]);
  let stores = &runs.stores;
  let set = out.path().join("Chosen.mkv").to_string_lossy().into_owned();
  stores
    .files
    .update(cx, |files, cx| files.set_file_output_path(a, set.clone(), cx));
  cx.update(|cx| merge_all(stores, cx));
  runs.run_until(cx, a, QueueItemStatus::Completed);
  cx.update(|cx| merge_all(stores, cx));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  cx.update(|cx| resume_merge(stores, a, cx));
  assert_eq!(status(stores, cx, a), Some(QueueItemStatus::Waiting));
  runs.run_until(cx, a, QueueItemStatus::Failed);
  let error = stores
    .queue
    .read_with(cx, |queue, _| queue.item(a).and_then(|item| item.error.clone()));
  assert_eq!(
    error,
    Some(bmm_core::merge::MergeError::Other(output_exists_message(&set)))
  );
  assert_eq!(runs.launched(), [set.clone()], "started only for the first merge");
  assert_eq!(std::fs::read(&set).unwrap(), b"merged");
}

#[gpui_kit::test]
fn a_single_member_unit_merges_its_selected_rows(cx: &mut TestAppContext) {
  let store = loaded(cx, &["/m/a.mkv"], HashMap::from([("/m/a.mkv".to_string(), movie())]));
  store.update(cx, |store, cx| {
    store.set_file_output_path("/m/a.mkv", "/out/A.mkv".to_string(), cx)
  });
  store.read_with(cx, |files, _| {
    let request = MergeRequest::unit_selection(files, &s(&["/m/a.mkv"])).expect("selected rows");
    assert!(!request.multi);
    assert_eq!(request.root, "/m/a.mkv");
    assert_eq!(request.inputs[0].tracks.len(), 4);
    assert_eq!(request.output.path.as_deref(), Some("/out/A.mkv"));
    let args = request.args(&request.output_path(&TakenOutputs::default()).unwrap());
    assert_eq!(&args[..2], ["-o", "/out/A.mkv"]);
    assert_eq!(args.last().map(String::as_str), Some("0:0,0:1,0:2,0:3"));
    assert!(MergeRequest::file(files, "/m/a.mkv", Vec::new()).is_none());
    assert!(MergeRequest::unit_selection(files, &[]).is_none());
  });
}

#[gpui_kit::test]
fn a_multi_member_unit_merges_in_its_combined_order(cx: &mut TestAppContext) {
  let fixtures = HashMap::from([
    (
      "/m/a.mkv".to_string(),
      metadata(&[(TrackType::Video, "eng"), (TrackType::Audio, "eng")]),
    ),
    ("/m/a.en.srt".to_string(), metadata(&[(TrackType::Subtitles, "eng")])),
    ("/m/a.ko.srt".to_string(), metadata(&[(TrackType::Subtitles, "kor")])),
  ]);
  let store = loaded(cx, &["/m/a.mkv", "/m/a.en.srt", "/m/a.ko.srt"], fixtures);
  store.update(cx, |store, cx| {
    assert_eq!(store.units().len(), 1);
    // Korean is not among the default profile's subtitle languages.
    assert_eq!(store.selected_ids("/m/a.ko.srt").unwrap(), [] as [String; 0]);
    let order = vec![
      "/m/a.en.srt\0subtitles:0".to_string(),
      "/m/a.mkv\0audio:1".to_string(),
      "/m/a.mkv\0video:0".to_string(),
    ];
    store.set_combined_track_order("/m/a.mkv", order, cx);
  });
  store.read_with(cx, |files, _| {
    let unit = files.units()[0].clone();
    let request = MergeRequest::unit_selection(files, &unit).expect("selected rows");
    assert!(request.multi);
    let inputs: Vec<&str> = request.inputs.iter().map(|input| input.file.as_str()).collect();
    assert_eq!(
      inputs,
      ["/m/a.mkv", "/m/a.en.srt"],
      "an input without selected rows is dropped"
    );
    let order: Vec<(&str, i64)> = request
      .order
      .as_ref()
      .unwrap()
      .iter()
      .map(|track| (track.file.as_str(), track.id))
      .collect();
    assert_eq!(order, [("/m/a.en.srt", 0), ("/m/a.mkv", 1), ("/m/a.mkv", 0)]);
    assert_eq!(
      request.args("/out/a.mkv").last().map(String::as_str),
      Some("1:0,0:1,0:0")
    );
    let empty = vec![bmm_core::merge_args::MergeInput {
      file: "/m/a.mkv".to_string(),
      tracks: Vec::new(),
      sub_charset: None,
    }];
    assert!(MergeRequest::unit(files, "/m/a.mkv", empty, None).is_none());
  });
}

/// Ends a fake merge process at once with exit code 0.
struct FinishedProcess;

impl MergeProcessControl for FinishedProcess {
  fn kill(&mut self) -> std::io::Result<()> {
    Ok(())
  }

  fn try_wait(&mut self) -> std::io::Result<Option<Option<i32>>> {
    Ok(Some(Some(0)))
  }
}

/// Merge All through `bmm_core`'s queue on its own worker threads, with a
/// fake `mkvmerge` that prints two progress lines: every card ends completed,
/// one drive at a time in list order. GPUI's test scheduler rejects a
/// foreground task woken from another thread, so the test thread relays the
/// workers' events into the app's channel.
#[gpui_kit::test]
fn merge_all_runs_through_the_core_queue(cx: &mut TestAppContext) {
  let files = ["/m/a.mkv", "/m/b.mkv", "/m/c.mkv"];
  let launched = Arc::new(Mutex::new(Vec::<(String, Vec<String>)>::new()));
  let recorder = launched.clone();
  let (sink, events) = merge_events::channel();
  let (relay, relayed) = std::sync::mpsc::channel();
  let relay = Mutex::new(relay);
  let queue = MergeQueue::new(
    Box::new(move |file, args| {
      recorder.lock().unwrap().push((file.to_string(), args.to_vec()));
      Ok(MergeProcess {
        output: Some(Box::new(Cursor::new(b"Progress: 30%\nProgress: 100%\n".to_vec()))),
        control: Box::new(FinishedProcess),
      })
    }),
    Box::new(move |event| {
      let _ = relay.lock().unwrap().send(event);
    }),
  );
  let (metadata, _) = fixture_source(files.iter().map(|file| (file.to_string(), movie())).collect());
  let stores = cx.update(|cx| {
    let services = Services {
      persistence: Persistence::new(|_| {}),
      metadata,
      merges: MergeBackend::queue(queue),
      taskbar: Taskbar::none(),
      tools: crate::tools::ToolDetector::system(),
    };
    let stores = Stores::new(Config::default(), services, cx);
    merge_events::start(&stores, events, cx);
    stores
  });
  stores.files.update(cx, |store, cx| store.add_files(s(&files), cx));
  cx.run_until_parked();
  assert!(cx.update(|cx| can_merge_all(&stores, cx)));
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  let deadline = Instant::now() + Duration::from_secs(10);
  loop {
    while let Ok(event) = relayed.try_recv() {
      sink(event);
    }
    cx.run_until_parked();
    let done = stores.queue.read_with(cx, |queue, _| {
      queue.items().count() == 3 && queue.items().all(|item| item.status == QueueItemStatus::Completed)
    });
    if done {
      break;
    }
    assert!(Instant::now() < deadline, "the merges did not finish");
    std::thread::sleep(Duration::from_millis(5));
  }
  let launched: Vec<String> = launched.lock().unwrap().iter().map(|(file, _)| file.clone()).collect();
  assert_eq!(launched, files, "one drive merges one file at a time, in order");
  stores.queue.read_with(cx, |queue, _| {
    assert!(
      queue
        .items()
        .all(|item| item.progress == 100 && item.merge_ended_at.is_some())
    );
    assert!(!queue.has_active());
  });
}

#[gpui_kit::test]
fn cancel_requests_reach_the_queue_and_clear_waits_for_the_merges(cx: &mut TestAppContext) {
  let cancelled = Arc::new(Mutex::new(Vec::<String>::new()));
  let recorder = cancelled.clone();
  let (metadata, _) = fixture_source(HashMap::from([("/m/a.mkv".to_string(), movie())]));
  let stores = cx.update(|cx| {
    let services = Services {
      persistence: Persistence::new(|_| {}),
      metadata,
      merges: MergeBackend::new(
        |_, _| Ok(()),
        move |file| {
          recorder.lock().unwrap().push(file.to_string());
          Err(anyhow::anyhow!("already gone"))
        },
      ),
      taskbar: Taskbar::none(),
      tools: crate::tools::ToolDetector::system(),
    };
    Stores::new(Config::default(), services, cx)
  });
  stores
    .files
    .update(cx, |store, cx| store.add_files(s(&["/m/a.mkv"]), cx));
  cx.run_until_parked();
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  cx.update(|cx| {
    assert!(stores.has_active_merges(cx));
    assert!(!can_clear(&stores, cx));
    clear_files(&stores, cx);
    assert!(
      stores.files.read(cx).has_files(),
      "nothing is cleared while a merge waits"
    );
    // A failing cancel is logged; the request flag stays.
    cancel_all(&stores, cx);
    assert!(stores.queue.read(cx).item("/m/a.mkv").unwrap().cancel_requested);
    remove_file(&stores, "/m/a.mkv", cx);
    assert!(!stores.files.read(cx).has_files());
    assert!(stores.queue.read(cx).item("/m/a.mkv").is_none());
  });
  assert_eq!(cancelled.lock().unwrap().as_slice(), ["/m/a.mkv"]);
}

#[gpui_kit::test]
fn a_failing_enqueue_takes_the_item_off_the_queue(cx: &mut TestAppContext) {
  let (metadata, _) = fixture_source(HashMap::from([("/m/a.mkv".to_string(), movie())]));
  let stores = cx.update(|cx| {
    let services = Services {
      persistence: Persistence::new(|_| {}),
      metadata,
      merges: MergeBackend::new(|_, _| Err(anyhow::anyhow!("no queue")), |_| Ok(())),
      taskbar: Taskbar::none(),
      tools: crate::tools::ToolDetector::system(),
    };
    Stores::new(Config::default(), services, cx)
  });
  stores
    .files
    .update(cx, |store, cx| store.add_files(s(&["/m/a.mkv"]), cx));
  cx.run_until_parked();
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  assert!(stores.queue.read_with(cx, |queue, _| queue.item("/m/a.mkv").is_none()));
  cx.update(|cx| {
    assert!(!stores.has_active_merges(cx));
    let taken = stores.queue.read(cx).outputs().taken();
    assert!(!taken.contains("/m/a.mkv"), "no output was claimed");
  });
}

/// What a recording merge backend was asked to do.
#[derive(Default)]
struct Calls {
  enqueued: Vec<(String, Vec<String>)>,
  cancelled: Vec<String>,
}

impl Calls {
  /// The output (`-o`) `file` was enqueued with.
  fn output(&self, file: &str) -> Option<String> {
    self
      .enqueued
      .iter()
      .find(|(enqueued, _)| enqueued == file)
      .map(|(_, args)| args[1].clone())
  }
}

/// Stores without group cards over `fixtures` whose merge backend records
/// every call, each merge starting at once (its arguments are built then);
/// nothing reports merge events but the test.
fn recording_stores(cx: &mut TestAppContext, fixtures: HashMap<String, MediaMetadata>) -> (Stores, Arc<Mutex<Calls>>) {
  let calls = Arc::new(Mutex::new(Calls::default()));
  let (enqueued, cancelled) = (calls.clone(), calls.clone());
  let (metadata, _) = fixture_source(fixtures);
  let stores = cx.update(|cx| {
    let services = Services {
      persistence: Persistence::new(|_| {}),
      metadata,
      merges: MergeBackend::new(
        move |file, args: MergeArgs| {
          let args = args.build().unwrap_or_default();
          enqueued.lock().unwrap().enqueued.push((file, args));
          Ok(())
        },
        move |file| {
          cancelled.lock().unwrap().cancelled.push(file.to_string());
          Ok(())
        },
      ),
      taskbar: Taskbar::none(),
      tools: crate::tools::ToolDetector::system(),
    };
    let config = Config {
      group_mode: GroupMode::None,
      ..Config::default()
    };
    Stores::new(config, services, cx)
  });
  (stores, calls)
}

fn add(stores: &Stores, cx: &mut TestAppContext, files: &[&str]) {
  stores.files.update(cx, |store, cx| store.add_files(s(files), cx));
  cx.run_until_parked();
}

fn status(stores: &Stores, cx: &mut TestAppContext, file: &str) -> Option<QueueItemStatus> {
  stores
    .queue
    .read_with(cx, |queue, _| queue.item(file).map(|item| item.status))
}

/// What the merge queue reports when `file`'s merge ends.
fn report_finished(stores: &Stores, cx: &mut TestAppContext, file: &str, outcome: MergeOutcome) {
  let at = std::time::SystemTime::now();
  let event = MergeEvent::Finished {
    file: file.to_string(),
    outcome,
    error: None,
    warnings: Vec::new(),
    launched: true,
    at,
  };
  stores.queue.update(cx, |queue, cx| queue.apply_event(&event, at, cx));
}

/// Two `Episode.srt` of one Merge All into one directory, and a third merged
/// while they run: each gets an output of its own, as mkvmerge would
/// overwrite a shared one. An output is free again once its merge ends.
#[gpui_kit::test]
fn merges_into_one_directory_never_share_an_output(cx: &mut TestAppContext) {
  let out = TempDir::new("shared-output");
  let out_dir = out.path().to_string_lossy().into_owned();
  let join = |name: &str| out.path().join(name).to_string_lossy().into_owned();
  let (a, b, c) = ("/m/A/Episode.srt", "/m/B/Episode.srt", "/m/C/Episode.srt");
  let subtitles = || metadata(&[(TrackType::Subtitles, "eng")]);
  let fixtures = HashMap::from([
    (a.into(), subtitles()),
    (b.into(), subtitles()),
    (c.into(), subtitles()),
  ]);
  let (stores, calls) = recording_stores(cx, fixtures);
  add(&stores, cx, &[a, b]);
  stores
    .files
    .update(cx, |files, cx| files.set_global_output_dir(out_dir, cx));
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  assert_eq!(calls.lock().unwrap().output(a), Some(join("Episode.mkv")));
  assert_eq!(calls.lock().unwrap().output(b), Some(join("Episode (1).mkv")));

  add(&stores, cx, &[c]);
  cx.update(|cx| {
    let request = MergeRequest::unit_selection(stores.files.read(cx), &s(&[c]));
    let copy = copy_commands(&stores, request.clone().into_iter().collect(), cx);
    copy.detach();
    enqueue(&stores, request.into_iter().collect(), cx);
  });
  cx.run_until_parked();
  assert_eq!(calls.lock().unwrap().output(c), Some(join("Episode (2).mkv")));
  let copied = cx.read(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
  assert!(
    copied.is_some_and(|command| command.contains("Episode (2).mkv")),
    "Copy Command shows the output Merge writes"
  );

  report_finished(&stores, cx, a, MergeOutcome::Completed);
  let taken = stores.queue.read_with(cx, |queue, _| queue.outputs().taken());
  assert!(!taken.contains(&join("Episode.mkv")));
  assert!(taken.contains(&join("Episode (1).mkv")) && taken.contains(&join("Episode (2).mkv")));
}

/// A file path set on a card is never replaced: a merge whose path holds a
/// file when it starts fails, and Copy Command copies nothing and says so.
#[gpui_kit::test]
fn a_set_output_path_that_holds_a_file_fails_the_merge(cx: &mut TestAppContext) {
  let _locale = crate::i18n::tests::LOCALE_LOCK
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  let out = TempDir::new("set-output");
  let target = out.path().join("Chosen.mkv").to_string_lossy().into_owned();
  let a = "/m/A/Pilot.srt";
  let subtitles = metadata(&[(TrackType::Subtitles, "eng")]);
  let (stores, calls) = recording_stores(cx, HashMap::from([(a.into(), subtitles)]));
  let snackbars = Rc::new(RefCell::new(Vec::<String>::new()));
  let sink = snackbars.clone();
  let _subscription = cx.update(|cx| {
    cx.subscribe(&stores.ui, move |_, event: &UiEvent, _| {
      if let UiEvent::Snackbar(snackbar) = event {
        sink.borrow_mut().push(snackbar.message.clone());
      }
    })
  });
  add(&stores, cx, &[a]);
  stores
    .files
    .update(cx, |files, cx| files.set_file_output_path(a, target.clone(), cx));
  std::fs::write(&target, b"another merge's output").unwrap();
  let requests = cx.update(|cx| MergeRequest::unit_selection(stores.files.read(cx), &s(&[a])));
  cx.update(|cx| copy_commands(&stores, requests.into_iter().collect(), cx))
    .detach();
  cx.run_until_parked();
  let exists = output_exists_message(&target);
  assert_eq!(cx.read(|cx| cx.read_from_clipboard()), None);
  assert_eq!(snackbars.borrow().as_slice(), std::slice::from_ref(&exists));
  // The recording merge queue starts it at once: building its arguments fails.
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  assert_eq!(calls.lock().unwrap().enqueued, [(a.to_string(), Vec::new())]);
  assert_eq!(
    resolve_merge_output_for(
      &cx.read(|cx| OutputTarget::of(stores.files.read(cx), a)),
      a,
      &TakenOutputs::default()
    ),
    Err(target.clone())
  );
  assert_eq!(std::fs::read(&target).unwrap(), b"another merge's output");
}

/// Merge All lists every root as waiting and hands it to the merge queue at
/// once: Clear waits for them, and a deleted or cancelled one is cancelled
/// in the merge queue.
#[gpui_kit::test]
fn merge_all_hands_every_root_to_the_merge_queue_at_once(cx: &mut TestAppContext) {
  let (a, b) = ("/m/a.mkv", "/m/b.mkv");
  let (stores, calls) = recording_stores(cx, HashMap::from([(a.into(), movie()), (b.into(), movie())]));
  add(&stores, cx, &[a, b]);
  cx.update(|cx| {
    merge_all(&stores, cx);
    let enqueued: Vec<String> = calls
      .lock()
      .unwrap()
      .enqueued
      .iter()
      .map(|(file, _)| file.clone())
      .collect();
    assert_eq!(enqueued, [a, b]);
    assert!(stores.has_active_merges(cx));
    assert!(!can_clear(&stores, cx));
    clear_files(&stores, cx);
    assert_eq!(stores.files.read(cx).files().len(), 2, "Clear waits for the merges");
    assert!(delete_unit(&stores, &s(&[a]), cx));
    cancel_merge(&stores, b, cx);
  });
  assert_eq!(calls.lock().unwrap().cancelled, [a, b]);
}

/// Deleting a waiting card cancels its merge; the merge queue's late
/// `Cancelled` lists nothing again, and the merge counts as active until it.
#[gpui_kit::test]
fn a_deleted_waiting_merge_does_not_come_back(cx: &mut TestAppContext) {
  let a = "/m/a.mkv";
  let (stores, calls) = recording_stores(cx, HashMap::from([(a.into(), movie())]));
  add(&stores, cx, &[a]);
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  assert!(cx.update(|cx| delete_unit(&stores, &s(&[a]), cx)));
  assert_eq!(calls.lock().unwrap().cancelled, [a]);
  assert_eq!(status(&stores, cx, a), None);
  assert!(
    cx.update(|cx| stores.has_active_merges(cx)),
    "until the merge queue confirms"
  );
  report_finished(&stores, cx, a, MergeOutcome::Cancelled);
  assert_eq!(status(&stores, cx, a), None);
  stores.queue.read_with(cx, |queue, _| {
    assert_eq!(queue.items().count(), 0);
    assert!(!queue.has_active());
  });
}

/// A card regrouped around a file whose own merge runs cannot be deleted:
/// the merge would run on untracked.
#[gpui_kit::test]
fn a_unit_with_a_merging_member_is_not_deleted(cx: &mut TestAppContext) {
  let (a, b) = ("/m/a.mkv", "/m/b.mkv");
  let (stores, _) = recording_stores(cx, HashMap::from([(a.into(), movie()), (b.into(), movie())]));
  add(&stores, cx, &[a, b]);
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  let at = std::time::SystemTime::now();
  let started = MergeEvent::Started {
    file: b.to_string(),
    at,
  };
  stores.queue.update(cx, |queue, cx| queue.apply_event(&started, at, cx));
  cx.update(|cx| {
    assert!(!delete_unit(&stores, &s(&[a, b]), cx));
    clear_units(&stores, &[s(&[a, b])], cx);
    assert_eq!(stores.files.read(cx).files().len(), 2);
    assert!(stores.has_active_merges(cx));
  });
  assert_eq!(status(&stores, cx, b), Some(QueueItemStatus::Merging));
}

/// Merges wait for their drive while files are written to their outputs:
/// when its turn comes, a merge with an automatic name picks the next free
/// one, and a merge with a set path fails, both instead of replacing a file.
#[gpui_kit::test]
fn a_waiting_merge_chooses_its_output_when_it_starts(cx: &mut TestAppContext) {
  let out = TempDir::new("chosen-at-start");
  let (a, b, c) = ("/m/a.mkv", "/m/b.mkv", "/m/c.mkv");
  let launched = Arc::new(Mutex::new(Vec::<(String, Vec<String>)>::new()));
  let recorder = launched.clone();
  let (gate, held) = std::sync::mpsc::channel::<()>();
  let held = Mutex::new(held);
  let (sink, events) = merge_events::channel();
  let (relay, relayed) = std::sync::mpsc::channel();
  let relay = Mutex::new(relay);
  let queue = MergeQueue::new(
    Box::new(move |file, args| {
      if file == a {
        held
          .lock()
          .unwrap()
          .recv_timeout(Duration::from_secs(10))
          .expect("released");
      }
      recorder.lock().unwrap().push((file.to_string(), args.to_vec()));
      Ok(MergeProcess {
        output: None,
        control: Box::new(FinishedProcess),
      })
    }),
    Box::new(move |event| {
      let _ = relay.lock().unwrap().send(event);
    }),
  );
  let fixtures = [a, b, c].map(|file| (file.to_string(), movie()));
  let (metadata, _) = fixture_source(HashMap::from(fixtures));
  let stores = cx.update(|cx| {
    let services = Services {
      persistence: Persistence::new(|_| {}),
      metadata,
      merges: MergeBackend::queue(queue),
      taskbar: Taskbar::none(),
      tools: crate::tools::ToolDetector::system(),
    };
    let config = Config {
      group_mode: GroupMode::None,
      ..Config::default()
    };
    let stores = Stores::new(config, services, cx);
    merge_events::start(&stores, events, cx);
    stores
  });
  add(&stores, cx, &[a, b, c]);
  let join = |name: &str| out.path().join(name).to_string_lossy().into_owned();
  let chosen = join("Chosen.mkv");
  stores.files.update(cx, |files, cx| {
    files.set_global_output_dir(out.path().to_string_lossy().into_owned(), cx);
    files.set_file_output_path(c, chosen.clone(), cx);
  });
  cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  // A's launch is held, so B and C wait for the drive; files appear at their
  // outputs.
  std::fs::write(join("b.mkv"), b"another program's file").unwrap();
  std::fs::write(&chosen, b"another program's file").unwrap();
  gate.send(()).unwrap();
  let deadline = Instant::now() + Duration::from_secs(10);
  loop {
    while let Ok(event) = relayed.try_recv() {
      sink(event);
    }
    cx.run_until_parked();
    let done = stores.queue.read_with(cx, |queue, _| {
      queue.items().count() == 3 && queue.items().all(|item| item.status.is_terminal())
    });
    if done {
      break;
    }
    assert!(Instant::now() < deadline, "the merges did not finish");
    std::thread::sleep(Duration::from_millis(5));
  }
  let launched = launched.lock().unwrap().clone();
  // `mkvmerge` is given the output chosen at the start.
  let (output_a, output_b) = (join("a.mkv"), join("b (1).mkv"));
  let written: Vec<(&str, &str)> = launched
    .iter()
    .map(|(file, args)| (file.as_str(), args[1].as_str()))
    .collect();
  assert_eq!(written, [(a, output_a.as_str()), (b, output_b.as_str())]);
  let mut names: Vec<String> = std::fs::read_dir(out.path())
    .unwrap()
    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  names.sort();
  // The fake `mkvmerge` writes nothing, and the merge creates nothing.
  assert_eq!(names, ["Chosen.mkv", "b.mkv"], "the other program's files only");
  let failed = stores.queue.read_with(cx, |queue, _| queue.item(c).cloned()).unwrap();
  assert_eq!(failed.status, QueueItemStatus::Failed);
  assert!(failed.error.is_some_and(|error| error.to_string().contains(&chosen)));
  for path in [join("b.mkv"), chosen] {
    assert_eq!(std::fs::read(path).unwrap(), b"another program's file");
  }
  assert!(
    !stores
      .queue
      .read_with(cx, |queue, _| queue.outputs().taken().contains(&output_b))
  );
}
