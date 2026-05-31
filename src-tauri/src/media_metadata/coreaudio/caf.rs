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

//! CoreAudio File Format (CAF) chunk walker.  Apple TN2095 layout:
//!
//! ```text
//! 4   "caff"
//! u16 mFileVersion (BE)
//! u16 mFileFlags
//! repeat:
//!   4 chunk type
//!   i64 chunk_size (BE — `-1` means "to end of file" for the data chunk)
//!   [chunk_size bytes of body]
//! ```
//!
//! The `desc` chunk carries the AudioStreamBasicDescription:
//!
//! ```text
//! f64 sample_rate (BE)
//! 4   format_id (FourCC, e.g. "lpcm", "aac ", "alac")
//! u32 format_flags
//! u32 bytes_per_packet
//! u32 frames_per_packet
//! u32 channels_per_frame
//! u32 bits_per_channel
//! ```

use crate::media_metadata::error::ParseError;

pub const CAFF_MAGIC: [u8; 4] = *b"caff";

#[derive(Debug, Clone, Copy)]
pub struct AudioDescription {
  pub sample_rate: f64,
  pub format_id: [u8; 4],
  pub format_flags: u32,
  pub bytes_per_packet: u32,
  pub frames_per_packet: u32,
  pub channels: u32,
  pub bits_per_channel: u32,
}

#[derive(Debug, Default, Clone)]
pub struct CafMetadata {
  pub description: Option<AudioDescription>,
  pub data_size: Option<u64>,
}

pub fn parse(bytes: &[u8]) -> Result<CafMetadata, ParseError> {
  if bytes.len() < 8 || !bytes[..4].eq_ignore_ascii_case(&CAFF_MAGIC) {
    return Err(ParseError::Unrecognised);
  }
  let mut metadata = CafMetadata::default();
  let mut pos = 8usize;
  while pos + 12 <= bytes.len() {
    let chunk_type = &bytes[pos..pos + 4];
    let chunk_size = i64::from_be_bytes([
      bytes[pos + 4],
      bytes[pos + 5],
      bytes[pos + 6],
      bytes[pos + 7],
      bytes[pos + 8],
      bytes[pos + 9],
      bytes[pos + 10],
      bytes[pos + 11],
    ]);
    let body_start = pos + 12;
    let chunk_end = if chunk_size < 0 {
      bytes.len()
    } else {
      body_start.saturating_add(chunk_size as usize)
    };
    if chunk_end > bytes.len() && chunk_type != b"data" {
      break;
    }
    let safe_end = chunk_end.min(bytes.len());
    let body = &bytes[body_start..safe_end];
    match chunk_type {
      b"desc" if body.len() >= 32 => {
        let sample_rate = f64::from_bits(u64::from_be_bytes([
          body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
        ]));
        let format_id = [body[8], body[9], body[10], body[11]];
        let format_flags = u32::from_be_bytes([body[12], body[13], body[14], body[15]]);
        let bytes_per_packet = u32::from_be_bytes([body[16], body[17], body[18], body[19]]);
        let frames_per_packet = u32::from_be_bytes([body[20], body[21], body[22], body[23]]);
        let channels = u32::from_be_bytes([body[24], body[25], body[26], body[27]]);
        let bits_per_channel = u32::from_be_bytes([body[28], body[29], body[30], body[31]]);
        metadata.description = Some(AudioDescription {
          sample_rate,
          format_id,
          format_flags,
          bytes_per_packet,
          frames_per_packet,
          channels,
          bits_per_channel,
        });
      }
      b"data" => {
        let actual_size = if chunk_size < 0 {
          (bytes.len() - body_start) as u64
        } else {
          chunk_size as u64
        };
        // Skip the 4-byte edit_count prefix per CAF spec
        metadata.data_size = Some(actual_size.saturating_sub(4));
      }
      _ => {}
    }
    if chunk_size < 0 {
      break;
    }
    pos = chunk_end;
  }
  Ok(metadata)
}

/// Decode an AudioStreamBasicDescription (`desc` chunk body, ≥ 32 bytes).
pub fn decode_desc(body: &[u8]) -> Option<AudioDescription> {
  if body.len() < 32 {
    return None;
  }
  let sample_rate = f64::from_bits(u64::from_be_bytes([
    body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
  ]));
  Some(AudioDescription {
    sample_rate,
    format_id: [body[8], body[9], body[10], body[11]],
    format_flags: u32::from_be_bytes([body[12], body[13], body[14], body[15]]),
    bytes_per_packet: u32::from_be_bytes([body[16], body[17], body[18], body[19]]),
    frames_per_packet: u32::from_be_bytes([body[20], body[21], body[22], body[23]]),
    channels: u32::from_be_bytes([body[24], body[25], body[26], body[27]]),
    bits_per_channel: u32::from_be_bytes([body[28], body[29], body[30], body[31]]),
  })
}

/// `sizeof(mtx::alac::codec_config_t)`.
pub const ALAC_CONFIG_SIZE: usize = 24;

