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

//! Merge units: which files share one card and one output. Port of the React
//! app's `file-tree.ts`.
//!
//! With *Group by file name* on, files are grouped into "merge trees": in each
//! directory the file with the shortest stem is a root, and every other file
//! whose stem starts with it is a direct child. Manual drag-merges fold whole
//! units into another, and detached files are pulled back out. A unit is the
//! list of its member files, root first.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_track::{MediaTrack, MediaTrackKind, MediaTrackType};
use bmm_core::merge_args::{file_name, parent_dir};

/// One merge tree: `members[0]` is the root, then its children by stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTree {
  pub root: String,
  pub members: Vec<String>,
  pub child_count: usize,
}

/// A row of a multi-file unit's combined table: a member file's track, with
/// the file and its index in the unit (0 = root).
#[derive(Debug, Clone, PartialEq)]
pub struct CombinedTrack {
  pub track: MediaTrack,
  pub source_file: String,
  pub member_index: usize,
}

impl CombinedTrack {
  /// The table's row key, `"{member index}:{type}:{id}"`: unique across the
  /// members and the same for structurally identical units.
  pub fn row_key(&self) -> String {
    format!("{}:{}", self.member_index, self.track.key())
  }

  /// An identity that survives merges and detaches (which shift member
  /// indices): the source file and `"{type}:{id}"`, joined by a NUL, which no
  /// path contains. Custom combined orders are stored with it.
  pub fn combined_id(&self) -> String {
    format!("{}\0{}", self.source_file, self.track.key())
  }

  /// The source file and its row, for an explicit cross-file track order.
  pub fn ordered_ref(&self) -> bmm_core::merge_args::OrderedTrackRef {
    bmm_core::merge_args::OrderedTrackRef {
      file: self.source_file.clone(),
      id: self.track.id,
      track_type: self.track.track_type,
    }
  }
}

/// A combined row key split back into the member index and the per-file
/// `"{type}:{id}"` key. `None` when the key has no member index.
pub fn parse_row_key(row_key: &str) -> Option<(usize, &str)> {
  let (member_index, bare_key) = row_key.split_once(':')?;
  Some((member_index.parse().ok()?, bare_key))
}

/// The file name without its last extension: `Movie.en.srt` → `Movie.en`. A
/// leading dot is not an extension.
pub fn stem_of(path: &str) -> &str {
  let name = file_name(path);
  match name.rfind('.') {
    Some(dot) if dot > 0 => &name[..dot],
    _ => name,
  }
}

/// Stem, then full path, in string order.
fn by_stem_then_path(a: &str, b: &str, stems: &HashMap<&str, &str>) -> Ordering {
  stems[a].cmp(stems[b]).then_with(|| a.cmp(b))
}

/// The *Group by file name* forest. Per directory, files are walked from the
/// shortest stem up; the first unclaimed file becomes a root and claims every
/// unclaimed file whose stem starts with its own (case-sensitive), so each
/// file lands in exactly one tree and the shortest prefix wins. Equal stems
/// (`Movie.mkv`, `Movie.srt`) share a tree; `The.Movie` is not a child of
/// `Movie`. Roots keep the order of `files`.
pub fn build_forest(files: &[String]) -> Vec<FileTree> {
  let stems: HashMap<&str, &str> = files.iter().map(|file| (file.as_str(), stem_of(file))).collect();
  let mut by_dir: HashMap<&str, Vec<&str>> = HashMap::new();
  for file in files {
    by_dir.entry(parent_dir(file)).or_default().push(file);
  }

  let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
  let mut is_root: HashSet<&str> = HashSet::new();
  for dir_files in by_dir.values() {
    let mut ordered = dir_files.clone();
    ordered.sort_by(|a, b| {
      stems[a]
        .len()
        .cmp(&stems[b].len())
        .then_with(|| by_stem_then_path(a, b, &stems))
    });
    let mut claimed: HashSet<&str> = HashSet::new();
    for &root in &ordered {
      if !claimed.insert(root) {
        continue;
      }
      let root_stem = stems[root];
      let mut children = Vec::new();
      for &candidate in &ordered {
        if !claimed.contains(candidate) && stems[candidate].starts_with(root_stem) {
          claimed.insert(candidate);
          children.push(candidate);
        }
      }
      children.sort_by(|a, b| by_stem_then_path(a, b, &stems));
      is_root.insert(root);
      children_of.insert(root, children);
    }
  }

  let mut emitted: HashSet<&str> = HashSet::new();
  files
    .iter()
    .filter(|file| is_root.contains(file.as_str()) && emitted.insert(file.as_str()))
    .map(|root| {
      let children = &children_of[root.as_str()];
      let mut members = vec![root.clone()];
      members.extend(children.iter().map(|child| (*child).to_string()));
      FileTree {
        root: root.clone(),
        members,
        child_count: children.len(),
      }
    })
    .collect()
}

