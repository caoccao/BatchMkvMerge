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

//! AC-3 / E-AC-3 reader.
//!
//! Frame sync = `0x0B 0x77` (ATSC A/52 §4.4.1).  Port of
//! `mkvtoolnix/src/common/ac3.cpp` (`frame_c::decode_header`,
//! `decode_header_type_ac3`, `decode_header_type_eac3`,
//! `parser_c::find_consecutive_frames`).
//!
//! After the 16-bit sync word, the bit layout common to both variants up to
//! `bsid` is:
//!
//! ```text
//! u16 crc1
//! 2 bits  fscod
//! 6 bits  frmsizecod
//! 5 bits  bsid           (1..=8 AC-3, 11..=16 E-AC-3, everything else invalid)
//! ```
//!
//! `bsid` selects the decode path; the regular AC-3 path then reads `bsmod`,
//! `acmod`, the conditional `cmixlev` / `surmixlev` / `dsurmod` fields and
//! finally `lfeon`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::endian::{get_u16_be, get_u16_le};
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;
const MIN_CONFIRM_FRAMES: usize = 8;

/// AC-3 frame sync word, big-endian (`0x0B 0x77`).
const SYNC_WORD: u16 = 0x0B77;
/// Minimum bytes `frame_c::decode_header` requires before touching a buffer.
const HEADER_BYTES: usize = 18;

const SAMPLE_RATES: [u32; 3] = [48_000, 44_100, 32_000];

/// E-AC-3 sample-rate table (`fscod` 0..=2, then `fscod==3` selects via fscod2).
const EAC3_SAMPLE_RATES: [u32; 6] = [48_000, 44_100, 32_000, 24_000, 22_050, 16_000];

