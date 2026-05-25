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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::duration::DurationValue;
use super::playlist::PlaylistInfo;

/// Container-level metadata.  Mirrors `mkvmerge -J`'s container object as a
/// floor but adds typed fields the v20 schema omits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Container {
  /// Did at least one reader's `probe()` claim this file?
  pub recognized: bool,
  /// Is the recognised format fully supported (vs. probe-only)?
  pub supported: bool,
  /// Typed format identifier — one variant per reader in the registry.
  pub format: ContainerFormat,
  pub properties: ContainerProperties,
}

impl Default for Container {
  fn default() -> Self {
    Self {
      recognized: false,
      supported: false,
      format: ContainerFormat::Unknown,
      properties: ContainerProperties::default(),
    }
  }
}

/// One variant per reader we ship.  Listed in roughly the same order the
/// probe cascade tries them.  Add new variants here when adding a new
/// format reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ContainerFormat {
  Matroska,
  WebM,
  Mp4,
  QuickTime,
  Avi,
  Ogg,
  Webvtt,
  MpegTs,
  MpegPs,
  Flv,
  RealMedia,
  Ivf,
  CoreAudio,
  Wav,
  Flac,
  Mp3,
  Aac,
  Ac3,
  Eac3,
  Dts,
  TrueHd,
  Tta,
  Wavpack,
  Avc,
  Hevc,
  MpegVideo,
  Vc1,
  Dirac,
  Dv,
  Av1Obu,
  Srt,
  SsaAss,
  VobSub,
  Usf,
  MicroDvd,
  HdmvPgs,
  HdmvTextSt,
  VobButton,
  Chapters,
  Cdxa,
  HdSub,
  Asf,
  Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ContainerProperties {
  pub title: Option<String>,
  pub muxing_app: Option<String>,
  pub writing_app: Option<String>,
  pub duration: Option<DurationValue>,
  pub segment_uid_hex: Option<String>,
  pub previous_segment_uid_hex: Option<String>,
  pub next_segment_uid_hex: Option<String>,
  pub date_utc: Option<String>,
  /// Source-file mux date in local time zone (ISO-8601).  Mirrors mkvmerge's
  /// `date_local` field.
  pub date_local: Option<String>,
  pub is_fragmented: Option<bool>,
  /// Matroska TimestampScale (ns per timestamp unit, default 1_000_000 = ms).
  #[specta(type = Option<Number>)]
  pub timestamp_scale: Option<u64>,
  /// MP4 movie timescale (units per second, default 600).
  pub movie_timescale: Option<u32>,
  pub major_brand: Option<String>,
  pub compatible_brands: Vec<String>,
  /// Average bitrate when the container declares one (FLV, MP4 mdhd).
  #[specta(type = Option<Number>)]
  pub bitrate_bps: Option<u64>,
  pub programs: Vec<super::program::Program>,
  /// Sibling files demuxed alongside the primary input (VobSub `.sub`,
  /// multi-file AVI extents, ...).  File-system paths in the order they
  /// were discovered.  Mirrors mkvmerge's `other_file` field.
  pub other_files: Vec<String>,
  /// Blu-ray playlist metadata when the input is an `.mpls` playlist.
  pub playlist: Option<PlaylistInfo>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn default_container_is_unknown() {
    let c = Container::default();
    assert!(!c.recognized);
    assert!(!c.supported);
    assert_eq!(c.format, ContainerFormat::Unknown);
  }

  #[test]
  fn container_format_round_trip_for_every_variant() {
    for f in [
      ContainerFormat::Matroska,
      ContainerFormat::WebM,
      ContainerFormat::Mp4,
      ContainerFormat::QuickTime,
      ContainerFormat::Avi,
      ContainerFormat::Ogg,
      ContainerFormat::Webvtt,
      ContainerFormat::MpegTs,
      ContainerFormat::MpegPs,
      ContainerFormat::Flv,
      ContainerFormat::RealMedia,
      ContainerFormat::Ivf,
      ContainerFormat::CoreAudio,
      ContainerFormat::Wav,
      ContainerFormat::Flac,
      ContainerFormat::Mp3,
      ContainerFormat::Aac,
      ContainerFormat::Ac3,
      ContainerFormat::Eac3,
      ContainerFormat::Dts,
      ContainerFormat::TrueHd,
      ContainerFormat::Tta,
      ContainerFormat::Wavpack,
      ContainerFormat::Avc,
      ContainerFormat::Hevc,
      ContainerFormat::MpegVideo,
      ContainerFormat::Vc1,
      ContainerFormat::Dirac,
      ContainerFormat::Dv,
      ContainerFormat::Av1Obu,
      ContainerFormat::Srt,
      ContainerFormat::SsaAss,
      ContainerFormat::VobSub,
      ContainerFormat::Usf,
      ContainerFormat::MicroDvd,
      ContainerFormat::HdmvPgs,
      ContainerFormat::HdmvTextSt,
      ContainerFormat::VobButton,
      ContainerFormat::Chapters,
      ContainerFormat::Cdxa,
      ContainerFormat::HdSub,
      ContainerFormat::Asf,
      ContainerFormat::Unknown,
    ] {
      let back: ContainerFormat = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
      assert_eq!(back, f);
    }
  }

  #[test]
  fn container_format_emits_camel_case() {
    let s = serde_json::to_string(&ContainerFormat::QuickTime).unwrap();
    assert_eq!(s, "\"quickTime\"");
    let s = serde_json::to_string(&ContainerFormat::HdmvPgs).unwrap();
    assert_eq!(s, "\"hdmvPgs\"");
  }

  #[test]
  fn properties_round_trip() {
    let p = ContainerProperties {
      title: Some("Some Movie".to_owned()),
      muxing_app: Some("libmkv".to_owned()),
      writing_app: Some("mkvmerge v89".to_owned()),
      duration: Some(DurationValue::from_ns(60 * 1_000_000_000)),
      segment_uid_hex: Some("01020304".to_owned()),
      previous_segment_uid_hex: None,
      next_segment_uid_hex: None,
      date_utc: Some("2026-05-24T00:00:00Z".to_owned()),
      date_local: Some("2026-05-24T08:00:00+08:00".to_owned()),
      is_fragmented: Some(false),
      timestamp_scale: Some(1_000_000),
      movie_timescale: None,
      major_brand: None,
      compatible_brands: vec![],
      bitrate_bps: None,
      programs: vec![],
      other_files: vec!["sub.idx".to_owned(), "sub.sub".to_owned()],
      playlist: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"title\":\"Some Movie\""));
    assert!(s.contains("\"muxingApp\":\"libmkv\""));
    assert!(s.contains("\"timestampScale\":1000000"));
    assert!(s.contains("\"dateLocal\":\"2026-05-24T08:00:00+08:00\""));
    assert!(s.contains("\"otherFiles\":[\"sub.idx\",\"sub.sub\"]"));
    let back: ContainerProperties = serde_json::from_str(&s).unwrap();
    assert_eq!(back, p);
  }

  #[test]
  fn container_with_typed_format_round_trips() {
    let c = Container {
      recognized: true,
      supported: true,
      format: ContainerFormat::Matroska,
      properties: ContainerProperties::default(),
    };
    let s = serde_json::to_string(&c).unwrap();
    assert!(s.contains("\"format\":\"matroska\""));
    let back: Container = serde_json::from_str(&s).unwrap();
    assert_eq!(back, c);
  }
}