/// A unit's members flattened into one combined table.
///
/// Without `order` (or with an empty one) rows sort by track type, then member
/// (root first), then each file's own order. With `order` (combined ids, e.g.
/// a drag-reorder) rows follow it, and rows it does not list (a newly added
/// member's) follow in the default order. A repeated id counts at its last
/// position.
pub fn combine_unit_tracks(
  members: &[String],
  file_tracks: &HashMap<String, Vec<MediaTrack>>,
  order: Option<&[String]>,
) -> Vec<CombinedTrack> {
  let mut rows: Vec<(CombinedTrack, usize)> = Vec::new();
  for (member_index, file) in members.iter().enumerate() {
    for (within, track) in file_tracks.get(file).into_iter().flatten().enumerate() {
      let combined = CombinedTrack {
        track: track.clone(),
        source_file: file.clone(),
        member_index,
      };
      rows.push((combined, within));
    }
  }
  rows.sort_by(|(a, a_within), (b, b_within)| {
    a.track
      .track_type
      .rank()
      .cmp(&b.track.track_type.rank())
      .then(a.member_index.cmp(&b.member_index))
      .then(a_within.cmp(b_within))
  });
  if let Some(order) = order.filter(|order| !order.is_empty()) {
    let position: HashMap<&str, usize> = order
      .iter()
      .enumerate()
      .map(|(index, id)| (id.as_str(), index))
      .collect();
    rows.sort_by_key(|(row, _)| position.get(row.combined_id().as_str()).copied().unwrap_or(usize::MAX));
  }
  rows.into_iter().map(|(row, _)| row).collect()
}

/// Whether a flag edit sets the default-track or the forced-display flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlagKind {
  Default,
  Forced,
}

/// The value a click on a flag button sets (`nextTrackFlag`): true → false →
/// unspecified → true.
pub fn next_track_flag(flag: TrackFlag) -> TrackFlag {
  match flag {
    TrackFlag::True => TrackFlag::False,
    TrackFlag::False => TrackFlag::Unspecified,
    TrackFlag::Unspecified => TrackFlag::True,
  }
}

/// One `setTrackFlag` call: set `kind` to `value` on rows `keys` of `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagEdit {
  pub file: String,
  pub keys: Vec<String>,
  pub kind: FlagKind,
  pub value: TrackFlag,
}

/// Group `(file, key)` pairs by file, keeping first-seen file order.
fn group_by_file(pairs: Vec<(String, String)>) -> Vec<(String, Vec<String>)> {
  let mut groups: Vec<(String, Vec<String>)> = Vec::new();
  for (file, key) in pairs {
    match groups.iter_mut().find(|(existing, _)| *existing == file) {
      Some((_, keys)) => keys.push(key),
      None => groups.push((file, vec![key])),
    }
  }
  groups
}

