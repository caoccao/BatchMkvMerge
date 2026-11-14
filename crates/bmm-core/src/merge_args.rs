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

//! The `mkvmerge` command line of a merge, built from the selected track rows,
//! plus the profile's track auto-selection and the path helpers the queue and
//! the file list share. Port of the React app's `merge.ts`.
//!
//! The argument layout mirrors mkvtoolnix-gui (`merge/source_file.cpp`,
//! `merge/mux_config.cpp`):
//!
//! ```text
//! mkvmerge -o <out>
//!   { [-d|-a|-s <ids> | --no-video|--no-audio|--no-subtitles]
//!     [--language <id>:<lang>] [--track-name <id>:<name>]
//!     [--default-track-flag <id>:0|1] [--forced-display-flag <id>:0|1]
//!     [--no-chapters] --no-global-tags [--attachments <ids> | --no-attachments]
//!     <input> }…
//!   [--track-order <file>:<id>,…]
//! ```

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::ConfigProfile;
use crate::media_metadata::model::track_properties_common::TrackFlag;
use crate::media_track::{MediaTrack, MediaTrackType};

/// One input file and its selected rows, for a multi-input merge.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeInput {
  pub file: String,
  pub tracks: Vec<MediaTrack>,
}

/// A track of one input, used to spell out an explicit cross-file
/// `--track-order` (the combined table's user-defined order).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OrderedTrackRef {
  pub file: String,
  pub id: i64,
  pub track_type: MediaTrackType,
}

/// The profile's auto-selection rule for a file's rows (`makeTrackSelector`).
///
/// Video, audio and subtitles: with the type's checkbox off every track of the
/// type is selected; with it on only tracks whose language is in the type's
/// comma-separated list (an empty list selects all). Chapters and attachments
/// are selected exactly when their checkbox is on. Anything else is never
/// selected.
#[derive(Debug, Clone)]
pub struct TrackSelector {
  select_video: bool,
  select_audio: bool,
  select_subtitle: bool,
  select_chapters: bool,
  select_attachments: bool,
  video_languages: Option<HashSet<String>>,
  audio_languages: Option<HashSet<String>>,
  subtitle_languages: Option<HashSet<String>>,
}

impl TrackSelector {
  pub fn new(profile: &ConfigProfile) -> Self {
    Self {
      select_video: profile.select_video,
      select_audio: profile.select_audio,
      select_subtitle: profile.select_subtitle,
      select_chapters: profile.select_chapters,
      select_attachments: profile.select_attachments,
      video_languages: parse_language_filter(&profile.video_languages_for_track_selection),
      audio_languages: parse_language_filter(&profile.audio_languages_for_track_selection),
      subtitle_languages: parse_language_filter(&profile.subtitle_languages_for_track_selection),
    }
  }

  /// Whether the profile selects `track`.
  pub fn selects(&self, track: &MediaTrack) -> bool {
    let by_language = |enabled: bool, filter: &Option<HashSet<String>>| {
      !enabled || matches_language(filter.as_ref(), &track.language_codes)
    };
    match track.track_type {
      MediaTrackType::Video => by_language(self.select_video, &self.video_languages),
      MediaTrackType::Audio => by_language(self.select_audio, &self.audio_languages),
      MediaTrackType::Subtitles => by_language(self.select_subtitle, &self.subtitle_languages),
      MediaTrackType::Chapters => self.select_chapters,
      MediaTrackType::Attachment => self.select_attachments,
      MediaTrackType::Buttons | MediaTrackType::Unknown => false,
    }
  }
}

/// A comma-separated language list as a lowercased set; `None` when empty.
fn parse_language_filter(filter: &str) -> Option<HashSet<String>> {
  let items: HashSet<String> = filter
    .split(',')
    .map(|item| item.trim().to_lowercase())
    .filter(|item| !item.is_empty())
    .collect();
  (!items.is_empty()).then_some(items)
}

