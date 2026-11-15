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

//! The flattened track rows the selection table renders, derived from a parsed
//! [`MediaMetadata`].
//!
//! The parser keeps `video / audio / subtitle` sub-trees on each [`Track`],
//! plus a chapter summary and an attachment list on the file. The table needs
//! one row per track plus synthetic rows for chapters and attachments, so they
//! are adapted here. Nothing in this module touches the file system.

use crate::media_metadata::model::metadata::MediaMetadata;
use crate::media_metadata::model::track::{Track, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioTrackProperties, ChannelLayoutKind};
use crate::media_metadata::model::track_properties_common::TrackFlag;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;

/// Which kind of row a [`MediaTrack`] is: a parsed track, the one synthetic
/// chapters row, or one synthetic row per attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaTrackKind {
  Track,
  Chapters,
  Attachment,
}

/// The row's media type, spelled like mkvmerge `-J`'s `type` (see
/// [`MediaTrackType::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaTrackType {
  Video,
  Audio,
  Subtitles,
  Buttons,
  Chapters,
  Attachment,
  Unknown,
}

impl MediaTrackType {
  /// The lowercase spelling used in row keys (`video:1`) and the merge command.
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Video => "video",
      Self::Audio => "audio",
      Self::Subtitles => "subtitles",
      Self::Buttons => "buttons",
      Self::Chapters => "chapters",
      Self::Attachment => "attachment",
      Self::Unknown => "unknown",
    }
  }

  /// Canonical sort rank wherever tracks are shown or ordered: video, audio,
  /// subtitles, buttons, then the synthetic chapters and attachment rows, then
  /// anything unknown. The combined merge-tree table and the multi-input merge
  /// command both sort by it, so they stay in step.
  pub fn rank(self) -> u8 {
    match self {
      Self::Video => 0,
      Self::Audio => 1,
      Self::Subtitles => 2,
      Self::Buttons => 3,
      Self::Chapters => 4,
      Self::Attachment => 5,
      Self::Unknown => 6,
    }
  }

  /// The three media types that take part in `--track-order`, language
  /// filters and default-track automation.
  pub fn is_media(self) -> bool {
    matches!(self, Self::Video | Self::Audio | Self::Subtitles)
  }
}

impl From<TrackType> for MediaTrackType {
  fn from(track_type: TrackType) -> Self {
    match track_type {
      TrackType::Video => Self::Video,
      TrackType::Audio => Self::Audio,
      TrackType::Subtitles => Self::Subtitles,
      TrackType::Buttons => Self::Buttons,
      TrackType::Unknown => Self::Unknown,
    }
  }
}

/// One row of the track selection table.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaTrack {
  pub kind: MediaTrackKind,
  /// Track id for parsed tracks, attachment id, or 0 for the chapters row.
  pub id: i64,
  /// Track number for parsed tracks; 0 for synthetic rows.
  pub number: u64,
  pub track_type: MediaTrackType,
  /// Human-readable codec name (falls back to the codec id). For attachments
  /// the file extension or mime subtype; empty for the chapters row.
  pub codec: String,
  /// Raw container codec id (`V_MPEG4/ISO/AVC`, a FOURCC, ...). Empty for the
  /// chapters row.
  pub codec_id: String,
  /// Short summary of the domain properties: resolution and frame rate
  /// (video), channel layout and sample rate (audio), text/image, variant and
  /// encoding (subtitles). Empty for synthetic rows.
  pub description: String,
  /// Number of chapter entries, on the chapters row only.
  pub chapter_entry_count: Option<u32>,
  /// Payload size in bytes (Matroska `NUMBER_OF_BYTES` statistics tag, or the
  /// attachment's stored size), when known.
  pub size: Option<f64>,
  /// Bit rate in bits per second (Matroska `BPS` statistics tag), when known.
  pub bit_rate: Option<f64>,
  /// Track name; the file name for attachments; empty for the chapters row.
  pub track_name: String,
  /// The language shown and emitted in the merge command: ISO 639-1 when one
  /// exists, else ISO 639-2 (`und` without a language). Empty for synthetic
  /// rows.
  pub language: String,
  /// Every equivalent code (terminologic, bibliographic, alpha-2), lowercased,
  /// so a filter in any form matches. Empty for synthetic rows.
  pub language_codes: Vec<String>,
  /// Raw `FlagDefault` tri-state; [`TrackFlag::Unspecified`] for synthetic rows.
  pub default_track: TrackFlag,
  /// Raw `FlagForced` tri-state; [`TrackFlag::Unspecified`] for synthetic rows.
  pub forced: TrackFlag,
}

