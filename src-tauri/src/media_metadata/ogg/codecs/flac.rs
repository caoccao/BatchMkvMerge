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

//! FLAC-in-Ogg identification packet (FLAC v0.9 mapping):
//!
//! ```text
//! u8  0x7F
//! 4   "FLAC"
//! u8  major_version (1)
//! u8  minor_version (0)
//! u16 number_of_other_header_packets (BE)
//! 4   "fLaC"   (native FLAC stream marker)
//! ...metadata blocks (STREAMINFO must be first)
//! ```
//!
//! STREAMINFO layout (FLAC spec §4.2):
//!
//! ```text
//! u8  block_type | last_flag (block_type = 0)
//! u24 block_length (== 34, BE)
//! u16 min_block_size  (BE)
//! u16 max_block_size  (BE)
//! u24 min_frame_size  (BE)
//! u24 max_frame_size  (BE)
//! 8-byte packed: 20 bits sample_rate | 3 bits channels (-1) | 5 bits bps (-1) | 36 bits total samples
//! 16 bytes MD5
//! ```

use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};

use super::BitstreamMetadata;

const SIGNATURE: [u8; 5] = [0x7F, b'F', b'L', b'A', b'C'];

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  if packet.len() < 13 || packet[..5] != SIGNATURE {
    return None;
  }
  // packet[5..9] = major/minor + 16-bit packet count
  if &packet[9..13] != b"fLaC" {
    return None;
  }
  let mut metadata = BitstreamMetadata::audio_only("A_FLAC", "FLAC");
  // STREAMINFO follows at offset 13 if present (block header + body).
  if packet.len() >= 13 + 4 + 34 {
    let info_offset = 13 + 4;
    let info = &packet[info_offset..info_offset + 34];
    let min_block_size = u16::from_be_bytes([info[0], info[1]]) as u32;
    let max_block_size = u16::from_be_bytes([info[2], info[3]]) as u32;
    let min_frame_size = ((info[4] as u32) << 16) | ((info[5] as u32) << 8) | (info[6] as u32);
    let max_frame_size = ((info[7] as u32) << 16) | ((info[8] as u32) << 8) | (info[9] as u32);
    let packed = u64::from_be_bytes([
      info[10], info[11], info[12], info[13], info[14], info[15], info[16], info[17],
    ]);
    let sample_rate = (packed >> 44) & 0xF_FFFF;
    let channels = ((packed >> 41) & 0x07) as u32 + 1;
    let bps = ((packed >> 36) & 0x1F) as u32 + 1;
    let total_samples = packed & 0x0F_FFFF_FFFF;
    let md5: [u8; 16] = info[18..34].try_into().unwrap();

    let mut audio = AudioTrackProperties::default();
    audio.channels = Some(channels);
    audio.sampling_frequency = if sample_rate > 0 {
      Some(sample_rate as f64)
    } else {
      None
    };
    audio.bit_depth = Some(bps);
    audio.codec_config = Some(AudioCodecConfig {
      flac_min_block_size: Some(min_block_size),
      flac_max_block_size: Some(max_block_size),
      flac_min_frame_size: Some(min_frame_size),
      flac_max_frame_size: Some(max_frame_size),
      flac_total_samples: if total_samples == 0 { None } else { Some(total_samples) },
      flac_md5_hex: Some(hex_encode(&md5)),
      ..AudioCodecConfig::default()
    });
    metadata.audio = Some(audio);
  }
  Some(metadata)
}

fn hex_encode(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{:02x}", b));
  }
  s
}

#[cfg(test)]
pub(crate) fn build_identification_packet(
  sample_rate: u32,
  channels: u32,
  bit_depth: u32,
  total_samples: u64,
) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&SIGNATURE);
  p.push(1); // major
  p.push(0); // minor
  p.extend_from_slice(&1u16.to_be_bytes()); // packet count
  p.extend_from_slice(b"fLaC");
  // STREAMINFO block: 1B type | 3B length = 34
  p.push(0x00);
  p.push(0);
  p.push(0);
  p.push(34);
  // min/max block size
  p.extend_from_slice(&4096u16.to_be_bytes());
  p.extend_from_slice(&4096u16.to_be_bytes());
  // min/max frame size (3 bytes each)
  p.extend_from_slice(&[0u8; 3]);
  p.extend_from_slice(&[0u8; 3]);
  // 64-bit packed: 20 rate | 3 chan-1 | 5 bps-1 | 36 total samples
  let packed = ((sample_rate as u64) << 44)
    | (((channels - 1) as u64 & 0x7) << 41)
    | (((bit_depth - 1) as u64 & 0x1F) << 36)
    | (total_samples & 0x0F_FFFF_FFFF);
  p.extend_from_slice(&packed.to_be_bytes());
  p.extend_from_slice(&[0u8; 16]); // MD5
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sniffs_flac_audio_streaminfo() {
    let pkt = build_identification_packet(48000, 2, 24, 1_000_000);
    let m = sniff(&pkt).unwrap();
    let a = m.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.bit_depth, Some(24));
    let cfg = a.codec_config.unwrap();
    assert_eq!(cfg.flac_total_samples, Some(1_000_000));
  }

  #[test]
  fn rejects_native_flac_without_ogg_wrapper() {
    let mut pkt = b"fLaC".to_vec();
    pkt.extend_from_slice(&[0u8; 100]);
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn rejects_other_signatures() {
    assert!(sniff(b"OpusHead").is_none());
  }

  #[test]
  fn rejects_when_native_marker_missing() {
    let mut pkt = SIGNATURE.to_vec();
    pkt.extend_from_slice(&[0u8; 4]); // version + packet count
    pkt.extend_from_slice(b"XXXX"); // wrong native marker
    pkt.extend_from_slice(&[0u8; 4 + 34]);
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn handles_zero_total_samples() {
    let pkt = build_identification_packet(48000, 2, 24, 0);
    let m = sniff(&pkt).unwrap();
    let cfg = m.audio.unwrap().codec_config.unwrap();
    assert!(cfg.flac_total_samples.is_none());
  }

  #[test]
  fn md5_round_trips_as_hex() {
    let pkt = build_identification_packet(48000, 2, 24, 1);
    let m = sniff(&pkt).unwrap();
    let cfg = m.audio.unwrap().codec_config.unwrap();
    assert_eq!(cfg.flac_md5_hex.as_deref(), Some("00000000000000000000000000000000"));
  }

  #[test]
  fn rejects_short_packet() {
    assert!(sniff(&SIGNATURE).is_none());
  }
}