/// `codes` holds every form of the track's language (terminologic,
/// bibliographic, alpha-2), so a filter written in any form matches.
fn matches_language(filter: Option<&HashSet<String>>, codes: &[String]) -> bool {
  filter.is_none_or(|filter| codes.iter().any(|code| filter.contains(code)))
}

/// Everything before the last `/` or `\` (empty without one). Both separators
/// count on every OS, as in the React app.
pub fn parent_dir(path: &str) -> &str {
  path.rfind(['/', '\\']).map_or("", |slash| &path[..slash])
}

/// Everything after the last `/` or `\` (the whole path without one).
pub fn file_name(path: &str) -> &str {
  path.rfind(['/', '\\']).map_or(path, |slash| &path[slash + 1..])
}

/// The directory a merge writes into: the card's override, else the global
/// output directory, else the source file's own directory. Empty strings
/// count as unset.
pub fn resolve_output_dir(file: &str, override_dir: Option<&str>, global_dir: Option<&str>) -> String {
  if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
    return dir.to_string();
  }
  if let Some(dir) = global_dir.filter(|dir| !dir.is_empty()) {
    return dir.to_string();
  }
  Path::new(file)
    .parent()
    .map(|dir| dir.to_string_lossy().into_owned())
    .unwrap_or_default()
}

/// Quote one argument of the copyable command: values with whitespace,
/// quotes or backslashes (and empty values) are wrapped in double quotes with
/// inner double quotes escaped.
fn shell_quote(value: &str) -> String {
  if !value.is_empty()
    && !value
      .chars()
      .any(|c| crate::js::is_whitespace(c) || matches!(c, '"' | '\'' | '\\'))
  {
    return value.to_string();
  }
  format!("\"{}\"", value.replace('"', "\\\""))
}

fn flag_value(flag: TrackFlag) -> Option<u8> {
  match flag {
    TrackFlag::Unspecified => None,
    TrackFlag::True => Some(1),
    TrackFlag::False => Some(0),
  }
}

fn join_ids<'a>(tracks: impl IntoIterator<Item = &'a MediaTrack>) -> String {
  tracks
    .into_iter()
    .map(|track| track.id.to_string())
    .collect::<Vec<_>>()
    .join(",")
}

/// Append one input's option block and its path. Track ids are the file's own,
/// so the same block serves every input of a multi-input merge. Flags are only
/// written when set, so "unspecified" keeps the source's flag.
fn append_input_options(args: &mut Vec<String>, file: &str, tracks: &[MediaTrack]) {
  let mut media: Vec<&MediaTrack> = Vec::new();
  for (track_type, select_flag, no_flag) in [
    (MediaTrackType::Video, "-d", "--no-video"),
    (MediaTrackType::Audio, "-a", "--no-audio"),
    (MediaTrackType::Subtitles, "-s", "--no-subtitles"),
  ] {
    let selected: Vec<&MediaTrack> = tracks.iter().filter(|track| track.track_type == track_type).collect();
    if selected.is_empty() {
      args.push(no_flag.to_string());
    } else {
      args.push(select_flag.to_string());
      args.push(join_ids(selected.iter().copied()));
    }
    media.extend(selected);
  }

  for track in media {
    if !track.language.is_empty() {
      args.push("--language".to_string());
      args.push(format!("{}:{}", track.id, track.language));
    }
    if !track.track_name.is_empty() {
      args.push("--track-name".to_string());
      args.push(format!("{}:{}", track.id, track.track_name));
    }
    if let Some(value) = flag_value(track.default_track) {
      args.push("--default-track-flag".to_string());
      args.push(format!("{}:{value}", track.id));
    }
    if let Some(value) = flag_value(track.forced) {
      args.push("--forced-display-flag".to_string());
      args.push(format!("{}:{value}", track.id));
    }
  }

  if !tracks.iter().any(|track| track.track_type == MediaTrackType::Chapters) {
    args.push("--no-chapters".to_string());
  }
  args.push("--no-global-tags".to_string());
  let attachments: Vec<&MediaTrack> = tracks
    .iter()
    .filter(|track| track.track_type == MediaTrackType::Attachment)
    .collect();
  if attachments.is_empty() {
    args.push("--no-attachments".to_string());
  } else {
    args.push("--attachments".to_string());
    args.push(join_ids(attachments));
  }

  args.push(file.to_string());
}

