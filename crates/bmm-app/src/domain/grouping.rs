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

//! Group cards: units whose combined tracks look alike share one card, so a
//! batch edit applies to all of them.

use std::collections::HashMap;

use bmm_core::config::GroupMode;
use bmm_core::media_track::{MediaTrack, MediaTrackType, media_track_counts};
use bmm_core::merge_args::parent_dir;

use super::file_tree::{CombinedTrack, combine_unit_tracks};

/// One entry of the file list: a card of one unit, or a group card of two or
/// more alike units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderEntry {
  Single { members: Vec<String> },
  Group { key: String, units: Vec<Vec<String>> },
}

/// The sorted, comma-joined languages of one track type, so units group only
/// when their per-type languages match.
fn language_signature(tracks: &[CombinedTrack], track_type: MediaTrackType) -> String {
  let mut languages: Vec<&str> = tracks
    .iter()
    .filter(|row| row.track.track_type == track_type)
    .map(|row| row.track.language.as_str())
    .collect();
  languages.sort();
  languages.join(",")
}

/// Each member's track keys (`"{type}:{id}"`, sorted, with `=language` when
/// `with_languages`), members in order. A group card's table shows its first
/// unit and sends each edit to the same key of the same member in every unit,
/// so units group only when these match: then every edit lands on a track
/// that exists and is the same kind of track in each of them.
fn layout_signature(
  members: &[String],
  file_tracks: &HashMap<String, Vec<MediaTrack>>,
  with_languages: bool,
) -> String {
  members
    .iter()
    .map(|file| {
      let mut keys: Vec<String> = file_tracks
        .get(file)
        .into_iter()
        .flatten()
        .map(|track| {
          if with_languages {
            format!("{}={}", track.key(), track.language)
          } else {
            track.key()
          }
        })
        .collect();
      keys.sort();
      keys.join(",")
    })
    .collect::<Vec<_>>()
    .join(";")
}

/// The group key of a unit: its directory and member count, then per-type
/// track counts ([`GroupMode::TrackCount`]) or per-type languages plus the
/// chapter and attachment counts ([`GroupMode::TrackCountAndLanguage`]), then
/// the members' track layout ([`layout_signature`]; with each track's
/// language in the language mode). `None` while a member is not parsed yet,
/// and with [`GroupMode::None`].
pub fn grouping_key(
  members: &[String],
  file_tracks: &HashMap<String, Vec<MediaTrack>>,
  group_mode: &GroupMode,
) -> Option<String> {
  if matches!(group_mode, GroupMode::None) || !members.iter().all(|file| file_tracks.contains_key(file)) {
    return None;
  }
  let combined = combine_unit_tracks(members, file_tracks, None);
  let counts = media_track_counts(combined.iter().map(|row| &row.track));
  // The member count keeps differently shaped trees apart, so the group's
  // per-position batch edits line up across its units.
  let mut key = format!("{}|n={}", parent_dir(&members[0]), members.len());
  if matches!(group_mode, GroupMode::TrackCountAndLanguage) {
    key.push_str(&format!(
      "|v={}|a={}|s={}|c={}|t={}",
      language_signature(&combined, MediaTrackType::Video),
      language_signature(&combined, MediaTrackType::Audio),
      language_signature(&combined, MediaTrackType::Subtitles),
      counts.chapters,
      counts.attachments,
    ));
  } else {
    key.push_str(&format!(
      "|v={}|a={}|s={}|c={}|t={}",
      counts.video, counts.audio, counts.subtitles, counts.chapters, counts.attachments,
    ));
  }
  let with_languages = matches!(group_mode, GroupMode::TrackCountAndLanguage);
  key.push_str(&format!(
    "|ids={}",
    layout_signature(members, file_tracks, with_languages)
  ));
  Some(key)
}

