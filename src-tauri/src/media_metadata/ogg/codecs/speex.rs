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

//! Speex identification header.  Layout (Speex spec):
//!
//! ```text
//! 8   "Speex   "                (8 bytes, trailing spaces)
//! 20  speex_version_string      (20 bytes, NUL/space padded)
//! u32 speex_version_id          (LE)
//! u32 header_size               (LE — should be 80)
//! u32 rate                      (LE)
//! u32 mode                      (LE)
//! u32 mode_bitstream_version    (LE)
//! u32 nb_channels               (LE)
//! ...
//! ```

use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;

use super::BitstreamMetadata;

const SIGNATURE: &[u8; 8] = b"Speex   ";

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  if packet.len() < 44 || &packet[..8] != SIGNATURE {
    return None;
  }
  // Rate at offset 36 (after signature + 20 version + 2x u32 = 8+20+4+4 = 36).
  let rate = u32::from_le_bytes([packet[36], packet[37], packet[38], packet[39]]);
  // nb_channels at offset 48 if header is long enough.
  let channels = if packet.len() >= 52 {
    u32::from_le_bytes([packet[48], packet[49], packet[50], packet[51]])
  } else {
    1
  };
  let mut metadata = BitstreamMetadata::audio_only("A_SPEEX", "Speex");
  metadata.audio = Some(AudioTrackProperties {
    channels: Some(channels.max(1)),
    sampling_frequency: if rate > 0 { Some(rate as f64) } else { None },
    ..AudioTrackProperties::default()
  });
  Some(metadata)
}

#[cfg(test)]
pub(crate) fn build_identification_packet(rate: u32, channels: u32) -> Vec<u8> {
  let mut p = Vec::with_capacity(80);
  p.extend_from_slice(SIGNATURE);
  p.extend_from_slice(&[0u8; 20]); // version string
  p.extend_from_slice(&1u32.to_le_bytes()); // version id
  p.extend_from_slice(&80u32.to_le_bytes()); // header size
  p.extend_from_slice(&rate.to_le_bytes());
  p.extend_from_slice(&0u32.to_le_bytes()); // mode
  p.extend_from_slice(&0u32.to_le_bytes()); // mode_bitstream_version
  p.extend_from_slice(&channels.to_le_bytes());
  // Pad to 80 bytes
  while p.len() < 80 {
    p.push(0);
  }
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sniffs_speex_narrowband() {
    let pkt = build_identification_packet(16000, 1);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "A_SPEEX");
    let a = m.audio.unwrap();
    assert_eq!(a.channels, Some(1));
    assert_eq!(a.sampling_frequency, Some(16000.0));
  }

  #[test]
  fn rejects_non_speex() {
    assert!(sniff(b"OpusHead").is_none());
    assert!(sniff(b"\x01vorbis").is_none());
  }

  #[test]
  fn signature_includes_trailing_spaces() {
    assert_eq!(SIGNATURE, b"Speex   ");
  }

  #[test]
  fn rejects_short_packet() {
    assert!(sniff(SIGNATURE).is_none());
  }
}
