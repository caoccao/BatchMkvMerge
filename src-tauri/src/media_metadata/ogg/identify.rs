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

//! Convert the per-bitstream collector into protocol-level tracks.

use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::tag::TagEntry;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;

use super::codecs::BitstreamMetadata;

#[derive(Debug, Clone)]
pub struct BitstreamState {
  pub serial: u32,
  pub first_packet: Vec<u8>,
  pub header_packets: Vec<Vec<u8>>,
  pub metadata: Option<BitstreamMetadata>,
  pub vorbis_tags: Vec<TagEntry>,
  pub comment_language: Option<String>,
  pub vendor: Option<String>,
}

pub fn finalise(states: Vec<BitstreamState>, out: &mut MediaMetadata) {
  out.container.format = crate::media_metadata::model::container::ContainerFormat::Ogg;
  out.container.recognized = true;
  out.container.supported = true;

  for (idx, state) in states.into_iter().enumerate() {
    let Some(metadata) = state.metadata else {
      continue;
    };
    let track = make_track(
      idx as i64,
      state.serial,
      state.vendor,
      state.vorbis_tags,
      state.comment_language,
      state.header_packets,
      metadata,
    );
    out.tracks.push(track);
  }

  out.tags.per_track_count = out.tracks.iter().map(|t| t.properties.tags.len() as u32).sum();

  // Collect any global VorbisComment vendor lines as informational tags.
  // (Mkvtoolnix groups muxing/writing app from the first vendor seen — we
  // mirror that by populating `muxing_app` when not already set.)
  if out.container.properties.muxing_app.is_none() {
    if let Some(first) = out.tracks.iter().find_map(|t| {
      t.properties
        .tags
        .iter()
        .find(|tag| tag.name.eq_ignore_ascii_case("VENDOR"))
        .map(|tag| tag.value.clone())
    }) {
      out.container.properties.muxing_app = Some(first);
    }
  }
}

fn make_track(
  id: i64,
  serial: u32,
  vendor: Option<String>,
  mut tags: Vec<TagEntry>,
  comment_language: Option<String>,
  header_packets: Vec<Vec<u8>>,
  metadata: BitstreamMetadata,
) -> Track {
  if let Some(vendor) = vendor {
    tags.push(TagEntry {
      name: "VENDOR".to_string(),
      value: vendor,
      language: None,
    });
  }

  let mut common = CommonTrackProperties::default();
  // PARSER-081: mkvtoolnix's `r_ogm.cpp:671-724` keys tracks on serialno;
  // we surface the same value so the number is a stable cross-process id.
  common.number = Some(serial as u64);
  common.stream_id = Some(serial);
  let language_hint = comment_language.or(metadata.language.clone());
  if let Some(lang) = language_hint {
    common.language = Some(Language::resolve(Some(&lang), None, false));
  }
  // PARSER-082: a TITLE Vorbis comment becomes the track name (mkvtoolnix
  // `r_ogm.cpp:793-808`).  Stream-level comments map to the track; if no
  // TITLE is present the track stays unnamed.
  if let Some(title) = tags
    .iter()
    .find(|t| t.name.eq_ignore_ascii_case("TITLE"))
    .map(|t| t.value.clone())
  {
    common.track_name = Some(title);
  }

  let mut properties = TrackProperties {
    common,
    video: metadata.video.clone(),
    audio: metadata.audio.clone(),
    subtitle: None,
    tags,
  };
  if metadata.track_type == TrackType::Subtitles {
    properties.subtitle = Some(SubtitleTrackProperties {
      text_subtitles: true,
      encoding: None,
      variant: Some(metadata.codec_name.to_string()),
      teletext_page: None,
    });
  }
  if let (Some(ns), Some(video)) = (metadata.frame_duration_ns, properties.video.as_mut()) {
    video.default_duration_ns.get_or_insert(ns);
  }

  Track {
    id,
    track_type: metadata.track_type,
    codec: CodecInfo {
      id: metadata.codec_id.to_string(),
      name: Some(metadata.codec_name.to_string()),
      codec_private: codec_private_for(metadata.codec_id, &header_packets),
    },
    properties,
  }
}

pub fn header_packet_target(codec_id: &str) -> usize {
  match codec_id {
    "A_VORBIS" | "V_THEORA" => 3,
    "A_OPUS" | "A_SPEEX" => 2,
    _ => 1,
  }
}