/// The file list's entries for `units` (from
/// [`build_merge_units`](super::file_tree::build_merge_units)). Units with the
/// same key form a group card when there are at least two, placed where the
/// key first appeared; a lone unit stays a single card. Units with a member
/// still loading have no key and come last, as single cards.
pub fn build_render_entries(
  units: Vec<Vec<String>>,
  group_mode: &GroupMode,
  file_tracks: &HashMap<String, Vec<MediaTrack>>,
) -> Vec<RenderEntry> {
  if matches!(group_mode, GroupMode::None) {
    return units
      .into_iter()
      .map(|members| RenderEntry::Single { members })
      .collect();
  }
  let mut buckets: Vec<(String, Vec<Vec<String>>)> = Vec::new();
  let mut bucket_of: HashMap<String, usize> = HashMap::new();
  let mut ungroupable = Vec::new();
  for members in units {
    let Some(key) = grouping_key(&members, file_tracks, group_mode) else {
      ungroupable.push(members);
      continue;
    };
    match bucket_of.get(&key) {
      Some(index) => buckets[*index].1.push(members),
      None => {
        bucket_of.insert(key.clone(), buckets.len());
        buckets.push((key, vec![members]));
      }
    }
  }
  let mut entries = Vec::new();
  for (key, units) in buckets {
    if units.len() >= 2 {
      entries.push(RenderEntry::Group { key, units });
    } else {
      entries.extend(units.into_iter().map(|members| RenderEntry::Single { members }));
    }
  }
  entries.extend(ungroupable.into_iter().map(|members| RenderEntry::Single { members }));
  entries
}

#[cfg(test)]
mod tests {
  use super::*;
  use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
  use bmm_core::media_track::MediaTrackKind;

  fn track(track_type: MediaTrackType, id: i64, language: &str) -> MediaTrack {
    MediaTrack {
      kind: match track_type {
        MediaTrackType::Chapters => MediaTrackKind::Chapters,
        MediaTrackType::Attachment => MediaTrackKind::Attachment,
        _ => MediaTrackKind::Track,
      },
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
      source_track_name: String::new(),
      language: language.to_string(),
      language_codes: Vec::new(),
      default_track: TrackFlag::Unspecified,
      forced: TrackFlag::Unspecified,
    }
  }

  fn english_japanese() -> Vec<MediaTrack> {
    vec![
      track(MediaTrackType::Video, 0, "und"),
      track(MediaTrackType::Audio, 1, "ja"),
      track(MediaTrackType::Audio, 2, "en"),
      track(MediaTrackType::Subtitles, 3, "en"),
      track(MediaTrackType::Chapters, 0, ""),
    ]
  }

  fn file_tracks() -> HashMap<String, Vec<MediaTrack>> {
    let mut japanese_english = english_japanese();
    japanese_english.swap(1, 2);
    let mut french = english_japanese();
    french[1].language = "fr".to_string();
    HashMap::from([
      ("/m/E01.mkv".to_string(), english_japanese()),
      ("/m/E02.mkv".to_string(), japanese_english),
      ("/m/E03.mkv".to_string(), french),
      ("/m/E04.mkv".to_string(), vec![track(MediaTrackType::Video, 0, "und")]),
      ("/n/E05.mkv".to_string(), english_japanese()),
      (
        "/m/E01.en.srt".to_string(),
        vec![track(MediaTrackType::Subtitles, 0, "en")],
      ),
    ])
  }

  fn single(members: &[&str]) -> RenderEntry {
    RenderEntry::Single {
      members: members.iter().map(|member| (*member).to_string()).collect(),
    }
  }