impl MediaTrack {
  /// The per-file row key, `"{type}:{id}"`. The selection and flag maps are
  /// keyed by it.
  pub fn key(&self) -> String {
    format!("{}:{}", self.track_type.as_str(), self.id)
  }
}

/// Track counts per row type, used to decide whether files can share a group
/// card. Derived from the rows so the counts match what the table shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MediaTrackCounts {
  pub video: usize,
  pub audio: usize,
  pub subtitles: usize,
  pub chapters: usize,
  pub attachments: usize,
}

/// Count the rows per type; buttons and unknown rows are not counted.
pub fn media_track_counts<'a>(tracks: impl IntoIterator<Item = &'a MediaTrack>) -> MediaTrackCounts {
  let mut counts = MediaTrackCounts::default();
  for track in tracks {
    match track.track_type {
      MediaTrackType::Video => counts.video += 1,
      MediaTrackType::Audio => counts.audio += 1,
      MediaTrackType::Subtitles => counts.subtitles += 1,
      MediaTrackType::Chapters => counts.chapters += 1,
      MediaTrackType::Attachment => counts.attachments += 1,
      MediaTrackType::Buttons | MediaTrackType::Unknown => {}
    }
  }
  counts
}

/// Flatten a parsed file into table rows: one per track, then one chapters row
/// when the file has chapter entries, then one row per attachment.
pub fn metadata_to_media_tracks(meta: &MediaMetadata) -> Vec<MediaTrack> {
  let mut rows = Vec::with_capacity(meta.tracks.len() + meta.attachments.len() + 1);
  for track in &meta.tracks {
    let common = &track.properties.common;
    rows.push(MediaTrack {
      kind: MediaTrackKind::Track,
      id: track.id,
      number: common.number.unwrap_or(0),
      track_type: track.track_type.into(),
      codec: track.codec.name.clone().unwrap_or_else(|| track.codec.id.clone()),
      codec_id: track.codec.id.clone(),
      description: track_description(track),
      chapter_entry_count: None,
      size: stat_tag_number(track, "NUMBER_OF_BYTES"),
      bit_rate: stat_tag_number(track, "BPS"),
      track_name: common.track_name.clone().unwrap_or_default(),
      language: pick_track_language(track),
      language_codes: track_language_codes(track),
      default_track: common.default,
      forced: common.forced,
    });
  }
  if meta.chapters.num_entries > 0 {
    rows.push(MediaTrack {
      kind: MediaTrackKind::Chapters,
      id: 0,
      number: 0,
      track_type: MediaTrackType::Chapters,
      codec: String::new(),
      codec_id: String::new(),
      description: String::new(),
      chapter_entry_count: Some(meta.chapters.num_entries),
      size: None,
      bit_rate: None,
      track_name: String::new(),
      language: String::new(),
      language_codes: Vec::new(),
      default_track: TrackFlag::Unspecified,
      forced: TrackFlag::Unspecified,
    });
  }
  for attachment in &meta.attachments {
    let subtype = attachment_subtype(&attachment.file_name, attachment.mime_type.as_deref());
    rows.push(MediaTrack {
      kind: MediaTrackKind::Attachment,
      id: i64::from(attachment.id),
      number: 0,
      track_type: MediaTrackType::Attachment,
      codec: subtype.clone(),
      codec_id: subtype,
      description: String::new(),
      chapter_entry_count: None,
      // Sizes are f64 like every other row's; precision loss above 2^53 bytes
      // is accepted.
      size: Some(attachment.size as f64),
      bit_rate: None,
      track_name: attachment.file_name.clone(),
      language: String::new(),
      language_codes: Vec::new(),
      default_track: TrackFlag::Unspecified,
      forced: TrackFlag::Unspecified,
    });
  }
  rows
}

/// ISO 639-1 when the parser resolved one, else the bibliographic, else the
/// terminologic ISO 639-2 code, else `und`.
fn pick_track_language(track: &Track) -> String {
  let Some(language) = &track.properties.common.language else {
    return "und".to_string();
  };
  [
    language.iso639_1.as_deref(),
    language.iso639_2_bib.as_deref(),
    Some(language.iso639_2.as_str()),
  ]
  .into_iter()
  .flatten()
  .find(|code| !code.is_empty())
  .unwrap_or("und")
  .to_string()
}