/// The `mkvmerge` arguments that merge `source_file`'s selected `tracks` into
/// `output_path`. `tracks` are the selected rows in the table's (possibly
/// reordered) order, which the trailing `--track-order` keeps; the input is
/// file 0.
pub fn build_merge_args(source_file: &str, output_path: &str, tracks: &[MediaTrack]) -> Vec<String> {
  let mut args = vec!["-o".to_string(), output_path.to_string()];
  append_input_options(&mut args, source_file, tracks);
  let order: Vec<String> = tracks
    .iter()
    .filter(|track| track.track_type.is_media())
    .map(|track| format!("0:{}", track.id))
    .collect();
  if !order.is_empty() {
    args.push("--track-order".to_string());
    args.push(order.join(","));
  }
  args
}

/// The `mkvmerge` arguments that merge several `inputs` into one output. Each
/// input writes its own option block and path (file index = position, root
/// first), then one `--track-order` covers every selected video, audio and
/// subtitle track.
///
/// With `order` (the combined table's user-defined order) the track order
/// follows it exactly, keeping only media tracks of files that are inputs, so
/// a reorder that interleaves files and types is honoured. Without it the
/// order is type-major: video, audio, subtitles; within a type by input, then
/// by each file's own order (the default combined table).
pub fn build_merge_args_multi(
  inputs: &[MergeInput],
  output_path: &str,
  order: Option<&[OrderedTrackRef]>,
) -> Vec<String> {
  let mut args = vec!["-o".to_string(), output_path.to_string()];
  for input in inputs {
    append_input_options(&mut args, &input.file, &input.tracks);
  }

  let track_order: Vec<String> = match order {
    Some(order) => {
      // Like a JavaScript `Map` built from the inputs: a repeated file keeps
      // its last index.
      let file_index: HashMap<&str, usize> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.file.as_str(), index))
        .collect();
      order
        .iter()
        .filter(|track| track.track_type.is_media())
        .filter_map(|track| {
          file_index
            .get(track.file.as_str())
            .map(|index| format!("{index}:{}", track.id))
        })
        .collect()
    }
    None => {
      let mut ordered: Vec<(u8, usize, i64)> = Vec::new();
      for (file_index, input) in inputs.iter().enumerate() {
        for track in input.tracks.iter().filter(|track| track.track_type.is_media()) {
          ordered.push((track.track_type.rank(), file_index, track.id));
        }
      }
      // Stable: input order and each file's own order survive within a type.
      ordered.sort_by_key(|(rank, _, _)| *rank);
      ordered
        .into_iter()
        .map(|(_, file_index, id)| format!("{file_index}:{id}"))
        .collect()
    }
  };
  if !track_order.is_empty() {
    args.push("--track-order".to_string());
    args.push(track_order.join(","));
  }
  args
}

/// The `mkvmerge` executable inside the MKVToolNix directory.
fn mkvmerge_path(mkv_toolnix_path: &str) -> String {
  format!("{mkv_toolnix_path}{}mkvmerge", std::path::MAIN_SEPARATOR)
}

fn command_string(mkv_toolnix_path: &str, args: Vec<String>) -> String {
  std::iter::once(mkvmerge_path(mkv_toolnix_path))
    .chain(args)
    .map(|arg| shell_quote(&arg))
    .collect::<Vec<_>>()
    .join(" ")
}

