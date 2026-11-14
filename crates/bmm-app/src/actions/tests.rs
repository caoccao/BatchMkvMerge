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

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmm_core::config::{Config, GroupMode};
use bmm_core::media_metadata::model::MediaMetadata;
use bmm_core::media_metadata::model::track::TrackType;
use bmm_core::merge::{MergeProcess, MergeProcessControl, MergeQueue};
use core::prelude::v1::test;
use gpui_kit::{Entity, TestAppContext};

use super::*;
use crate::domain::status::QueueItemStatus;
use crate::merge_events::{self, MergeBackend};
use crate::platform::taskbar::Taskbar;
use crate::state::files::tests::{TempDir, fixture_source, metadata, movie};
use crate::state::files::{FileSettings, FileStore};
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

  // In place: the source itself is taken, so the name gets a counter.
  let target = OutputTarget::default();
  assert_eq!(
    resolve_merge_output_for(&target, &source),
    join(dir.path(), "Movie (1).mkv")
  );
  // The global directory, then a card's directory win over that.
  let mut target = OutputTarget {
    global_dir: Some(out_dir.clone()),
    ..OutputTarget::default()
  };
  assert_eq!(
    resolve_merge_output_for(&target, &source),
    join(out.path(), "Movie.mkv")
  );
  out.file("Movie.mkv");
  assert_eq!(
    resolve_merge_output_for(&target, &source),
    join(out.path(), "Movie (1).mkv")
  );
  target.dir = Some(String::new());
  assert_eq!(
    resolve_merge_output_for(&target, &source),
    join(out.path(), "Movie (1).mkv"),
    "an empty override counts as unset"
  );
  // A full path is used as given, an existing directory gets the input's name.
  target.path = Some("/somewhere/else.mkv".to_string());
  assert_eq!(resolve_merge_output_for(&target, &source), "/somewhere/else.mkv");
  target.path = Some(out_dir.clone());
  assert_eq!(
    resolve_merge_output_for(&target, &source),
    join(out.path(), "Movie.mkv")
  );
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
    let args = request.resolve();
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
    assert_eq!(request.resolve().last().map(String::as_str), Some("1:0,0:1,0:0"));
    let empty = vec![bmm_core::merge_args::MergeInput {
      file: "/m/a.mkv".to_string(),
      tracks: Vec::new(),
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

  fn wait(&mut self) -> std::io::Result<Option<i32>> {
    Ok(Some(0))
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
    };
    let stores = Stores::new(Config::default(), services, cx);
    merge_events::start(&stores, events, cx);
    stores
  });
  stores.files.update(cx, |store, cx| store.add_files(s(&files), cx));
  cx.run_until_parked();
  assert!(cx.update(|cx| can_merge_all(&stores, cx)));
  let task = cx.update(|cx| merge_all(&stores, cx));
  cx.run_until_parked();
  drop(task);
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
    };
    Stores::new(Config::default(), services, cx)
  });
  stores
    .files
    .update(cx, |store, cx| store.add_files(s(&["/m/a.mkv"]), cx));
  cx.run_until_parked();
  cx.update(|cx| merge_all(&stores, cx)).detach();
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
    };
    Stores::new(Config::default(), services, cx)
  });
  stores
    .files
    .update(cx, |store, cx| store.add_files(s(&["/m/a.mkv"]), cx));
  cx.run_until_parked();
  cx.update(|cx| merge_all(&stores, cx)).detach();
  cx.run_until_parked();
  assert!(stores.queue.read_with(cx, |queue, _| queue.item("/m/a.mkv").is_none()));
}