/// The terminologic, bibliographic and alpha-2 codes, lowercased and deduped
/// in that order; `["und"]` without a language.
fn track_language_codes(track: &Track) -> Vec<String> {
  let Some(language) = &track.properties.common.language else {
    return vec!["und".to_string()];
  };
  let mut codes: Vec<String> = Vec::new();
  for code in [
    Some(language.iso639_2.as_str()),
    language.iso639_2_bib.as_deref(),
    language.iso639_1.as_deref(),
  ]
  .into_iter()
  .flatten()
  .filter(|code| !code.is_empty())
  {
    let code = code.to_lowercase();
    if !codes.contains(&code) {
      codes.push(code);
    }
  }
  if codes.is_empty() {
    codes.push("und".to_string());
  }
  codes
}

/// A numeric Matroska statistics tag (`BPS`, `NUMBER_OF_BYTES`), matched
/// case-insensitively; `None` when absent or not a finite number.
fn stat_tag_number(track: &Track, name: &str) -> Option<f64> {
  let upper = name.to_uppercase();
  let tag = track
    .properties
    .tags
    .iter()
    .find(|tag| tag.name.to_uppercase() == upper)?;
  tag.value.trim().parse::<f64>().ok().filter(|value| value.is_finite())
}

/// The attachment's file extension, else its mime subtype, lowercased.
fn attachment_subtype(file_name: &str, mime_type: Option<&str>) -> String {
  if let Some(dot) = file_name.rfind('.')
    && dot + 1 < file_name.len()
  {
    return file_name[dot + 1..].to_lowercase();
  }
  if let Some(mime_type) = mime_type
    && let Some(slash) = mime_type.find('/')
    && slash + 1 < mime_type.len()
  {
    return mime_type[slash + 1..].to_lowercase();
  }
  String::new()
}

fn channel_layout_label(kind: ChannelLayoutKind) -> Option<&'static str> {
  match kind {
    ChannelLayoutKind::Mono => Some("mono"),
    ChannelLayoutKind::Stereo => Some("stereo"),
    ChannelLayoutKind::Layout21 => Some("2.1"),
    ChannelLayoutKind::Layout30 => Some("3.0"),
    ChannelLayoutKind::Layout31 => Some("3.1"),
    ChannelLayoutKind::Layout40 => Some("4.0"),
    ChannelLayoutKind::Layout41 => Some("4.1"),
    ChannelLayoutKind::Layout50 => Some("5.0"),
    ChannelLayoutKind::Layout51 => Some("5.1"),
    ChannelLayoutKind::Layout61 => Some("6.1"),
    ChannelLayoutKind::Layout71 => Some("7.1"),
    ChannelLayoutKind::Layout714 => Some("7.1.4"),
    ChannelLayoutKind::Other => None,
  }
}

/// Round to three decimals (halves away from zero).
fn round3(value: f64) -> f64 {
  (value * 1000.0).round() / 1000.0
}

/// Frame rate from a per-frame duration, e.g. 41 708 333 ns → `23.976fps`.
fn format_frame_rate(default_duration_ns: Option<u64>) -> Option<String> {
  let duration = default_duration_ns.filter(|duration| *duration > 0)?;
  Some(format!("{}fps", round3(1e9 / duration as f64)))
}

/// Sample rate in kHz, e.g. 48 000 → `48kHz`, 44 100 → `44.1kHz`.
fn format_sample_rate(hz: Option<f64>) -> Option<String> {
  let hz = hz.filter(|hz| *hz > 0.0)?;
  Some(format!("{}kHz", round3(hz / 1000.0)))
}

fn video_description(video: &VideoTrackProperties) -> String {
  let mut parts = Vec::new();
  if let Some(dimensions) = video.pixel_dimensions.as_ref().or(video.display_dimensions.as_ref()) {
    parts.push(format!("{}x{}", dimensions.width, dimensions.height));
  }
  parts.extend(format_frame_rate(video.default_duration_ns));
  parts.join(" ")
}