  fn unit(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| (*member).to_string()).collect()
  }

  #[test]
  fn keys_units_by_directory_members_and_counts() {
    let tracks = file_tracks();
    assert_eq!(
      grouping_key(&unit(&["/m/E01.mkv"]), &tracks, &GroupMode::TrackCount).as_deref(),
      Some("/m|n=1|v=1|a=2|s=1|c=1|t=0|ids=audio:1,audio:2,chapters:0,subtitles:3,video:0")
    );
    assert_eq!(
      grouping_key(&unit(&["/m/E01.mkv", "/m/E01.en.srt"]), &tracks, &GroupMode::TrackCount).as_deref(),
      Some("/m|n=2|v=1|a=2|s=2|c=1|t=0|ids=audio:1,audio:2,chapters:0,subtitles:3,video:0;subtitles:0")
    );
    // Keys are sorted, so the order of the tracks inside a file does not matter.
    assert_eq!(
      grouping_key(&unit(&["/m/E02.mkv"]), &tracks, &GroupMode::TrackCountAndLanguage).as_deref(),
      Some("/m|n=1|v=und|a=en,ja|s=en|c=1|t=0|ids=audio:1=ja,audio:2=en,chapters:0=,subtitles:3=en,video:0=und")
    );
    assert_eq!(grouping_key(&unit(&["/m/E01.mkv"]), &tracks, &GroupMode::None), None);
    assert_eq!(
      grouping_key(&unit(&["/m/unparsed.mkv"]), &tracks, &GroupMode::TrackCount),
      None
    );
  }

  #[test]
  fn groups_alike_units_where_the_key_first_appears() {
    let units = vec![
      unit(&["/m/E04.mkv"]),
      unit(&["/m/E01.mkv"]),
      unit(&["/m/loading.mkv"]),
      unit(&["/m/E02.mkv"]),
      unit(&["/m/E03.mkv"]),
      unit(&["/n/E05.mkv"]),
    ];
    let tracks = file_tracks();
    let by_count = build_render_entries(units.clone(), &GroupMode::TrackCount, &tracks);
    assert_eq!(
      by_count,
      [
        single(&["/m/E04.mkv"]),
        RenderEntry::Group {
          key: "/m|n=1|v=1|a=2|s=1|c=1|t=0|ids=audio:1,audio:2,chapters:0,subtitles:3,video:0".to_string(),
          units: vec![unit(&["/m/E01.mkv"]), unit(&["/m/E02.mkv"]), unit(&["/m/E03.mkv"])],
        },
        single(&["/n/E05.mkv"]),
        single(&["/m/loading.mkv"]),
      ]
    );
    let by_language = build_render_entries(units.clone(), &GroupMode::TrackCountAndLanguage, &tracks);
    assert_eq!(by_language.len(), 5);
    assert_eq!(
      by_language[1],
      RenderEntry::Group {
        key: "/m|n=1|v=und|a=en,ja|s=en|c=1|t=0|ids=audio:1=ja,audio:2=en,chapters:0=,subtitles:3=en,video:0=und"
          .to_string(),
        units: vec![unit(&["/m/E01.mkv"]), unit(&["/m/E02.mkv"])],
      }
    );
    assert_eq!(by_language[2], single(&["/m/E03.mkv"]));
    let none = build_render_entries(units.clone(), &GroupMode::None, &tracks);
    assert_eq!(none.len(), units.len());
    assert!(none.iter().all(|entry| matches!(entry, RenderEntry::Single { .. })));
  }

  /// Same counts, different track ids (another muxing order): an edit of
  /// `subtitles:3` would miss the second file, so they get cards of their own.
  /// Same ids with languages swapped between them group by count, not by
  /// language.
  #[test]
  fn units_group_only_when_every_edit_lands_on_the_same_tracks() {
    let mut tracks = file_tracks();
    tracks.insert(
      "/m/E06.mkv".to_string(),
      vec![
        track(MediaTrackType::Video, 0, "und"),
        track(MediaTrackType::Subtitles, 1, "en"),
        track(MediaTrackType::Audio, 2, "ja"),
        track(MediaTrackType::Audio, 3, "en"),
        track(MediaTrackType::Chapters, 0, ""),
      ],
    );
    let mut swapped = english_japanese();
    swapped[1].language = "en".to_string();
    swapped[2].language = "ja".to_string();
    tracks.insert("/m/E07.mkv".to_string(), swapped);
    let units = vec![unit(&["/m/E01.mkv"]), unit(&["/m/E06.mkv"]), unit(&["/m/E07.mkv"])];
    assert_eq!(
      build_render_entries(units.clone(), &GroupMode::TrackCount, &tracks),
      [
        RenderEntry::Group {
          key: "/m|n=1|v=1|a=2|s=1|c=1|t=0|ids=audio:1,audio:2,chapters:0,subtitles:3,video:0".to_string(),
          units: vec![unit(&["/m/E01.mkv"]), unit(&["/m/E07.mkv"])],
        },
        single(&["/m/E06.mkv"]),
      ]
    );
    let by_language = build_render_entries(units, &GroupMode::TrackCountAndLanguage, &tracks);
    assert!(
      by_language
        .iter()
        .all(|entry| matches!(entry, RenderEntry::Single { .. }))
    );
  }
}
