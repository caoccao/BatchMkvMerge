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

//! Per-codec first-packet sniffers.  Each sub-module recognises one Ogg
//! payload signature and decodes the identification header into a typed
//! `BitstreamMetadata`.

pub mod flac;
pub mod kate;
pub mod ogm;
pub mod opus;
pub mod speex;
pub mod theora;
pub mod vorbis;
pub mod vp8;

use crate::media_metadata::model::track::TrackType;
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;

/// Typed metadata extracted from the first packet of a bitstream.  We avoid
/// `Default` here because `TrackType` doesn't implement it — each sniffer
/// initialises the struct explicitly.
///
/// `codec_id` / `codec_name` are owned because OGM MS-compatible video maps
/// the stream-header FOURCC to a per-file codec id (PARSER-164), which is not
/// a compile-time constant.
#[derive(Debug, Clone)]
pub struct BitstreamMetadata {
  pub codec_id: String,
  pub codec_name: String,
  pub track_type: TrackType,
  pub video: Option<VideoTrackProperties>,
  pub audio: Option<AudioTrackProperties>,
  pub language: Option<String>,
  pub frame_duration_ns: Option<u64>,
  /// True for OGM "new stream header" MS-compatible video demuxers (both the
  /// VfW and the AVC path).  mkvtoolnix promotes such a stream's `TITLE`
  /// comment to the container title rather than the track name
  /// (`r_ogm.cpp:677-681, 692-693, 804-806`).  PARSER-165.
  pub ms_compat: bool,
  /// Explicit number of header packets the codec requires before
  /// `headers_read` is satisfied, overriding the codec-id table in
  /// `identify::header_packet_target`.  FLAC sets this from the Ogg-FLAC
  /// mapping header's `number_of_other_header_packets` field (PARSER-204);
  /// `None` falls back to the codec-id table.
  pub header_packet_count: Option<usize>,
  /// FLAC-in-Ogg wrapper mode (PARSER-203 / PARSER-204).  `Some(true)` for the
  /// post-1.1.1 `[0x7f]FLAC` mapping (first packet carries a 9-byte wrapper to
  /// strip), `Some(false)` for the pre-1.1.1 bare-`fLaC` mapping (the first
  /// header packet is skipped entirely when assembling codec private), `None`
  /// for non-FLAC codecs.  Mirrors `ogm_a_flac_demuxer_c`'s `ofm_post_1_1_1` /
  /// `ofm_pre_1_1_1` (`r_ogm_flac.cpp:264-290`).
  pub flac_post_1_1_1: Option<bool>,
}

impl BitstreamMetadata {
  pub fn audio_only(codec_id: impl Into<String>, codec_name: impl Into<String>) -> Self {
    Self {
      codec_id: codec_id.into(),
      codec_name: codec_name.into(),
      track_type: TrackType::Audio,
      video: None,
      audio: Some(AudioTrackProperties::default()),
      language: None,
      frame_duration_ns: None,
      ms_compat: false,
      header_packet_count: None,
      flac_post_1_1_1: None,
    }
  }

  pub fn video_only(codec_id: impl Into<String>, codec_name: impl Into<String>) -> Self {
    Self {
      codec_id: codec_id.into(),
      codec_name: codec_name.into(),
      track_type: TrackType::Video,
      video: Some(VideoTrackProperties::default()),
      audio: None,
      language: None,
      frame_duration_ns: None,
      ms_compat: false,
      header_packet_count: None,
      flac_post_1_1_1: None,
    }
  }

  pub fn subtitle(codec_id: impl Into<String>, codec_name: impl Into<String>) -> Self {
    Self {
      codec_id: codec_id.into(),
      codec_name: codec_name.into(),
      track_type: TrackType::Subtitles,
      video: None,
      audio: None,
      language: None,
      frame_duration_ns: None,
      ms_compat: false,
      header_packet_count: None,
      flac_post_1_1_1: None,
    }
  }
}

/// Try every sniffer against the first-packet bytes.  Returns the first
/// successful match.  Order matters: signatures with longer prefixes go first
/// so they don't collide with shorter ones.
pub fn sniff_first_packet(packet: &[u8]) -> Option<BitstreamMetadata> {
  if let Some(m) = vorbis::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = opus::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = theora::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = vp8::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = flac::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = speex::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = kate::sniff(packet) {
    return Some(m);
  }
  if let Some(m) = ogm::sniff(packet) {
    return Some(m);
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn random_bytes_match_nothing() {
    let m = sniff_first_packet(&[0xFF; 16]);
    assert!(m.is_none());
  }

  #[test]
  fn empty_packet_matches_nothing() {
    assert!(sniff_first_packet(&[]).is_none());
  }
}