fn channel_label(audio: &AudioTrackProperties) -> Option<String> {
  if let Some(label) = audio
    .channel_layout
    .as_ref()
    .and_then(|layout| layout.kind)
    .and_then(channel_layout_label)
  {
    return Some(label.to_string());
  }
  let channels = audio
    .channel_layout
    .as_ref()
    .map(|layout| layout.channels)
    .or(audio.channels)?;
  Some(match channels {
    1 => "mono".to_string(),
    2 => "stereo".to_string(),
    _ => format!("{channels}ch"),
  })
}

fn audio_description(audio: &AudioTrackProperties) -> String {
  let mut parts = Vec::new();
  parts.extend(channel_label(audio));
  parts.extend(format_sample_rate(audio.sampling_frequency));
  parts.join(" ")
}

fn subtitle_description(subtitle: &SubtitleTrackProperties) -> String {
  let mut parts = vec![if subtitle.text_subtitles { "Text" } else { "Image" }.to_string()];
  for part in [&subtitle.variant, &subtitle.encoding].into_iter().flatten() {
    if !part.is_empty() {
      parts.push(part.clone());
    }
  }
  if let Some(page) = subtitle.teletext_page {
    parts.push(format!("p{page}"));
  }
  parts.join(" ")
}

/// The Description column, from whichever domain sub-tree is populated.
fn track_description(track: &Track) -> String {
  let properties = &track.properties;
  if let Some(video) = &properties.video {
    video_description(video)
  } else if let Some(audio) = &properties.audio {
    audio_description(audio)
  } else if let Some(subtitle) = &properties.subtitle {
    subtitle_description(subtitle)
  } else {
    String::new()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::language::Language;
  use crate::media_metadata::model::attachment::Attachment;
  use crate::media_metadata::model::tag::TagEntry;
  use crate::media_metadata::model::track::{CodecInfo, TrackProperties};
  use crate::media_metadata::model::track_properties_audio::ChannelLayout;
  use crate::media_metadata::model::track_properties_video::Dimensions2D;

  fn track(id: i64, track_type: TrackType) -> Track {
    Track {
      id,
      track_type,
      codec: CodecInfo {
        id: "V_TEST".to_string(),
        name: None,
        codec_private: None,
      },
      properties: TrackProperties::default(),
    }
  }

  fn tag(name: &str, value: &str) -> TagEntry {
    TagEntry {
      name: name.to_string(),
      value: value.to_string(),
      language: None,
    }
  }

  fn attachment(id: u32, file_name: &str, mime_type: Option<&str>, size: u64) -> Attachment {
    Attachment {
      id,
      file_name: file_name.to_string(),
      mime_type: mime_type.map(str::to_string),
      description: None,
      size,
      uid_hex: None,
    }
  }

  fn one_row(track: Track) -> MediaTrack {
    let mut meta = MediaMetadata::new("a.mkv", 1);
    meta.tracks.push(track);
    metadata_to_media_tracks(&meta).remove(0)
  }

  #[test]
  fn maps_a_parsed_track_to_a_row() {
    let mut video = track(3, TrackType::Video);
    video.codec.name = Some("AVC/H.264".to_string());
    video.codec.id = "V_MPEG4/ISO/AVC".to_string();
    video.properties.common.number = Some(4);
    video.properties.common.track_name = Some("Main".to_string());
    video.properties.common.language = Some(Language::from_iso_639_2("fre"));
    video.properties.common.default = TrackFlag::True;
    video.properties.common.forced = TrackFlag::False;
    video.properties.tags = vec![tag("bps", "8000000"), tag("NUMBER_OF_BYTES", " 1048576 ")];
    video.properties.video = Some(VideoTrackProperties {
      pixel_dimensions: Some(Dimensions2D {
        width: 1920,
        height: 1080,
      }),
      default_duration_ns: Some(41_708_333),
      ..Default::default()
    });
    let row = one_row(video);
    assert_eq!(row.kind, MediaTrackKind::Track);
    assert_eq!((row.id, row.number), (3, 4));
    assert_eq!(row.track_type, MediaTrackType::Video);
    assert_eq!(row.key(), "video:3");
    assert_eq!(row.codec, "AVC/H.264");
    assert_eq!(row.codec_id, "V_MPEG4/ISO/AVC");
    assert_eq!(row.description, "1920x1080 23.976fps");
    assert_eq!(row.size, Some(1_048_576.0));
    assert_eq!(row.bit_rate, Some(8_000_000.0));
    assert_eq!(row.track_name, "Main");
    assert_eq!(row.language, "fr");
    assert_eq!(row.language_codes, ["fra", "fre", "fr"]);
    assert_eq!((row.default_track, row.forced), (TrackFlag::True, TrackFlag::False));
  }

  #[test]
  fn falls_back_for_missing_values() {
    let row = one_row(track(0, TrackType::Unknown));
    assert_eq!(row.codec, "V_TEST");
    assert_eq!(row.number, 0);
    assert_eq!(row.track_type, MediaTrackType::Unknown);
    assert_eq!(row.description, "");
    assert_eq!((row.size, row.bit_rate), (None, None));
    assert_eq!(row.track_name, "");
    assert_eq!(row.language, "und");
    assert_eq!(row.language_codes, ["und"]);
    assert_eq!(row.default_track, TrackFlag::Unspecified);
  }

  #[test]
  fn picks_the_shortest_language_form_available() {
    let mut german = track(0, TrackType::Audio);
    german.properties.common.language = Some(Language::from_iso_639_2("deu"));
    assert_eq!(one_row(german).language, "de");
    let mut no_alpha2 = track(0, TrackType::Audio);
    no_alpha2.properties.common.language = Some(Language::from_iso_639_2("haw"));
    let row = one_row(no_alpha2);
    assert_eq!(row.language, "haw");
    assert_eq!(row.language_codes, ["haw"]);
    let mut empty = track(0, TrackType::Audio);
    empty.properties.common.language = Some(Language {
      iso639_2: String::new(),
      iso639_2_bib: Some(String::new()),
      iso639_1: None,
      ietf: None,
      name: None,
    });
    let row = one_row(empty);
    assert_eq!(row.language, "und");
    assert_eq!(row.language_codes, ["und"]);
  }

  #[test]
  fn describes_audio_layouts_and_sample_rates() {
    let describe = |audio: AudioTrackProperties| {
      let mut track = track(0, TrackType::Audio);
      track.properties.audio = Some(audio);
      one_row(track).description
    };
    let layout = |channels, kind| {
      Some(ChannelLayout {
        channels,
        kind,
        raw_mask_hex: None,
      })
    };
    assert_eq!(
      describe(AudioTrackProperties {
        channel_layout: layout(6, Some(ChannelLayoutKind::Layout51)),
        sampling_frequency: Some(48_000.0),
        ..Default::default()
      }),
      "5.1 48kHz"
    );
    assert_eq!(
      describe(AudioTrackProperties {
        channel_layout: layout(12, Some(ChannelLayoutKind::Layout714)),
        sampling_frequency: Some(44_100.0),
        ..Default::default()
      }),
      "7.1.4 44.1kHz"
    );
    assert_eq!(
      describe(AudioTrackProperties {
        channel_layout: layout(3, Some(ChannelLayoutKind::Other)),
        ..Default::default()
      }),
      "3ch"
    );
    assert_eq!(
      describe(AudioTrackProperties {
        channels: Some(1),
        sampling_frequency: Some(22_050.0),
        ..Default::default()
      }),
      "mono 22.05kHz"
    );
    assert_eq!(
      describe(AudioTrackProperties {
        channels: Some(2),
        sampling_frequency: Some(0.0),
        ..Default::default()
      }),
      "stereo"
    );
    // The layout's own count wins over the stream's channel count.
    assert_eq!(
      describe(AudioTrackProperties {
        channel_layout: layout(2, None),
        channels: Some(6),
        ..Default::default()
      }),
      "stereo"
    );
    assert_eq!(describe(AudioTrackProperties::default()), "");
  }

  #[test]
  fn describes_video_and_subtitles() {
    let mut video = track(0, TrackType::Video);
    video.properties.video = Some(VideoTrackProperties {
      display_dimensions: Some(Dimensions2D {
        width: 720,
        height: 576,
      }),
      default_duration_ns: Some(0),
      ..Default::default()
    });
    assert_eq!(one_row(video).description, "720x576");
    let mut fps_only = track(0, TrackType::Video);
    fps_only.properties.video = Some(VideoTrackProperties {
      default_duration_ns: Some(40_000_000),
      ..Default::default()
    });
    assert_eq!(one_row(fps_only).description, "25fps");

    let describe = |subtitle: SubtitleTrackProperties| {
      let mut track = track(0, TrackType::Subtitles);
      track.properties.subtitle = Some(subtitle);
      one_row(track).description
    };
    assert_eq!(
      describe(SubtitleTrackProperties {
        text_subtitles: true,
        encoding: Some("UTF-8".to_string()),
        variant: Some("ASS".to_string()),
        teletext_page: None,
      }),
      "Text ASS UTF-8"
    );
    assert_eq!(
      describe(SubtitleTrackProperties {
        text_subtitles: false,
        encoding: Some(String::new()),
        variant: None,
        teletext_page: Some(888),
      }),
      "Image p888"
    );
  }

  #[test]
  fn appends_chapter_and_attachment_rows() {
    let mut meta = MediaMetadata::new("a.mkv", 1);
    meta.tracks.push(track(0, TrackType::Video));
    meta.chapters.num_entries = 12;
    meta
      .attachments
      .push(attachment(1, "Cover.JPG", Some("image/jpeg"), 2048));
    meta
      .attachments
      .push(attachment(2, "font", Some("application/x-truetype-font"), 10));
    meta.attachments.push(attachment(3, "trailing.", Some("text/"), 0));
    meta.attachments.push(attachment(4, "none", None, 0));
    let rows = metadata_to_media_tracks(&meta);
    assert_eq!(rows.len(), 6);
    let chapters = &rows[1];
    assert_eq!(chapters.kind, MediaTrackKind::Chapters);
    assert_eq!(chapters.key(), "chapters:0");
    assert_eq!(chapters.chapter_entry_count, Some(12));
    assert_eq!(chapters.codec, "");
    assert!(chapters.language_codes.is_empty());
    let cover = &rows[2];
    assert_eq!(cover.kind, MediaTrackKind::Attachment);
    assert_eq!(cover.key(), "attachment:1");
    assert_eq!((cover.codec.as_str(), cover.codec_id.as_str()), ("jpg", "jpg"));
    assert_eq!(cover.track_name, "Cover.JPG");
    assert_eq!(cover.size, Some(2048.0));
    assert_eq!(cover.language, "");
    assert_eq!(rows[3].codec, "x-truetype-font");
    assert_eq!(rows[4].codec, "");
    assert_eq!(rows[5].codec, "");
  }

  #[test]
  fn omits_the_chapters_row_without_entries() {
    let mut meta = MediaMetadata::new("a.mkv", 1);
    meta.chapters.num_editions = 1;
    assert!(metadata_to_media_tracks(&meta).is_empty());
  }

  #[test]
  fn counts_rows_per_type() {
    let mut meta = MediaMetadata::new("a.mkv", 1);
    for (id, track_type) in [
      (0, TrackType::Video),
      (1, TrackType::Audio),
      (2, TrackType::Audio),
      (3, TrackType::Subtitles),
      (4, TrackType::Buttons),
      (5, TrackType::Unknown),
    ] {
      meta.tracks.push(track(id, track_type));
    }
    meta.chapters.num_entries = 1;
    meta.attachments.push(attachment(1, "a.ttf", None, 1));
    let counts = media_track_counts(&metadata_to_media_tracks(&meta));
    assert_eq!(
      counts,
      MediaTrackCounts {
        video: 1,
        audio: 2,
        subtitles: 1,
        chapters: 1,
        attachments: 1,
      }
    );
  }

  #[test]
  fn ranks_types_in_table_order() {
    let ordered = [
      MediaTrackType::Video,
      MediaTrackType::Audio,
      MediaTrackType::Subtitles,
      MediaTrackType::Buttons,
      MediaTrackType::Chapters,
      MediaTrackType::Attachment,
      MediaTrackType::Unknown,
    ];
    for (rank, track_type) in ordered.iter().enumerate() {
      assert_eq!(usize::from(track_type.rank()), rank);
    }
    let media: Vec<_> = ordered.iter().filter(|t| t.is_media()).map(|t| t.as_str()).collect();
    assert_eq!(media, ["video", "audio", "subtitles"]);
  }

  #[test]
  fn ignores_statistics_tags_that_are_not_finite_numbers() {
    let mut bad = track(0, TrackType::Audio);
    bad.properties.tags = vec![tag("BPS", "Infinity"), tag("NUMBER_OF_BYTES", "n/a")];
    let row = one_row(bad);
    assert_eq!((row.bit_rate, row.size), (None, None));
  }
}