fn codec_private_for(codec_id: &str, packets: &[Vec<u8>]) -> Option<CodecPrivate> {
  let bytes = match codec_id {
    // Matroska stores Vorbis/Theora header packets as Xiph lacing:
    // packet-count-minus-one, lace sizes for all but the final packet,
    // then the packet payloads.
    "A_VORBIS" | "V_THEORA" if packets.len() >= 3 => xiph_laced_headers(&packets[..3])?,
    "A_OPUS" | "A_SPEEX" | "A_FLAC" | "V_OGM" | "A_OGM" | "S_OGM_TEXT" | "S_KATE" => packets.first()?.clone(),
    _ => return None,
  };
  Some(CodecPrivate::from_bytes(&bytes))
}

fn xiph_laced_headers(packets: &[Vec<u8>]) -> Option<Vec<u8>> {
  if packets.len() < 2 || packets.len() > u8::MAX as usize + 1 {
    return None;
  }
  let mut out = vec![(packets.len() - 1) as u8];
  for packet in &packets[..packets.len() - 1] {
    let mut remaining = packet.len();
    while remaining >= 255 {
      out.push(255);
      remaining -= 255;
    }
    out.push(remaining as u8);
  }
  for packet in packets {
    out.extend_from_slice(packet);
  }
  Some(out)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;

  fn state_with_vorbis() -> BitstreamState {
    let mut metadata = BitstreamMetadata::audio_only("A_VORBIS", "Vorbis");
    metadata.audio = Some(AudioTrackProperties {
      channels: Some(2),
      sampling_frequency: Some(44100.0),
      ..AudioTrackProperties::default()
    });
    BitstreamState {
      serial: 0xC0FE,
      first_packet: Vec::new(),
      header_packets: vec![b"\x01vorbis-ident".to_vec()],
      metadata: Some(metadata),
      vorbis_tags: vec![TagEntry {
        name: "TITLE".to_string(),
        value: "Track".to_string(),
        language: None,
      }],
      comment_language: Some("eng".to_string()),
      vendor: Some("libvorbis 1.3.7".to_string()),
    }
  }

  #[test]
  fn finalise_creates_audio_track() {
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state_with_vorbis()], &mut m);
    assert_eq!(m.tracks.len(), 1);
    let t = &m.tracks[0];
    assert_eq!(t.track_type, TrackType::Audio);
    assert_eq!(t.codec.id, "A_VORBIS");
    let common = &t.properties.common;
    assert_eq!(common.stream_id, Some(0xC0FE));
    assert_eq!(common.language.as_ref().unwrap().iso639_2, "eng");
    // VENDOR + TITLE tags
    assert_eq!(t.properties.tags.len(), 2);
  }

  #[test]
  fn finalise_populates_container_muxing_app_from_first_vendor() {
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state_with_vorbis()], &mut m);
    assert_eq!(m.container.properties.muxing_app.as_deref(), Some("libvorbis 1.3.7"));
  }

  #[test]
  fn state_without_metadata_is_skipped() {
    let state = BitstreamState {
      serial: 1,
      first_packet: Vec::new(),
      header_packets: Vec::new(),
      metadata: None,
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state], &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn subtitle_track_gets_subtitle_properties() {
    let metadata = BitstreamMetadata::subtitle("S_KATE", "Kate");
    let state = BitstreamState {
      serial: 2,
      first_packet: Vec::new(),
      header_packets: vec![b"\x80kate\0\0\0header".to_vec()],
      metadata: Some(metadata),
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state], &mut m);
    let t = &m.tracks[0];
    assert_eq!(t.track_type, TrackType::Subtitles);
    let sub = t.properties.subtitle.as_ref().unwrap();
    assert!(sub.text_subtitles);
    assert_eq!(sub.variant.as_deref(), Some("Kate"));
  }

  #[test]
  fn vorbis_header_packets_become_xiph_laced_private_data() {
    let mut metadata = BitstreamMetadata::audio_only("A_VORBIS", "Vorbis");
    metadata.audio = Some(AudioTrackProperties {
      channels: Some(2),
      sampling_frequency: Some(44100.0),
      ..AudioTrackProperties::default()
    });
    let state = BitstreamState {
      serial: 3,
      first_packet: Vec::new(),
      header_packets: vec![b"id".to_vec(), b"comments".to_vec(), b"setup".to_vec()],
      metadata: Some(metadata),
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state], &mut m);
    let private = m.tracks[0].codec.codec_private.as_ref().unwrap();
    assert_eq!(private.hex, "0202086964636f6d6d656e74737365747570");
  }
}
