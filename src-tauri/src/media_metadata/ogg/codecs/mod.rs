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

use crate::media_metadata::model::track::TrackType;
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::VideoTrackProperties;

/// Typed metadata extracted from the first packet of a bitstream.  We avoid
/// `Default` here because `TrackType` doesn't implement it — each sniffer
/// initialises the struct explicitly.
#[derive(Debug, Clone)]
pub struct BitstreamMetadata {
  pub codec_id: &'static str,
  pub codec_name: &'static str,
  pub track_type: TrackType,
  pub video: Option<VideoTrackProperties>,
  pub audio: Option<AudioTrackProperties>,
  pub language: Option<String>,
  pub frame_duration_ns: Option<u64>,
}

impl BitstreamMetadata {
  pub fn audio_only(codec_id: &'static str, codec_name: &'static str) -> Self {
    Self {
      codec_id,
      codec_name,
      track_type: TrackType::Audio,
      video: None,
      audio: Some(AudioTrackProperties::default()),
      language: None,
      frame_duration_ns: None,
    }
  }

  pub fn video_only(codec_id: &'static str, codec_name: &'static str) -> Self {
    Self {
      codec_id,
      codec_name,
      track_type: TrackType::Video,
      video: Some(VideoTrackProperties::default()),
      audio: None,
      language: None,
      frame_duration_ns: None,
    }
  }

  pub fn subtitle(codec_id: &'static str, codec_name: &'static str) -> Self {
    Self {
      codec_id,
      codec_name,
      track_type: TrackType::Subtitles,
      video: None,
      audio: None,
      language: None,
      frame_duration_ns: None,
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
