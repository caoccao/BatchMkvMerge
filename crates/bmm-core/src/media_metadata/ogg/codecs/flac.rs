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

//! FLAC-in-Ogg identification packet.  Two mappings are recognised, mirroring
//! `r_ogm.cpp:457-472`:
//!
//! * **post-1.1.1** (`ofm_post_1_1_1`) — the standard Ogg-FLAC mapping:
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
//! * **pre-1.1.1** (`ofm_pre_1_1_1`) — older libFLAC that wrote the bare native
//!   FLAC stream into Ogg, so the first packet starts directly with `fLaC`
//!   followed by the STREAMINFO metadata block.  PARSER-203.
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

/// Post-1.1.1 Ogg-FLAC mapping marker: `0x7f` + "FLAC".
const SIGNATURE: [u8; 5] = [0x7F, b'F', b'L', b'A', b'C'];
/// Native FLAC stream marker.
const NATIVE_MARKER: &[u8; 4] = b"fLaC";

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  // r_ogm.cpp:457-459 accepts either the bare native marker (pre-1.1.1) or the
  // 0x7f-FLAC wrapper (post-1.1.1).
  let (post_1_1_1, info_offset, other_header_packets) =
    if packet.len() >= 13 && packet[..5] == SIGNATURE && &packet[9..13] == NATIVE_MARKER {
      // packet[5..7] = major/minor, packet[7..9] = number_of_other_header_packets.
      let other = u16::from_be_bytes([packet[7], packet[8]]) as usize;
      // STREAMINFO follows the native marker (block header + body) at offset 13.
      (true, 13 + 4, Some(other))
    } else if packet.len() >= 4 && &packet[..4] == NATIVE_MARKER {
      // PARSER-203: pre-1.1.1 bare-`fLaC` mapping; STREAMINFO follows the marker.
      (false, 4 + 4, None)
    } else {
      return None;
    };

  let mut metadata = BitstreamMetadata::audio_only("A_FLAC", "FLAC");
  metadata.flac_post_1_1_1 = Some(post_1_1_1);
  // PARSER-204: total header packet count = STREAMINFO packet + the mapping's
  // advertised "other" header packets.  Only the post-1.1.1 mapping carries the
  // count in the BOS; the pre-1.1.1 path leaves it `None` so the reader
  // discovers it by following the metadata-block "last" flag.
  metadata.header_packet_count = other_header_packets.map(|other| other + 1);
  // STREAMINFO follows at `info_offset` if present (block header + body).
  if packet.len() >= info_offset + 34 {
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

/// PARSER-204: return `true` when the FLAC metadata block carried by `packet`
/// has its "last-metadata-block" flag set (bit 7 of the block header byte).
///
/// The FLAC metadata block header (FLAC spec §4) starts with a byte whose top
/// bit is the last-block flag and whose low 7 bits are the block type.  In the
/// Ogg-FLAC mapping the first packet prefixes that block header with either the
/// 9-byte post-1.1.1 wrapper + native `fLaC` marker (offset 13) or just the
/// native `fLaC` marker (offset 4); every subsequent header packet carries a
/// raw metadata block whose header is at offset 0.  Returns `None` when the
/// block header byte is out of range.
pub fn is_last_metadata_block(packet: &[u8], is_first_packet: bool, post_1_1_1: bool) -> Option<bool> {
  let offset = if !is_first_packet {
    0
  } else if post_1_1_1 {
    13
  } else {
    4
  };
  packet.get(offset).map(|b| b & 0x80 != 0)
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
  build_identification_packet_ex(sample_rate, channels, bit_depth, total_samples, true, 0, true)
}

/// Build a FLAC-in-Ogg identification packet for tests.
///
/// * `post_1_1_1` — emit the `0x7f`-FLAC wrapper (`true`) or the bare-`fLaC`
///   pre-1.1.1 mapping (`false`).
/// * `other_header_packets` — value written into the post-1.1.1 mapping's
///   `number_of_other_header_packets` field (ignored when `post_1_1_1` is
///   `false`).
/// * `last_block` — set the STREAMINFO block's last-metadata-block flag.
#[cfg(test)]
pub(crate) fn build_identification_packet_ex(
  sample_rate: u32,
  channels: u32,
  bit_depth: u32,
  total_samples: u64,
  post_1_1_1: bool,
  other_header_packets: u16,
  last_block: bool,
) -> Vec<u8> {
  let mut p = Vec::new();
  if post_1_1_1 {
    p.extend_from_slice(&SIGNATURE);
    p.push(1); // major
    p.push(0); // minor
    p.extend_from_slice(&other_header_packets.to_be_bytes());
  }
  p.extend_from_slice(NATIVE_MARKER);
  // STREAMINFO block: 1B type (0 = STREAMINFO, bit7 = last) | 3B length = 34.
  p.push(if last_block { 0x80 } else { 0x00 });
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

/// Build a standalone FLAC metadata block packet (one per Ogg header packet
/// after STREAMINFO) for tests.  `block_type` is the low-7-bit type; `last`
/// sets the last-metadata-block flag.
#[cfg(test)]
pub(crate) fn build_metadata_block_packet(block_type: u8, last: bool, body: &[u8]) -> Vec<u8> {
  let mut p = Vec::new();
  let header = (if last { 0x80 } else { 0x00 }) | (block_type & 0x7f);
  p.push(header);
  let len = body.len() as u32;
  p.push((len >> 16) as u8);
  p.push((len >> 8) as u8);
  p.push(len as u8);
  p.extend_from_slice(body);
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
  fn post_1_1_1_reports_header_packet_count_from_mapping() {
    // PARSER-204: total header packets = STREAMINFO + advertised "other" count.
    let pkt = build_identification_packet_ex(48000, 2, 24, 1, true, 3, false);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.flac_post_1_1_1, Some(true));
    assert_eq!(m.header_packet_count, Some(4));
  }

  #[test]
  fn accepts_pre_1_1_1_bare_flac_marker() {
    // PARSER-203: a packet that starts directly with `fLaC` (pre-1.1.1) is
    // recognised and STREAMINFO is parsed from offset 4.
    let pkt = build_identification_packet_ex(44100, 2, 16, 500, false, 0, true);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "A_FLAC");
    assert_eq!(m.flac_post_1_1_1, Some(false));
    // Pre-1.1.1 carries no explicit count in the BOS.
    assert!(m.header_packet_count.is_none());
    let a = m.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44100.0));
    assert_eq!(a.bit_depth, Some(16));
  }

  #[test]
  fn is_last_metadata_block_reads_correct_offset() {
    // PARSER-204: first packet, post-1.1.1 → block header at offset 13.
    let last = build_identification_packet_ex(48000, 2, 24, 1, true, 0, true);
    assert_eq!(is_last_metadata_block(&last, true, true), Some(true));
    let not_last = build_identification_packet_ex(48000, 2, 24, 1, true, 1, false);
    assert_eq!(is_last_metadata_block(&not_last, true, true), Some(false));
    // First packet, pre-1.1.1 → offset 4.
    let pre = build_identification_packet_ex(48000, 2, 24, 1, false, 0, true);
    assert_eq!(is_last_metadata_block(&pre, true, false), Some(true));
    // Subsequent packets → offset 0.
    let block = build_metadata_block_packet(4, true, b"x");
    assert_eq!(is_last_metadata_block(&block, false, true), Some(true));
    let non_last_block = build_metadata_block_packet(4, false, b"x");
    assert_eq!(is_last_metadata_block(&non_last_block, false, true), Some(false));
    // Out-of-range offset yields None.
    assert_eq!(is_last_metadata_block(&[0x7f], true, true), None);
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