/// The "reset default track" and "reset forced display" automation for one
/// unit, over its *selected* parsed tracks in combined order (`order` as in
/// [`combine_unit_tracks`]). With `reset_default` the first selected video,
/// audio and subtitle track become default and the other selected ones of
/// those types lose it; with `reset_forced` every selected track's forced flag
/// becomes unspecified.
///
/// Returns the edits per file in the order the React app applied them: all
/// default-on edits, then default-off, then forced. Units are evaluated one at
/// a time because grouped units can have different layouts.
pub fn apply_unit_flag_automation(
  members: &[String],
  file_tracks: &HashMap<String, Vec<MediaTrack>>,
  file_selected_ids: &HashMap<String, Vec<String>>,
  reset_default: bool,
  reset_forced: bool,
  order: Option<&[String]>,
) -> Vec<FlagEdit> {
  if !reset_default && !reset_forced {
    return Vec::new();
  }
  let mut selected: HashSet<String> = HashSet::new();
  for (member_index, file) in members.iter().enumerate() {
    for bare_key in file_selected_ids.get(file).into_iter().flatten() {
      selected.insert(format!("{member_index}:{bare_key}"));
    }
  }
  let mut default_on = Vec::new();
  let mut default_off = Vec::new();
  let mut forced_clear = Vec::new();
  let mut claimed: HashSet<MediaTrackType> = HashSet::new();
  for row in combine_unit_tracks(members, file_tracks, order) {
    if row.track.kind != MediaTrackKind::Track || !selected.contains(&row.row_key()) {
      continue;
    }
    let pair = (row.source_file.clone(), row.track.key());
    if reset_forced {
      forced_clear.push(pair.clone());
    }
    if reset_default && row.track.track_type.is_media() {
      if claimed.insert(row.track.track_type) {
        default_on.push(pair);
      } else {
        default_off.push(pair);
      }
    }
  }
  let mut edits = Vec::new();
  for (pairs, kind, value) in [
    (default_on, FlagKind::Default, TrackFlag::True),
    (default_off, FlagKind::Default, TrackFlag::False),
    (forced_clear, FlagKind::Forced, TrackFlag::Unspecified),
  ] {
    for (file, keys) in group_by_file(pairs) {
      edits.push(FlagEdit {
        file,
        keys,
        kind,
        value,
      });
    }
  }
  edits
}

/// Fold manual card merges into `units`. `merged_roots` maps an absorbed
/// root to the root it was dropped on; chains (A→B→C) resolve to the last
/// root, whose unit gets the absorbed members appended (flat, in `units`
/// order). An entry whose source is not a root, whose target does not resolve
/// to a surviving root, or that forms a cycle is ignored. Surviving units
/// keep their order.
pub fn apply_manual_merges(units: Vec<Vec<String>>, merged_roots: &HashMap<String, String>) -> Vec<Vec<String>> {
  if merged_roots.is_empty() {
    return units;
  }
  let base_roots: HashSet<&str> = units.iter().map(|unit| unit[0].as_str()).collect();
  let resolve_target = |root: &str| -> Option<String> {
    let mut seen: HashSet<&str> = HashSet::from([root]);
    let mut current = root;
    while let Some(next) = merged_roots.get(current) {
      current = next;
      if !seen.insert(current) {
        return None;
      }
    }
    (current != root && base_roots.contains(current)).then(|| current.to_string())
  };
  let target_of: HashMap<String, String> = units
    .iter()
    .filter(|unit| merged_roots.contains_key(&unit[0]))
    .filter_map(|unit| resolve_target(&unit[0]).map(|target| (unit[0].clone(), target)))
    .collect();
  if target_of.is_empty() {
    return units;
  }
  let mut absorbed: HashMap<&str, Vec<String>> = HashMap::new();
  for unit in &units {
    if let Some(target) = target_of.get(&unit[0]) {
      absorbed
        .entry(target.as_str())
        .or_default()
        .extend(unit.iter().cloned());
    }
  }
  units
    .iter()
    .filter(|unit| !target_of.contains_key(&unit[0]))
    .map(|unit| {
      let mut members = unit.clone();
      if let Some(extra) = absorbed.get(unit[0].as_str()) {
        members.extend(extra.iter().cloned());
      }
      members
    })
    .collect()
}

/// Pull every detached file out of its unit and append it as a unit of its
/// own. Only non-root members can be detached: a root is its card's identity.
pub fn apply_detachments(units: Vec<Vec<String>>, detached_files: &HashSet<String>) -> Vec<Vec<String>> {
  if detached_files.is_empty() {
    return units;
  }
  let mut kept = Vec::with_capacity(units.len());
  let mut extracted = Vec::new();
  for unit in units {
    let mut members = unit.into_iter();
    let mut remaining: Vec<String> = members.next().into_iter().collect();
    for member in members {
      if detached_files.contains(&member) {
        extracted.push(vec![member]);
      } else {
        remaining.push(member);
      }
    }
    kept.push(remaining);
  }
  kept.extend(extracted);
  kept
}