const FRAME_SIZES: [[u16; 3]; 38] = [
  [64, 69, 96],
  [64, 70, 96],
  [80, 87, 120],
  [80, 88, 120],
  [96, 104, 144],
  [96, 105, 144],
  [112, 121, 168],
  [112, 122, 168],
  [128, 139, 192],
  [128, 140, 192],
  [160, 174, 240],
  [160, 175, 240],
  [192, 208, 288],
  [192, 209, 288],
  [224, 243, 336],
  [224, 244, 336],
  [256, 278, 384],
  [256, 279, 384],
  [320, 348, 480],
  [320, 349, 480],
  [384, 417, 576],
  [384, 418, 576],
  [448, 487, 672],
  [448, 488, 672],
  [512, 557, 768],
  [512, 558, 768],
  [640, 696, 960],
  [640, 697, 960],
  [768, 835, 1152],
  [768, 836, 1152],
  [896, 975, 1344],
  [896, 976, 1344],
  [1024, 1114, 1536],
  [1024, 1115, 1536],
  [1152, 1253, 1728],
  [1152, 1254, 1728],
  [1280, 1393, 1920],
  [1280, 1394, 1920],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ac3Variant {
  Ac3,
  Eac3,
}

#[derive(Debug, Clone, Copy)]
pub struct Ac3Frame {
  pub variant: Ac3Variant,
  pub sample_rate: u32,
  pub frame_length: usize,
  pub channels: u32,
  pub bsid: u8,
}

/// Decode an AC-3 / E-AC-3 frame header. Port of `frame_c::decode_header`.
///
/// Requires at least [`HEADER_BYTES`] bytes (mirrors mkvtoolnix's
/// `buffer_size < 18` guard). Handles both the big-endian sync word
/// (`0x0B 0x77`) and the byte-swapped little-endian form (`0x77 0x0B`), swapping
/// 16-bit pairs over the header window before decoding in the latter case.
/// Never indexes out of bounds — returns `None` on any malformed input.
pub fn decode_frame(bytes: &[u8]) -> Option<Ac3Frame> {
  if bytes.len() < HEADER_BYTES {
    return None;
  }

  // Pick (and, if byte-swapped, normalise) the header window. The big-endian
  // form is decoded in place; the byte-swapped form is copied into a local
  // buffer with 16-bit pairs swapped so the bit reader sees big-endian bits.
  let mut swapped = [0u8; HEADER_BYTES];
  let header: &[u8] = if get_u16_le(bytes) == SYNC_WORD {
    swap_pairs(&bytes[..HEADER_BYTES], &mut swapped);
    &swapped
  } else if get_u16_be(bytes) == SYNC_WORD {
    &bytes[..HEADER_BYTES]
  } else {
    return None;
  };

  // bsid lives in bits [40, 45): set_bit_position(16); get_bits(29) & 0x1f.
  let mut br = BitReader::new(header);
  br.skip_bits(16).ok()?;
  let bsid = (br.read_bits(29).ok()? & 0x1f) as u8;

  // Classification mirrors ac3.cpp:124-127 exactly. 9, 10 and 17+ are invalid.
  let mut br = BitReader::new(header);
  br.skip_bits(16).ok()?;
  match bsid {
    16 => decode_header_type_eac3(&mut br, bsid),
    b if b <= 8 => decode_header_type_ac3(&mut br, bsid),
    b if (11..16).contains(&b) => decode_header_type_eac3(&mut br, bsid),
    _ => None,
  }
}

/// Swap adjacent byte pairs (16-bit byte-swap) over `src` into `dst`.
/// Mirrors `mtx::bytes::swap_buffer(.., .., 18, 2)`.
fn swap_pairs(src: &[u8], dst: &mut [u8; HEADER_BYTES]) {
  let mut i = 0;
  while i + 1 < HEADER_BYTES {
    dst[i] = src[i + 1];
    dst[i + 1] = src[i];
    i += 2;
  }
}

/// Port of `frame_c::decode_header_type_ac3`. The bit reader is positioned at
/// bit 16 (just past the sync word).
fn decode_header_type_ac3(br: &mut BitReader<'_>, bsid: u8) -> Option<Ac3Frame> {
  br.skip_bits(16).ok()?; // crc1
  let fscod = br.read_bits(2).ok()? as u8;
  if fscod == 0x03 {
    return None;
  }
  let frmsizecod = br.read_bits(6).ok()? as usize;
  if frmsizecod >= FRAME_SIZES.len() {
    return None;
  }
  br.skip_bits(5).ok()?; // bsid (already decoded)
  br.skip_bits(3).ok()?; // bsmod
  let acmod = br.read_bits(3).ok()? as u8;

  if (acmod & 0x01) != 0 && acmod != 0x01 {
    br.skip_bits(2).ok()?; // cmixlev
  }
  if (acmod & 0x04) != 0 {
    br.skip_bits(2).ok()?; // surmixlev
  }
  if acmod == 0x02 {
    br.skip_bits(2).ok()?; // dsurmod
  }
  let lfeon = br.read_bit().ok()? as u32;

  let sample_rate = SAMPLE_RATES[fscod as usize];
  let frame_length = (FRAME_SIZES[frmsizecod][fscod as usize] as usize) * 2;
  if frame_length == 0 {
    return None;
  }
  Some(Ac3Frame {
    variant: Ac3Variant::Ac3,
    sample_rate,
    frame_length,
    channels: channels_from_acmod(acmod) + lfeon,
    bsid,
  })
}

/// Port of `frame_c::decode_header_type_eac3`. The bit reader is positioned at
/// bit 16 (just past the sync word).
fn decode_header_type_eac3(br: &mut BitReader<'_>, bsid: u8) -> Option<Ac3Frame> {
  let frame_type = br.read_bits(2).ok()? as u8;
  if frame_type == 0x03 {
    // FRAME_TYPE_RESERVED
    return None;
  }
  br.skip_bits(3).ok()?; // sub stream id
  let frame_length = ((br.read_bits(11).ok()? as usize) + 1) << 1;
  if frame_length == 0 {
    return None;
  }
  let fscod = br.read_bits(2).ok()? as u8;
  let fscod2 = br.read_bits(2).ok()? as u8;
  if fscod == 0x03 && fscod2 == 0x03 {
    return None;
  }
  let acmod = br.read_bits(3).ok()? as u8;
  let lfeon = br.read_bit().ok()? as u32;

  let sr_index = if fscod == 0x03 { 3 + fscod2 } else { fscod } as usize;
  let sample_rate = EAC3_SAMPLE_RATES[sr_index];
  Some(Ac3Frame {
    variant: Ac3Variant::Eac3,
    sample_rate,
    frame_length,
    channels: channels_from_acmod(acmod) + lfeon,
    bsid,
  })
}

fn channels_from_acmod(acmod: u8) -> u32 {
  match acmod {
    0 => 2,
    1 => 1,
    2 => 2,
    3 => 3,
    4 => 3,
    5 => 4,
    6 => 4,
    7 => 5,
    _ => 0,
  }
}

/// Port of `parser_c::find_consecutive_frames`. Scans for [`MIN_CONFIRM_FRAMES`]
/// back-to-back valid frame headers, skipping the IEC 61937 16-bit `0x0110`
/// preamble wherever it appears (`get_uint16_be == 0x0110` → advance 16 bytes).
pub fn find_frame_sync(bytes: &[u8]) -> Option<usize> {
  let len = bytes.len();
  let mut base = 0usize;

  while base < len {
    let mut position = base;

    // Skip a leading IEC 61937 preamble at the search base.
    if position + 16 < len && get_u16_be(&bytes[position..]) == 0x0110 {
      position += 16;
    }

    // Advance until the first decodable frame.
    while position + 8 < len && decode_frame(&bytes[position..]).is_none() {
      position += 1;
    }
    let Some(first) = decode_frame_at(bytes, position) else {
      return None;
    };

    let mut offset = position + first.frame_length;
    let mut found = 1usize;

    while found < MIN_CONFIRM_FRAMES && offset < len {
      if offset + 16 < len && get_u16_be(&bytes[offset..]) == 0x0110 {
        offset += 16;
      }
      let Some(current) = decode_frame_at(bytes, offset) else {
        break;
      };
      if current.frame_length < 8 {
        break;
      }
      found += 1;
      offset += current.frame_length;
    }

    if found == MIN_CONFIRM_FRAMES {
      return Some(position);
    }

    base = position + 2;
  }

  None
}

/// Decode a frame at `offset`, guarding the slice bounds first.
fn decode_frame_at(bytes: &[u8], offset: usize) -> Option<Ac3Frame> {
  if offset >= bytes.len() {
    return None;
  }
  decode_frame(&bytes[offset..])
}

/// PARSER-184: locate the first decodable AC-3 / E-AC-3 frame in `bytes` and
/// return `(channels, sample_rate)`.  Mirrors `frame_c::find_in`
/// (`common/ac3.cpp:316-327`): scan byte-by-byte for the first frame whose
/// header decodes, used by mkvtoolnix's
/// `qtmp4_demuxer_c::derive_track_params_from_ac3_audio_bitstream`
/// (`r_qtmp4.cpp:3526-3536`).  Returns `None` when no frame decodes.
pub fn first_frame_params(bytes: &[u8]) -> Option<(u32, u32)> {
  let mut offset = 0usize;
  while offset < bytes.len() {
    if let Some(frame) = decode_frame(&bytes[offset..]) {
      return Some((frame.channels, frame.sample_rate));
    }
    offset += 1;
  }
  None
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Ac3Reader;

impl Reader for Ac3Reader {
  fn name(&self) -> &'static str {
    "ac3"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut probe)?;
    src.seek_to(0)?;
    if read < 6 {
      return Ok(false);
    }
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    Ok(find_frame_sync(&probe[start..read]).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut probe)?;
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    let bytes = &probe[start..read];
    let offset = find_frame_sync(bytes).ok_or(ParseError::Unrecognised)?;
    let frame = decode_frame(&bytes[offset..]).ok_or(ParseError::Unrecognised)?;

    let (codec_id, codec_name, format) = match frame.variant {
      Ac3Variant::Ac3 => ("A_AC3", "AC-3", ContainerFormat::Ac3),
      Ac3Variant::Eac3 => ("A_EAC3", "E-AC-3", ContainerFormat::Eac3),
    };
    out.container.format = format;
    out.container.recognized = true;
    out.container.supported = true;

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let audio = AudioTrackProperties {
      channels: Some(frame.channels),
      sampling_frequency: Some(frame.sample_rate as f64),
      ..AudioTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: codec_id.to_string(),
        name: Some(codec_name.to_string()),
        codec_private: None,
      },
      properties: TrackProperties {
        common,
        audio: Some(audio),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

#[cfg(test)]
pub(crate) fn build_ac3_frame(fscod: u8, frmsizecod: u8) -> Vec<u8> {
  build_ac3_frame_full(fscod, frmsizecod, 8, 2, false)
}

/// Build a synthetic AC-3 frame with explicit `bsid` / `acmod` / `lfeon`.
///
/// Field bit layout after the 16-bit sync word: `crc1` (16) · `fscod` (2) ·
/// `frmsizecod` (6) · `bsid` (5) · `bsmod` (3) · `acmod` (3) · conditional
/// `cmixlev`/`surmixlev`/`dsurmod` · `lfeon` (1).
#[cfg(test)]
pub(crate) fn build_ac3_frame_full(fscod: u8, frmsizecod: u8, bsid: u8, acmod: u8, lfeon: bool) -> Vec<u8> {
  let len = (FRAME_SIZES[frmsizecod as usize][fscod as usize] as usize) * 2;
  let mut bytes = vec![0u8; len.max(HEADER_BYTES)];
  bytes[0] = 0x0B;
  bytes[1] = 0x77;
  // byte 4: fscod (2) + frmsizecod (6)
  bytes[4] = (fscod << 6) | (frmsizecod & 0x3F);
  // byte 5: bsid (5) + bsmod (3, left 0)
  bytes[5] = (bsid & 0x1F) << 3;
  // byte 6 onward: acmod (3) + conditional fields + lfeon.
  // Compute lfeon's bit offset relative to bit 48 (start of byte 6).
  bytes[6] = (acmod & 0x07) << 5;
  let mut bit = 3u8; // bits consumed within the post-acmod region of byte 6
  if (acmod & 0x01) != 0 && acmod != 0x01 {
    bit += 2; // cmixlev
  }
  if (acmod & 0x04) != 0 {
    bit += 2; // surmixlev
  }
  if acmod == 0x02 {
    bit += 2; // dsurmod
  }
  // lfeon sits at byte6-bit `bit` (counting from MSB). All conditional fields
  // are within byte 6 for the acmod values exercised by the tests.
  if lfeon {
    bytes[6] |= 0x80 >> bit;
  }
  bytes
}

#[cfg(test)]
pub(crate) fn build_ac3_stream(frames: usize, fscod: u8, frmsizecod: u8) -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..frames {
    bytes.extend(build_ac3_frame(fscod, frmsizecod));
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn decodes_ac3_stereo_48k_192kbps() {
    let frame = build_ac3_frame(0, 8);
    let f = decode_frame(&frame).unwrap();
    assert_eq!(f.variant, Ac3Variant::Ac3);
    assert_eq!(f.sample_rate, 48_000);
    assert_eq!(f.channels, 2);
  }

  #[test]
  fn channels_from_acmod_full_table() {
    let pairs = [(0, 2), (1, 1), (2, 2), (3, 3), (4, 3), (5, 4), (6, 4), (7, 5)];
    for (acmod, expected) in pairs {
      assert_eq!(channels_from_acmod(acmod), expected);
    }
  }

  #[test]
  fn rejects_invalid_sync() {
    let mut frame = build_ac3_frame(0, 8);
    frame[0] = 0x00;
    assert!(decode_frame(&frame).is_none());
  }

  #[test]
  fn rejects_invalid_fscod() {
    let mut frame = build_ac3_frame(0, 8);
    frame[4] = (3 << 6) | 8; // fscod = 3 reserved
    assert!(decode_frame(&frame).is_none());
  }

  #[test]
  fn find_frame_sync_requires_eight() {
    let bytes = build_ac3_stream(8, 0, 8);
    assert_eq!(find_frame_sync(&bytes), Some(0));
  }

  #[test]
  fn probe_accepts_ac3_stream() {
    let bytes = build_ac3_stream(10, 0, 8);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Ac3Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_ac3_track() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_ac3_stream(10, 0, 8);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ac3", 0);
    Ac3Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Ac3);
    assert_eq!(out.tracks[0].codec.id, "A_AC3");
  }

  #[test]
  fn eac3_bsid_branch_decodes_separately() {
    let mut frame = vec![0u8; 32];
    frame[0] = 0x0B;
    frame[1] = 0x77;
    // strmtyp + substreamid don't matter for this test
    frame[2] = 0x00;
    frame[3] = 0x07; // frmsiz low bits → frame_length = (7+1)*2 = 16
    frame[4] = 0x00 << 6; // fscod = 0 → 48 kHz
    frame[5] = 12 << 3; // bsid = 12 → E-AC-3
    let f = decode_frame(&frame).unwrap();
    assert_eq!(f.variant, Ac3Variant::Eac3);
    assert_eq!(f.sample_rate, 48_000);
    assert_eq!(f.frame_length, 16);
  }

  #[test]
  fn eac3_fscod_3_uses_fscod2_for_sample_rate() {
    let mut frame = vec![0u8; 32];
    frame[0] = 0x0B;
    frame[1] = 0x77;
    frame[3] = 0x07;
    frame[4] = 0b11_00_0000; // fscod = 3, fscod2 = 0 → 24 kHz
    frame[5] = 12 << 3;
    let f = decode_frame(&frame).unwrap();
    assert_eq!(f.sample_rate, 24_000);
  }

  // ---- PARSER-003: short buffers must never panic --------------------

  #[test]
  fn six_byte_buffer_does_not_panic() {
    // A 6-byte buffer that starts with the sync word used to index past its
    // end. It must now be rejected without panicking.
    let buf = [0x0B, 0x77, 0x00, 0x00, 0x00, 0x40];
    assert!(decode_frame(&buf).is_none());
  }

  #[test]
  fn buffers_below_header_length_are_rejected() {
    let full = build_ac3_frame(0, 8);
    for len in 0..HEADER_BYTES {
      assert!(decode_frame(&full[..len]).is_none(), "len {len} should be rejected");
    }
    // Exactly HEADER_BYTES of a valid frame decodes.
    assert!(decode_frame(&full[..HEADER_BYTES]).is_some());
  }

  // ---- PARSER-004: LFE / channel counting ----------------------------

  #[test]
  fn lfe_adds_one_channel_for_acmod_2() {
    let without = build_ac3_frame_full(0, 8, 8, 2, false);
    let with = build_ac3_frame_full(0, 8, 8, 2, true);
    assert_eq!(decode_frame(&without).unwrap().channels, 2);
    assert_eq!(decode_frame(&with).unwrap().channels, 3);
  }

  #[test]
  fn lfe_adds_one_channel_for_acmod_7() {
    let without = build_ac3_frame_full(0, 8, 8, 7, false);
    let with = build_ac3_frame_full(0, 8, 8, 7, true);
    assert_eq!(decode_frame(&without).unwrap().channels, 5);
    assert_eq!(decode_frame(&with).unwrap().channels, 6);
  }

  // ---- PARSER-005: bsid classification boundaries --------------------

  #[test]
  fn bsid_8_is_valid_ac3() {
    let f = decode_frame(&build_ac3_frame_full(0, 8, 8, 2, false)).unwrap();
    assert_eq!(f.variant, Ac3Variant::Ac3);
    assert_eq!(f.bsid, 8);
  }

  #[test]
  fn bsid_9_and_10_are_rejected() {
    assert!(decode_frame(&build_ac3_frame_full(0, 8, 9, 2, false)).is_none());
    assert!(decode_frame(&build_ac3_frame_full(0, 8, 10, 2, false)).is_none());
  }

  #[test]
  fn bsid_11_is_eac3() {
    let mut frame = vec![0u8; 32];
    frame[0] = 0x0B;
    frame[1] = 0x77;
    frame[3] = 0x07; // frmsiz → length 16
    frame[5] = 11 << 3;
    let f = decode_frame(&frame).unwrap();
    assert_eq!(f.variant, Ac3Variant::Eac3);
    assert_eq!(f.bsid, 11);
  }

  #[test]
  fn bsid_16_is_eac3() {
    let mut frame = vec![0u8; 32];
    frame[0] = 0x0B;
    frame[1] = 0x77;
    frame[3] = 0x07;
    frame[5] = 16 << 3;
    let f = decode_frame(&frame).unwrap();
    assert_eq!(f.variant, Ac3Variant::Eac3);
    assert_eq!(f.bsid, 16);
  }

  #[test]
  fn bsid_17_is_rejected() {
    let mut frame = vec![0u8; 32];
    frame[0] = 0x0B;
    frame[1] = 0x77;
    frame[3] = 0x07;
    frame[5] = 17 << 3;
    assert!(decode_frame(&frame).is_none());
  }

  // ---- PARSER-006: preamble skip + byte-swapped sync -----------------

  #[test]
  fn skips_0x0110_preamble_before_frames() {
    let mut bytes = vec![0u8; 16];
    bytes[0] = 0x01;
    bytes[1] = 0x10; // 16-bit BE 0x0110 preamble
    bytes.extend(build_ac3_stream(8, 0, 8));
    assert_eq!(find_frame_sync(&bytes), Some(16));
  }

  #[test]
  fn decodes_byte_swapped_sync() {
    // Swap every 16-bit pair of a valid big-endian frame; the sync word
    // becomes 0x77 0x0B and must still decode identically.
    let be = build_ac3_frame_full(0, 8, 8, 2, true);
    let mut le = be.clone();
    let mut i = 0;
    while i + 1 < le.len() {
      le.swap(i, i + 1);
      i += 2;
    }
    assert_eq!(le[0], 0x77);
    assert_eq!(le[1], 0x0B);
    let f = decode_frame(&le).unwrap();
    let g = decode_frame(&be).unwrap();
    assert_eq!(f.variant, g.variant);
    assert_eq!(f.sample_rate, g.sample_rate);
    assert_eq!(f.channels, g.channels);
    assert_eq!(f.frame_length, g.frame_length);
  }

  #[test]
  fn find_frame_sync_skips_leading_garbage() {
    let mut bytes = vec![0xAA, 0xBB, 0xCC];
    bytes.extend(build_ac3_stream(8, 0, 8));
    assert_eq!(find_frame_sync(&bytes), Some(3));
  }

  #[test]
  fn find_frame_sync_returns_none_without_enough_frames() {
    let bytes = build_ac3_stream(3, 0, 8);
    assert_eq!(find_frame_sync(&bytes), None);
  }
}
