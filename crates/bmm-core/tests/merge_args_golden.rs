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

//! Golden test of the merge command line. Every `tests/golden/*.json` case
//! names synthetic input files (`tests/golden/fixtures.rs`), a selection
//! (explicit row keys in table order, or a profile's auto-selection), cell
//! edits, and for merge trees an optional explicit track order. The expected
//! rows, `mkvmerge` arguments and copyable command were recorded from the
//! implementation before 1.0.0 on this parser's output for the same files,
//! then reviewed, so they pin the merge command: change them only when the
//! command is meant to change. Here the same files go through the parser,
//! `media_track` and `merge_args`, and everything must match.

#[path = "golden/fixtures.rs"]
mod fixtures;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bmm_core::config::ConfigProfile;
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_metadata::{ParseOptions, parse};
use bmm_core::media_track::{MediaTrack, MediaTrackKind, MediaTrackType, metadata_to_media_tracks};
use bmm_core::merge_args::{
  MergeInput, OrderedTrackRef, TrackSelector, build_command_string, build_command_string_multi, build_merge_args,
  build_merge_args_multi,
};
use serde_json::{Value, json};

fn flag_str(flag: TrackFlag) -> &'static str {
  match flag {
    TrackFlag::Unspecified => "unspecified",
    TrackFlag::True => "true",
    TrackFlag::False => "false",
  }
}

fn parse_flag(value: &Value) -> TrackFlag {
  match value.as_str() {
    Some("true") => TrackFlag::True,
    Some("false") => TrackFlag::False,
    Some("unspecified") => TrackFlag::Unspecified,
    other => panic!("bad flag {other:?}"),
  }
}

fn parse_type(value: &str) -> MediaTrackType {
  match value {
    "video" => MediaTrackType::Video,
    "audio" => MediaTrackType::Audio,
    "subtitles" => MediaTrackType::Subtitles,
    "buttons" => MediaTrackType::Buttons,
    "chapters" => MediaTrackType::Chapters,
    "attachment" => MediaTrackType::Attachment,
    _ => MediaTrackType::Unknown,
  }
}

/// The expected row with `size` and `bitRate` read as `f64`, the type of the
/// row fields (the file writes integral values as JSON integers).
fn float_numbers(row: &Value) -> Value {
  let mut row = row.clone();
  for field in ["size", "bitRate"] {
    if let Some(number) = row[field].as_f64() {
      row[field] = json!(number);
    }
  }
  row
}

/// A row in the JSON shape of the golden files.
fn row_json(track: &MediaTrack) -> Value {
  let kind = match track.kind {
    MediaTrackKind::Track => "track",
    MediaTrackKind::Chapters => "chapters",
    MediaTrackKind::Attachment => "attachment",
  };
  let mut row = json!({
    "kind": kind,
    "id": track.id,
    "number": track.number,
    "type": track.track_type.as_str(),
    "codec": track.codec,
    "codecId": track.codec_id,
    "description": track.description,
    "size": track.size,
    "bitRate": track.bit_rate,
    "trackName": track.track_name,
    "language": track.language,
    "languageCodes": track.language_codes,
    "defaultTrack": flag_str(track.default_track),
    "forced": flag_str(track.forced),
  });
  if let Some(count) = track.chapter_entry_count {
    row["chapterEntryCount"] = json!(count);
  }
  row
}

fn apply_edit(track: &mut MediaTrack, edit: &Value) {
  for (field, value) in edit.as_object().expect("edit object") {
    match field.as_str() {
      "language" => track.language = value.as_str().unwrap().to_string(),
      "trackName" => track.track_name = value.as_str().unwrap().to_string(),
      "defaultTrack" => track.default_track = parse_flag(value),
      "forced" => track.forced = parse_flag(value),
      other => panic!("unsupported edit field {other}"),
    }
  }
}