/// The card units of the file list: the forest (or one unit per file when
/// grouping by file name is off), with manual merges folded in and detached
/// files pulled out. One unit is one card and one merge output; the file list
/// and Merge All both use this.
pub fn build_merge_units(
  files: &[String],
  group_by_file_name: bool,
  merged_roots: &HashMap<String, String>,
  detached_files: &HashSet<String>,
) -> Vec<Vec<String>> {
  let base: Vec<Vec<String>> = if group_by_file_name {
    build_forest(files).into_iter().map(|tree| tree.members).collect()
  } else {
    files.iter().map(|file| vec![file.clone()]).collect()
  };
  apply_detachments(apply_manual_merges(base, merged_roots), detached_files)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn s(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
  }

  pub(crate) fn row(track_type: MediaTrackType, id: i64) -> MediaTrack {
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

  fn tracks(entries: &[(&str, Vec<MediaTrack>)]) -> HashMap<String, Vec<MediaTrack>> {
    entries
      .iter()
      .map(|(file, rows)| ((*file).to_string(), rows.clone()))
      .collect()
  }

  #[test]
  fn strips_only_the_last_extension() {
    assert_eq!(stem_of("/m/Movie.en.srt"), "Movie.en");
    assert_eq!(stem_of(r"C:\m\Movie.mkv"), "Movie");
    assert_eq!(stem_of("/m/.hidden"), ".hidden");
    assert_eq!(stem_of("/m/noext"), "noext");
    assert_eq!(stem_of("/m/trailing."), "trailing");
  }

  #[test]
  fn builds_flat_trees_under_the_shortest_stem() {
    let files = s(&[
      "/m/Movie.en.srt",
      "/m/The.Movie.mkv",
      "/m/Movie.mkv",
      "/m/Movie.srt",
      "/m/Movie.Extras.mkv",
      "/n/Movie.ja.mka",
      "/m/Other.mkv",
    ]);
    let forest = build_forest(&files);
    let roots: Vec<&str> = forest.iter().map(|tree| tree.root.as_str()).collect();
    // Roots keep the input order: Movie.mkv's tree root is the first of the
    // equal stems by path; /n has a lone file.
    assert_eq!(
      roots,
      ["/m/The.Movie.mkv", "/m/Movie.mkv", "/n/Movie.ja.mka", "/m/Other.mkv"]
    );
    let movie = &forest[1];
    assert_eq!(
      movie.members,
      s(&["/m/Movie.mkv", "/m/Movie.srt", "/m/Movie.Extras.mkv", "/m/Movie.en.srt"])
    );
    assert_eq!(movie.child_count, 3);
    assert_eq!(forest[0].child_count, 0);
  }

  #[test]
  fn shortest_prefix_wins_and_matching_is_case_sensitive() {
    let files = s(&["/m/AB.mkv", "/m/A.mkv", "/m/ABC.mkv", "/m/a.srt", "/m/ABD.mkv"]);
    let forest = build_forest(&files);
    assert_eq!(forest.len(), 2);
    assert_eq!(forest[0].root, "/m/A.mkv");
    assert_eq!(
      forest[0].members,
      s(&["/m/A.mkv", "/m/AB.mkv", "/m/ABC.mkv", "/m/ABD.mkv"])
    );
    assert_eq!(forest[1].members, s(&["/m/a.srt"]));
  }

  #[test]
  fn sorts_stems_in_string_order() {
    // Children follow Rust's string order (by code point): U+FF21 before
    // U+1F3AC.
    let files = s(&["/m/x\u{1f3ac}.mkv", "/m/x\u{ff21}.mkv", "/m/x.mkv"]);
    let forest = build_forest(&files);
    assert_eq!(
      forest[0].members,
      s(&["/m/x.mkv", "/m/x\u{ff21}.mkv", "/m/x\u{1f3ac}.mkv"])
    );
  }

  #[test]
  fn ignores_repeated_files() {
    let files = s(&["/m/A.mkv", "/m/A.mkv"]);
    assert_eq!(build_forest(&files).len(), 1);
  }

  #[test]
  fn combines_tracks_type_major_then_member_then_file_order() {
    let members = s(&["/m/A.mkv", "/m/A.srt"]);
    let file_tracks = tracks(&[
      (
        "/m/A.mkv",
        vec![
          row(MediaTrackType::Subtitles, 3),
          row(MediaTrackType::Audio, 2),
          row(MediaTrackType::Video, 0),
          row(MediaTrackType::Audio, 1),
          row(MediaTrackType::Attachment, 1),
        ],
      ),
      ("/m/A.srt", vec![row(MediaTrackType::Subtitles, 0)]),
    ]);
    let combined = combine_unit_tracks(&members, &file_tracks, None);
    let keys: Vec<String> = combined.iter().map(CombinedTrack::row_key).collect();
    assert_eq!(
      keys,
      [
        "0:video:0",
        "0:audio:2",
        "0:audio:1",
        "0:subtitles:3",
        "1:subtitles:0",
        "0:attachment:1"
      ]
    );
    assert_eq!(combined[4].combined_id(), "/m/A.srt\0subtitles:0");
    assert_eq!(combined[4].source_file, "/m/A.srt");
    let reference = combined[1].ordered_ref();
    assert_eq!(
      (reference.file.as_str(), reference.id, reference.track_type),
      ("/m/A.mkv", 2, MediaTrackType::Audio)
    );
    // A member without parsed tracks contributes nothing.
    let with_missing = s(&["/m/A.srt", "/m/missing.mkv"]);
    assert_eq!(combine_unit_tracks(&with_missing, &file_tracks, None).len(), 1);
  }

  #[test]
  fn follows_a_custom_combined_order() {
    let members = s(&["/m/A.mkv", "/m/A.srt"]);
    let file_tracks = tracks(&[
      (
        "/m/A.mkv",
        vec![
          row(MediaTrackType::Video, 0),
          row(MediaTrackType::Audio, 1),
          row(MediaTrackType::Subtitles, 2),
        ],
      ),
      ("/m/A.srt", vec![row(MediaTrackType::Subtitles, 0)]),
    ]);
    let order = s(&[
      "/m/A.srt\0subtitles:0",
      "/m/A.mkv\0audio:1",
      "/m/gone.mkv\0video:9",
      "/m/A.srt\0subtitles:0",
    ]);
    let keys: Vec<String> = combine_unit_tracks(&members, &file_tracks, Some(&order))
      .iter()
      .map(CombinedTrack::row_key)
      .collect();
    // Listed rows first (a repeat counts at its last position), then the rest
    // in default order.
    assert_eq!(keys, ["0:audio:1", "1:subtitles:0", "0:video:0", "0:subtitles:2"]);
    let default: Vec<String> = combine_unit_tracks(&members, &file_tracks, Some(&[]))
      .iter()
      .map(CombinedTrack::row_key)
      .collect();
    assert_eq!(default, ["0:video:0", "0:audio:1", "0:subtitles:2", "1:subtitles:0"]);
  }

  #[test]
  fn parses_row_keys() {
    assert_eq!(parse_row_key("1:audio:2"), Some((1, "audio:2")));
    assert_eq!(parse_row_key("0:chapters:0"), Some((0, "chapters:0")));
    assert_eq!(parse_row_key("audio:2"), None);
    assert_eq!(parse_row_key("nokey"), None);
  }

  #[test]
  fn a_flag_click_cycles_true_false_unspecified() {
    assert_eq!(next_track_flag(TrackFlag::True), TrackFlag::False);
    assert_eq!(next_track_flag(TrackFlag::False), TrackFlag::Unspecified);
    assert_eq!(next_track_flag(TrackFlag::Unspecified), TrackFlag::True);
  }

  #[test]
  fn resets_default_and_forced_flags_across_a_unit() {
    let members = s(&["/m/A.mkv", "/m/A.srt"]);
    let file_tracks = tracks(&[
      (
        "/m/A.mkv",
        vec![
          row(MediaTrackType::Video, 0),
          row(MediaTrackType::Audio, 1),
          row(MediaTrackType::Audio, 2),
          row(MediaTrackType::Subtitles, 3),
          row(MediaTrackType::Chapters, 0),
        ],
      ),
      ("/m/A.srt", vec![row(MediaTrackType::Subtitles, 0)]),
    ]);
    let selected: HashMap<String, Vec<String>> = HashMap::from([
      (
        "/m/A.mkv".to_string(),
        s(&["video:0", "audio:2", "audio:1", "chapters:0"]),
      ),
      ("/m/A.srt".to_string(), s(&["subtitles:0"])),
    ]);
    let edit = |file: &str, keys: &[&str], kind, value| FlagEdit {
      file: file.to_string(),
      keys: s(keys),
      kind,
      value,
    };
    assert_eq!(
      apply_unit_flag_automation(&members, &file_tracks, &selected, true, true, None),
      [
        edit("/m/A.mkv", &["video:0", "audio:1"], FlagKind::Default, TrackFlag::True),
        edit("/m/A.srt", &["subtitles:0"], FlagKind::Default, TrackFlag::True),
        edit("/m/A.mkv", &["audio:2"], FlagKind::Default, TrackFlag::False),
        edit(
          "/m/A.mkv",
          &["video:0", "audio:1", "audio:2"],
          FlagKind::Forced,
          TrackFlag::Unspecified
        ),
        edit("/m/A.srt", &["subtitles:0"], FlagKind::Forced, TrackFlag::Unspecified),
      ]
    );
    // With a custom order the first selected audio is the dragged-up one.
    let order = s(&["/m/A.mkv\0audio:2"]);
    assert_eq!(
      apply_unit_flag_automation(&members, &file_tracks, &selected, true, false, Some(&order)),
      [
        edit("/m/A.mkv", &["audio:2", "video:0"], FlagKind::Default, TrackFlag::True),
        edit("/m/A.srt", &["subtitles:0"], FlagKind::Default, TrackFlag::True),
        edit("/m/A.mkv", &["audio:1"], FlagKind::Default, TrackFlag::False),
      ]
    );
    assert!(apply_unit_flag_automation(&members, &file_tracks, &selected, false, false, None).is_empty());
  }

  fn units(values: &[&[&str]]) -> Vec<Vec<String>> {
    values.iter().map(|unit| s(unit)).collect()
  }

  fn merges(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
      .iter()
      .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
      .collect()
  }

  #[test]
  fn folds_manual_merges_and_resolves_chains() {
    let base = units(&[&["A", "a1"], &["B"], &["C", "c1"], &["D"]]);
    assert_eq!(apply_manual_merges(base.clone(), &HashMap::new()), base);
    // A → B → C: both land directly under C, in unit order.
    assert_eq!(
      apply_manual_merges(base.clone(), &merges(&[("A", "B"), ("B", "C")])),
      units(&[&["C", "c1", "A", "a1", "B"], &["D"]])
    );
  }

  #[test]
  fn ignores_cycles_dangling_targets_and_non_roots() {
    let base = units(&[&["A"], &["B"], &["C"]]);
    // A ↔ B is a cycle; C → gone dangles; a1 is not a root.
    assert_eq!(
      apply_manual_merges(
        base.clone(),
        &merges(&[("A", "B"), ("B", "A"), ("C", "gone"), ("a1", "A")])
      ),
      base
    );
    // A target that is not a base root does not resolve either.
    let with_child = units(&[&["A", "x"], &["B"]]);
    assert_eq!(
      apply_manual_merges(with_child.clone(), &merges(&[("B", "x")])),
      with_child
    );
    // A self-merge is no merge.
    assert_eq!(apply_manual_merges(base.clone(), &merges(&[("A", "A")])), base);
  }

  #[test]
  fn pulls_detached_files_out_of_their_units() {
    let base = units(&[&["A", "a1", "a2"], &["B", "b1"]]);
    let detached: HashSet<String> = ["a1", "b1", "B"].iter().map(|file| (*file).to_string()).collect();
    assert_eq!(
      apply_detachments(base.clone(), &detached),
      units(&[&["A", "a2"], &["B"], &["a1"], &["b1"]])
    );
    assert_eq!(apply_detachments(base.clone(), &HashSet::new()), base);
  }

  #[test]
  fn builds_the_card_units() {
    let files = s(&["/m/A.mkv", "/m/A.srt", "/m/B.mkv", "/m/B.en.srt"]);
    let none = HashMap::new();
    let no_detached = HashSet::new();
    assert_eq!(
      build_merge_units(&files, true, &none, &no_detached),
      units(&[&["/m/A.mkv", "/m/A.srt"], &["/m/B.mkv", "/m/B.en.srt"]])
    );
    assert_eq!(build_merge_units(&files, false, &none, &no_detached).len(), 4);
    let merged = merges(&[("/m/B.mkv", "/m/A.mkv")]);
    let detached: HashSet<String> = HashSet::from(["/m/A.srt".to_string()]);
    assert_eq!(
      build_merge_units(&files, true, &merged, &detached),
      units(&[&["/m/A.mkv", "/m/B.mkv", "/m/B.en.srt"], &["/m/A.srt"]])
    );
  }
}
