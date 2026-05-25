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

  // PARSER-165: the first MS-compatible OGM video stream's TITLE comment is
  // the container/segment title, not a track name (r_ogm.cpp:677-681, 804-806).
  if out.container.properties.title.is_none() {
    if let Some(title) = states.iter().find_map(|s| {
      let md = s.metadata.as_ref()?;
      if !md.ms_compat {
        return None;
      }
      s.vorbis_tags
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case("TITLE"))
        .map(|t| t.value.clone())
    }) {
      out.container.properties.title = Some(title);
    }
  }

  // PARSER-166: assign track ids compactly — a BOS stream we could not
  // identify (metadata == None) must not consume an id (r_ogm.cpp:629-633,
  // 685-716 only bumps track_id for emitted tracks).
  let mut track_id = 0i64;
  for state in states.into_iter() {
    let Some(metadata) = state.metadata else {
      continue;
    };
    // PARSER-181: erase streams whose headers were never fully read, mirroring
    // mkvtoolnix's `erase_if(!headers_read)` at r_ogm.cpp:633.  A stream that
    // never collected its required header-packet set (e.g. a Vorbis BOS with no
    // comment/setup packets) is dropped here and — like the metadata == None
    // case above — must not consume a track id, keeping the compact id
    // assignment intact.
    if state.header_packets.len() < header_packet_target(&metadata.codec_id) {
      continue;
    }
    let track = make_track(
      track_id,
      state.serial,
      state.vendor,
      state.vorbis_tags,
      state.comment_language,
      state.header_packets,
      metadata,
    );
    out.tracks.push(track);
    track_id += 1;
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
  // PARSER-082 / PARSER-165: a TITLE Vorbis comment becomes the track name —
  // except for MS-compatible OGM video, where mkvtoolnix promotes it to the
  // container title instead and leaves the track unnamed (`r_ogm.cpp:692-693`).
  if !metadata.ms_compat {
    if let Some(title) = tags
      .iter()
      .find(|t| t.name.eq_ignore_ascii_case("TITLE"))
      .map(|t| t.value.clone())
    {
      common.track_name = Some(title);
    }
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
      variant: Some(metadata.codec_name.clone()),
      teletext_page: None,
    });
  }
  if let (Some(ns), Some(video)) = (metadata.frame_duration_ns, properties.video.as_mut()) {
    video.default_duration_ns.get_or_insert(ns);
  }

  let codec_private = codec_private_for(&metadata.codec_id, &header_packets);
  Track {
    id,
    track_type: metadata.track_type,
    codec: CodecInfo {
      id: metadata.codec_id,
      name: Some(metadata.codec_name),
      codec_private,
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
      // PARSER-181: A_VORBIS's header_packet_target is 3 (ident + comments +
      // setup); supply the full set so finalise's `erase_if(!headers_read)`
      // (r_ogm.cpp:633) keeps the track.
      header_packets: vec![
        b"\x01vorbis-ident".to_vec(),
        b"\x03vorbis-comments".to_vec(),
        b"\x05vorbis-setup".to_vec(),
      ],
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
  fn ms_compat_video_title_becomes_container_title_not_track_name() {
    // PARSER-165
    let mut metadata = BitstreamMetadata::video_only("XVID", "Xvid");
    metadata.ms_compat = true;
    let state = BitstreamState {
      serial: 7,
      first_packet: Vec::new(),
      header_packets: vec![b"hdr".to_vec()],
      metadata: Some(metadata),
      vorbis_tags: vec![TagEntry {
        name: "TITLE".to_string(),
        value: "My Movie".to_string(),
        language: None,
      }],
      comment_language: None,
      vendor: None,
    };
    let mut m = MediaMetadata::new("clip.ogm", 0);
    finalise(vec![state], &mut m);
    assert_eq!(m.container.properties.title.as_deref(), Some("My Movie"));
    assert!(m.tracks[0].properties.common.track_name.is_none());
  }

  #[test]
  fn non_ms_compat_title_stays_on_track_name() {
    // A Vorbis audio TITLE is a track name, not a container title.
    let state = state_with_vorbis();
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![state], &mut m);
    assert!(m.container.properties.title.is_none());
    assert_eq!(m.tracks[0].properties.common.track_name.as_deref(), Some("Track"));
  }

  #[test]
  fn unidentified_bos_stream_does_not_consume_a_track_id() {
    // PARSER-166: a None-metadata stream before a recognised one must not push
    // the recognised track's id off 0.
    let skipped = BitstreamState {
      serial: 1,
      first_packet: Vec::new(),
      header_packets: Vec::new(),
      metadata: None,
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![skipped, state_with_vorbis()], &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].id, 0);
  }

  #[test]
  fn stream_with_incomplete_headers_is_dropped() {
    // PARSER-181: a Vorbis stream (header_packet_target == 3) that only has its
    // BOS ident packet is erased by finalise, mirroring mkvtoolnix's
    // `erase_if(!headers_read)` at r_ogm.cpp:633.  It must also not consume a
    // track id, so a following complete stream keeps id 0.
    let incomplete = BitstreamState {
      serial: 9,
      first_packet: Vec::new(),
      header_packets: vec![b"\x01vorbis-ident".to_vec()], // only 1 of 3
      metadata: Some(BitstreamMetadata::audio_only("A_VORBIS", "Vorbis")),
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    let mut complete = state_with_vorbis();
    complete.header_packets = vec![b"ident".to_vec(), b"comments".to_vec(), b"setup".to_vec()];
    let mut m = MediaMetadata::new("clip.ogg", 0);
    finalise(vec![incomplete, complete], &mut m);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].id, 0);
    assert_eq!(m.tracks[0].codec.id, "A_VORBIS");
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