/// The copyable shell command of [`build_merge_args`].
pub fn build_command_string(
  source_file: &str,
  output_path: &str,
  mkv_toolnix_path: &str,
  tracks: &[MediaTrack],
) -> String {
  command_string(mkv_toolnix_path, build_merge_args(source_file, output_path, tracks))
}

/// The copyable shell command of [`build_merge_args_multi`].
pub fn build_command_string_multi(
  inputs: &[MergeInput],
  output_path: &str,
  mkv_toolnix_path: &str,
  order: Option<&[OrderedTrackRef]>,
) -> String {
  command_string(mkv_toolnix_path, build_merge_args_multi(inputs, output_path, order))
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_track::MediaTrackKind;

  fn row(track_type: MediaTrackType, id: i64) -> MediaTrack {
    let kind = match track_type {
      MediaTrackType::Chapters => MediaTrackKind::Chapters,
      MediaTrackType::Attachment => MediaTrackKind::Attachment,
      _ => MediaTrackKind::Track,
    };
    MediaTrack {
      kind,
      id,
      number: 0,
      track_type,
      codec: String::new(),
      codec_id: String::new(),
      description: String::new(),
      chapter_entry_count: None,
      size: None,
      bit_rate: None,
      track_name: String::new(),
      language: String::new(),
      language_codes: Vec::new(),
      default_track: TrackFlag::Unspecified,
      forced: TrackFlag::Unspecified,
    }
  }

  fn with_language(mut track: MediaTrack, codes: &[&str]) -> MediaTrack {
    track.language = codes[0].to_string();
    track.language_codes = codes.iter().map(|code| (*code).to_string()).collect();
    track
  }

  fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
  }

  #[test]
  fn selects_tracks_by_profile() {
    let video = with_language(row(MediaTrackType::Video, 0), &["und"]);
    let english = with_language(row(MediaTrackType::Audio, 1), &["eng", "en"]);
    let french = with_language(row(MediaTrackType::Audio, 2), &["fra", "fre", "fr"]);
    let subtitle = with_language(row(MediaTrackType::Subtitles, 3), &["chi", "zh"]);
    let chapters = row(MediaTrackType::Chapters, 0);
    let attachment = row(MediaTrackType::Attachment, 1);
    let buttons = row(MediaTrackType::Buttons, 4);

    let mut profile = ConfigProfile {
      select_video: false,
      select_audio: true,
      select_subtitle: true,
      select_chapters: false,
      select_attachments: true,
      audio_languages_for_track_selection: " FRE , ".to_string(),
      subtitle_languages_for_track_selection: "jpn".to_string(),
      ..ConfigProfile::default()
    };
    let selector = TrackSelector::new(&profile);
    let picked: Vec<bool> = [&video, &english, &french, &subtitle, &chapters, &attachment, &buttons]
      .iter()
      .map(|track| selector.selects(track))
      .collect();
    assert_eq!(picked, [true, false, true, false, false, true, false]);

    // A checked type with an empty list selects every track of the type.
    profile.subtitle_languages_for_track_selection = " , ".to_string();
    profile.select_chapters = true;
    profile.select_attachments = false;
    let selector = TrackSelector::new(&profile);
    assert!(selector.selects(&subtitle));
    assert!(selector.selects(&chapters));
    assert!(!selector.selects(&attachment));
    profile.select_video = true;
    profile.video_languages_for_track_selection = "eng".to_string();
    assert!(!TrackSelector::new(&profile).selects(&video));
  }

  #[test]
  fn splits_paths_on_either_separator() {
    assert_eq!(parent_dir(r"C:\media\Movie.mkv"), r"C:\media");
    assert_eq!(file_name(r"C:\media\Movie.mkv"), "Movie.mkv");
    assert_eq!(parent_dir("/media/a/Movie.mkv"), "/media/a");
    assert_eq!(file_name("/media/a/Movie.mkv"), "Movie.mkv");
    assert_eq!(parent_dir(r"/mixed\dir/x"), r"/mixed\dir");
    assert_eq!(parent_dir("Movie.mkv"), "");
    assert_eq!(file_name("Movie.mkv"), "Movie.mkv");
    assert_eq!(file_name("/media/"), "");
  }

  #[test]
  fn resolves_the_output_directory() {
    assert_eq!(
      resolve_output_dir("/in/a.mkv", Some("/override"), Some("/global")),
      "/override"
    );
    assert_eq!(resolve_output_dir("/in/a.mkv", Some(""), Some("/global")), "/global");
    assert_eq!(resolve_output_dir("/in/a.mkv", None, Some("")), "/in");
    assert_eq!(resolve_output_dir("/in/a.mkv", None, None), "/in");
    assert_eq!(resolve_output_dir("a.mkv", None, None), "");
    assert_eq!(resolve_output_dir("/", None, None), "");
  }

  #[test]
  fn quotes_shell_arguments_only_when_needed() {
    assert_eq!(shell_quote("plain-arg_1.mkv"), "plain-arg_1.mkv");
    assert_eq!(shell_quote(""), "\"\"");
    assert_eq!(shell_quote("a b"), "\"a b\"");
    assert_eq!(shell_quote("tab\there"), "\"tab\there\"");
    assert_eq!(shell_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
    assert_eq!(shell_quote("it's"), "\"it's\"");
    assert_eq!(shell_quote(r"C:\x"), "\"C:\\x\"");
    assert_eq!(shell_quote("nbsp\u{a0}"), "\"nbsp\u{a0}\"");
  }

  #[test]
  fn builds_a_single_input_command() {
    let mut video = with_language(row(MediaTrackType::Video, 0), &["und"]);
    video.default_track = TrackFlag::True;
    let mut audio = with_language(row(MediaTrackType::Audio, 2), &["ja", "jpn"]);
    audio.track_name = "Japanese".to_string();
    audio.default_track = TrackFlag::False;
    audio.forced = TrackFlag::True;
    let mut subtitle = row(MediaTrackType::Subtitles, 3);
    subtitle.forced = TrackFlag::False;
    let attachment = row(MediaTrackType::Attachment, 1);
    let chapters = row(MediaTrackType::Chapters, 0);
    // Rows in table order after a drag: audio before video.
    let tracks = [audio, video, subtitle, chapters, attachment];
    assert_eq!(
      build_merge_args("/in/Movie.mkv", "/out/Movie.mkv", &tracks),
      strings(&[
        "-o",
        "/out/Movie.mkv",
        "-d",
        "0",
        "-a",
        "2",
        "-s",
        "3",
        "--language",
        "0:und",
        "--default-track-flag",
        "0:1",
        "--language",
        "2:ja",
        "--track-name",
        "2:Japanese",
        "--default-track-flag",
        "2:0",
        "--forced-display-flag",
        "2:1",
        "--forced-display-flag",
        "3:0",
        "--no-global-tags",
        "--attachments",
        "1",
        "/in/Movie.mkv",
        "--track-order",
        "0:2,0:0,0:3",
      ])
    );
  }

  #[test]
  fn writes_negative_options_for_unselected_types() {
    assert_eq!(
      build_merge_args("in.mkv", "out.mkv", &[]),
      strings(&[
        "-o",
        "out.mkv",
        "--no-video",
        "--no-audio",
        "--no-subtitles",
        "--no-chapters",
        "--no-global-tags",
        "--no-attachments",
        "in.mkv",
      ])
    );
    let attachments = [row(MediaTrackType::Attachment, 1), row(MediaTrackType::Attachment, 3)];
    let args = build_merge_args("in.mkv", "out.mkv", &attachments);
    assert_eq!(args[args.len() - 3..], strings(&["--attachments", "1,3", "in.mkv"]));
    assert!(!args.contains(&"--track-order".to_string()));
  }

  fn inputs() -> Vec<MergeInput> {
    vec![
      MergeInput {
        file: "/in/Movie.mkv".to_string(),
        tracks: vec![
          row(MediaTrackType::Video, 0),
          row(MediaTrackType::Subtitles, 3),
          row(MediaTrackType::Audio, 1),
          row(MediaTrackType::Chapters, 0),
        ],
      },
      MergeInput {
        file: "/in/Movie.ja.mka".to_string(),
        tracks: vec![row(MediaTrackType::Audio, 0), row(MediaTrackType::Audio, 1)],
      },
      MergeInput {
        file: "/in/Movie.en.srt".to_string(),
        tracks: vec![row(MediaTrackType::Subtitles, 0)],
      },
    ]
  }

  #[test]
  fn orders_multi_input_tracks_type_major_by_default() {
    let args = build_merge_args_multi(&inputs(), "/out/Movie.mkv", None);
    assert_eq!(&args[..2], ["-o", "/out/Movie.mkv"]);
    let files: Vec<&String> = args.iter().filter(|arg| arg.starts_with("/in/")).collect();
    assert_eq!(files, ["/in/Movie.mkv", "/in/Movie.ja.mka", "/in/Movie.en.srt"]);
    assert_eq!(
      args[args.len() - 2..],
      strings(&["--track-order", "0:0,0:1,1:0,1:1,0:3,2:0"])
    );
    // Each input's block is self-contained: the second has no video, keeps
    // both audio tracks and drops chapters.
    let second = args.iter().position(|arg| arg == "/in/Movie.mkv").unwrap() + 1;
    assert_eq!(
      args[second..second + 8],
      strings(&[
        "--no-video",
        "-a",
        "0,1",
        "--no-subtitles",
        "--no-chapters",
        "--no-global-tags",
        "--no-attachments",
        "/in/Movie.ja.mka",
      ])
    );
  }

  #[test]
  fn follows_an_explicit_cross_file_order() {
    let reference = |file: &str, track_type, id| OrderedTrackRef {
      file: file.to_string(),
      id,
      track_type,
    };
    let order = [
      reference("/in/Movie.en.srt", MediaTrackType::Subtitles, 0),
      reference("/in/Movie.mkv", MediaTrackType::Chapters, 0),
      reference("/in/Movie.ja.mka", MediaTrackType::Audio, 1),
      reference("/in/Other.mkv", MediaTrackType::Video, 0),
      reference("/in/Movie.mkv", MediaTrackType::Video, 0),
    ];
    let args = build_merge_args_multi(&inputs(), "/out/Movie.mkv", Some(&order));
    assert_eq!(args[args.len() - 2..], strings(&["--track-order", "2:0,1:1,0:0"]));
    let none = build_merge_args_multi(&inputs(), "/out/Movie.mkv", Some(&[]));
    assert_ne!(none[none.len() - 2], "--track-order");
  }

  #[test]
  fn builds_copyable_commands() {
    let separator = std::path::MAIN_SEPARATOR;
    let mut audio = row(MediaTrackType::Audio, 1);
    audio.track_name = "Director's cut".to_string();
    let command = build_command_string("/in/My Movie.mkv", "/out/My Movie.mkv", "/opt/mkv", &[audio]);
    let mkvmerge = shell_quote(&format!("/opt/mkv{separator}mkvmerge"));
    assert_eq!(
      command,
      format!(
        "{mkvmerge} -o \"/out/My Movie.mkv\" --no-video -a 1 --no-subtitles --track-name \"1:Director's cut\" \
         --no-chapters --no-global-tags --no-attachments \"/in/My Movie.mkv\" --track-order 0:1"
      )
    );
    let multi = build_command_string_multi(&inputs()[1..], "/out/x.mkv", "/opt/mkv", None);
    assert!(multi.starts_with(&format!("{mkvmerge} -o /out/x.mkv --no-video -a 0,1 ")));
    assert!(multi.ends_with("--track-order 0:0,0:1,1:0"));
  }
}