#[derive(Debug, Clone, Copy)]
pub struct AlacConfig {
  pub bit_depth: u8,
  pub num_channels: u8,
  pub sample_rate: u32,
}

/// Port of `handle_alac_magic_cookie`: unwrap an old-style (`frmaalac`) cookie
/// to the 24-byte codec_config, otherwise return the cookie as-is.
pub fn convert_alac_cookie(cookie: &[u8]) -> Option<Vec<u8>> {
  if cookie.len() < ALAC_CONFIG_SIZE {
    return None;
  }
  if cookie.len() >= 12 && &cookie[4..12] == b"frmaalac" {
    let min = 12 + 12 + ALAC_CONFIG_SIZE;
    if cookie.len() < min {
      return None;
    }
    Some(cookie[24..24 + ALAC_CONFIG_SIZE].to_vec())
  } else {
    Some(cookie.to_vec())
  }
}

/// Decode the ALAC `codec_config_t` fields used for identification.
pub fn parse_alac_config(cfg: &[u8]) -> Option<AlacConfig> {
  if cfg.len() < ALAC_CONFIG_SIZE {
    return None;
  }
  Some(AlacConfig {
    bit_depth: cfg[5],
    num_channels: cfg[11],
    sample_rate: u32::from_be_bytes([cfg[20], cfg[21], cfg[22], cfg[23]]),
  })
}

pub fn fourcc_string(bytes: &[u8; 4]) -> String {
  bytes
    .iter()
    .map(|b| if (0x20..=0x7E).contains(b) { *b as char } else { '?' })
    .collect()
}

#[cfg(test)]
pub(crate) fn build_caf(format_id: &[u8; 4], sample_rate: f64, channels: u32, bits: u32) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"caff");
  bytes.extend_from_slice(&1u16.to_be_bytes());
  bytes.extend_from_slice(&0u16.to_be_bytes());
  // desc chunk
  bytes.extend_from_slice(b"desc");
  bytes.extend_from_slice(&32i64.to_be_bytes());
  bytes.extend_from_slice(&sample_rate.to_bits().to_be_bytes());
  bytes.extend_from_slice(format_id);
  bytes.extend_from_slice(&0u32.to_be_bytes()); // flags
  bytes.extend_from_slice(&0u32.to_be_bytes()); // bytes_per_packet
  bytes.extend_from_slice(&1024u32.to_be_bytes()); // frames_per_packet
  bytes.extend_from_slice(&channels.to_be_bytes());
  bytes.extend_from_slice(&bits.to_be_bytes());
  // pakt chunk
  bytes.extend_from_slice(b"pakt");
  bytes.extend_from_slice(&24i64.to_be_bytes());
  bytes.extend_from_slice(&0u64.to_be_bytes()); // num_packets
  bytes.extend_from_slice(&0u64.to_be_bytes()); // num_valid_frames
  bytes.extend_from_slice(&0u32.to_be_bytes()); // priming frames
  bytes.extend_from_slice(&0u32.to_be_bytes()); // remainder frames
  // data chunk
  bytes.extend_from_slice(b"data");
  bytes.extend_from_slice(&100i64.to_be_bytes());
  bytes.extend_from_slice(&[0u8; 100]);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_caff_with_lpcm_desc() {
    let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
    let m = parse(&bytes).unwrap();
    let d = m.description.unwrap();
    assert_eq!(&d.format_id, b"lpcm");
    assert_eq!(d.sample_rate, 48_000.0);
    assert_eq!(d.channels, 2);
    assert_eq!(d.bits_per_channel, 24);
  }

  #[test]
  fn parses_data_chunk_size() {
    let bytes = build_caf(b"alac", 44_100.0, 2, 16);
    let m = parse(&bytes).unwrap();
    // 100 declared - 4 edit_count = 96
    assert_eq!(m.data_size, Some(96));
  }

  #[test]
  fn rejects_non_caff_magic() {
    let mut bytes = build_caf(b"lpcm", 48_000.0, 2, 16);
    bytes[0] = b'X';
    assert!(matches!(parse(&bytes), Err(ParseError::Unrecognised)));
  }

  #[test]
  fn accepts_ascii_case_variants_of_caff_magic() {
    let mut bytes = build_caf(b"lpcm", 48_000.0, 2, 16);
    bytes[0..4].copy_from_slice(b"CAFF");
    let m = parse(&bytes).unwrap();
    assert!(m.description.is_some());
  }

  #[test]
  fn handles_size_minus_one_data_chunk() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(-1i64).to_be_bytes());
    bytes.extend_from_slice(&[0u8; 200]);
    let m = parse(&bytes).unwrap();
    // 200 bytes total payload - 4 edit_count = 196
    assert_eq!(m.data_size, Some(196));
  }

  #[test]
  fn fourcc_string_renders_aac_format() {
    assert_eq!(fourcc_string(b"aac "), "aac ");
    assert_eq!(fourcc_string(&[b'a', 0xFF, b'c', b' ']), "a?c ");
  }
}