/// Writes every fixture to a fresh directory and parses it once.
fn parsed_fixtures() -> HashMap<&'static str, Vec<MediaTrack>> {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let dir = std::env::temp_dir().join(format!("bmm-golden-{}-{nanos}", std::process::id()));
  std::fs::create_dir_all(&dir).unwrap();
  let mut rows = HashMap::new();
  for fixture in fixtures::fixtures() {
    let path = dir.join(format!("{}.{}", fixture.name, fixture.extension));
    std::fs::write(&path, &fixture.bytes).unwrap();
    let meta = parse(&path, ParseOptions::default()).unwrap_or_else(|err| panic!("{}: {err}", fixture.name));
    rows.insert(fixture.name, metadata_to_media_tracks(&meta));
  }
  std::fs::remove_dir_all(&dir).unwrap();
  rows
}

fn golden_files() -> Vec<PathBuf> {
  let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden");
  let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
    .collect();
  files.sort();
  files
}

fn strings(value: &Value) -> Vec<String> {
  value
    .as_array()
    .expect("array")
    .iter()
    .map(|item| item.as_str().unwrap().to_string())
    .collect()
}

#[test]
fn merge_arguments_match_the_golden_cases() {
  let parsed = parsed_fixtures();
  let files = golden_files();
  assert!(files.len() >= 4, "golden cases missing: {files:?}");
  for path in files {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let case: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let expected = &case["expected"];

    for (fixture, rows) in expected["rows"].as_object().expect("expected.rows") {
      let actual: Vec<Value> = parsed[fixture.as_str()].iter().map(row_json).collect();
      let expected_rows: Vec<Value> = rows.as_array().expect("rows").iter().map(float_numbers).collect();
      assert_eq!(actual, expected_rows, "{name}: rows of {fixture}");
    }

    let inputs: Vec<MergeInput> = case["inputs"]
      .as_array()
      .unwrap()
      .iter()
      .map(|input| {
        let mut rows = parsed[input["fixture"].as_str().unwrap()].clone();
        if let Some(edits) = input.get("edits").and_then(Value::as_object) {
          for row in &mut rows {
            if let Some(edit) = edits.get(&row.key()) {
              apply_edit(row, edit);
            }
          }
        }
        let tracks = match input.get("selected") {
          Some(selected) => strings(selected)
            .iter()
            .map(|key| {
              rows
                .iter()
                .find(|row| row.key() == *key)
                .unwrap_or_else(|| panic!("{name}: no row {key}"))
                .clone()
            })
            .collect(),
          None => {
            let profile: ConfigProfile = serde_json::from_value(input["profile"].clone()).unwrap();
            let selector = TrackSelector::new(&profile);
            rows.into_iter().filter(|row| selector.selects(row)).collect()
          }
        };
        MergeInput {
          file: input["file"].as_str().unwrap().to_string(),
          tracks,
          sub_charset: None,
        }
      })
      .collect();
    let order: Option<Vec<OrderedTrackRef>> = case.get("order").and_then(Value::as_array).map(|order| {
      order
        .iter()
        .map(|item| OrderedTrackRef {
          file: item["file"].as_str().unwrap().to_string(),
          id: item["id"].as_i64().unwrap(),
          track_type: parse_type(item["type"].as_str().unwrap()),
        })
        .collect()
    });
    let output = case["outputPath"].as_str().unwrap();
    let toolnix = case["mkvToolNixPath"].as_str().unwrap();

    let (args, command) = if case["multi"].as_bool().unwrap() {
      (
        build_merge_args_multi(&inputs, output, order.as_deref()),
        build_command_string_multi(&inputs, output, toolnix, order.as_deref()),
      )
    } else {
      assert_eq!(inputs.len(), 1, "{name}: a single merge has one input");
      (
        build_merge_args(&inputs[0].file, output, &inputs[0].tracks, None),
        build_command_string(&inputs[0].file, output, toolnix, &inputs[0].tracks, None),
      )
    };
    assert_eq!(args, strings(&expected["args"]), "{name}: args");
    // The expected command was generated with `/` as the path separator.
    if std::path::MAIN_SEPARATOR == '/' {
      assert_eq!(command, expected["command"].as_str().unwrap(), "{name}: command");
    }
  }
}
