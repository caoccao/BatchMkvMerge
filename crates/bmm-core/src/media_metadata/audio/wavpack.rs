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

//! WavPack reader. Pure-Rust port of `mkvtoolnix/src/common/wavpack.cpp` +
//! `src/input/r_wavpack.cpp`.
//!
//! - The probe enforces major version 4 (`version >> 8 == 4`), like
//!   `wavpack_reader_c::probe_file` (PARSER-017).
//! - `parse_frame` is ported so multichannel layouts accumulate channels
//!   across the consecutive blocks of the first segment (PARSER-018).
//! - Non-table sample rates (index 15) are read from the `ID_SAMPLE_RATE`
//!   metadata sub-block, and DSD rate shifting is applied (PARSER-019).
//!
//! 32-byte block header (all little-endian):
//!
//! ```text
//! 4   "wvpk"
//! u32 ck_size (frame size minus 8)
//! u16 version
//! u8  track_no / u8 index_no
//! u32 total_samples (0xFFFFFFFF = unknown)
//! u32 block_index
//! u32 block_samples
//! u32 flags
//! u32 crc
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::endian::{get_u16_le, get_u24_le, get_u32_le};
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

const HEADER_SIZE: usize = 32;

// `flags` bits (wavpack.h).
const BYTES_STORED: u32 = 3;
const MONO_FLAG: u32 = 4;
const FLOAT_DATA: u32 = 0x80;
const INT32_DATA: u32 = 0x100;
const INITIAL_BLOCK: u32 = 0x800;
const FINAL_BLOCK: u32 = 0x1000;
const SRATE_LSB: u32 = 23;
const SRATE_MASK: u32 = 0xf << SRATE_LSB;
const DSD_FLAG: u32 = 0x8000_0000;

// Metadata sub-block ids.
const ID_DSD_BLOCK: u8 = 0x0e;
const ID_OPTIONAL_DATA: u8 = 0x20;
const ID_UNIQUE: u8 = 0x3f;
const ID_ODD_SIZE: u8 = 0x40;
const ID_LARGE: u8 = 0x80;
const ID_SAMPLE_RATE: u8 = ID_OPTIONAL_DATA | 0x7;

/// Maximum payload bytes read from the initial block to recover a non-standard
/// sample rate. `read_next_header`'s validity check caps `ck_size` below 1 MiB.
const MAX_BLOCK_PAYLOAD: u64 = 1 << 20;

const SAMPLE_RATES: [u32; 15] = [
  6_000, 8_000, 9_600, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 64_000, 88_200, 96_000, 192_000,
];

#[derive(Debug, Clone, Copy)]
pub struct WavpackHeader {
  pub ck_size: u32,
  pub version: u16,
  pub total_samples: u32,
  pub block_index: u32,
  pub block_samples: u32,
  pub flags: u32,
  pub crc: u32,
}

/// Parse a 32-byte block header. Only checks the `wvpk` magic; the major
/// version is enforced by the caller (probe / `is_valid_header`).
pub fn parse_header(bytes: &[u8]) -> Option<WavpackHeader> {
  if bytes.len() < HEADER_SIZE || &bytes[..4] != b"wvpk" {
    return None;
  }
  Some(WavpackHeader {
    ck_size: get_u32_le(&bytes[4..]),
    version: get_u16_le(&bytes[8..]),
    total_samples: get_u32_le(&bytes[12..]),
    block_index: get_u32_le(&bytes[16..]),
    block_samples: get_u32_le(&bytes[20..]),
    flags: get_u32_le(&bytes[24..]),
    crc: get_u32_le(&bytes[28..]),
  })
}

/// `read_next_header` validity test: `wvpk`, even `ck_size`, `ck_size < 1 MiB`,
/// major version 4.
fn is_valid_header(h: &WavpackHeader) -> bool {
  (h.version >> 8) == 4 && (h.ck_size & 1) == 0 && (h.ck_size >> 16) < 16
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WavpackMeta {
  pub version: u16,
  pub channel_count: u32,
  pub bits_per_sample: u32,
  pub sample_rate: u32,
  pub samples_per_block: u32,
  pub total_samples: u32,
}

/// Port of `get_non_standard_rate`: walk metadata sub-blocks for an
/// `ID_SAMPLE_RATE` entry and return the rate it carries (else 0).
fn get_non_standard_rate(buffer: &[u8]) -> u32 {
  let mut pos = 0usize;
  let mut bcount = buffer.len() as i64;
  while bcount >= 2 {
    let meta_id = buffer[pos];
    let c1 = buffer[pos + 1];
    pos += 2;
    let mut meta_bc = (c1 as i64) << 1;
    bcount -= 2;

    if meta_id & ID_LARGE != 0 {
      if bcount < 2 {
        return 0;
      }
      let c1b = buffer[pos];
      let c2 = buffer[pos + 1];
      pos += 2;
      meta_bc += ((c1b as i64) << 9) + ((c2 as i64) << 17);
      bcount -= 2;
    }

    if bcount < meta_bc {
      return 0;
    }

    if (meta_id & ID_UNIQUE) == ID_SAMPLE_RATE && meta_bc == 4 {
      let mut sample_rate = get_u24_le(&buffer[pos..]) as i64;
      if meta_id & ID_ODD_SIZE == 0 {
        sample_rate |= ((buffer[pos + 3] & 0x7f) as i64) << 24;
      }
      return sample_rate as u32;
    }

    bcount -= meta_bc;
    pos += meta_bc as usize;
  }
  0
}

/// Port of `get_dsd_rate_shifter`: walk metadata sub-blocks for a DSD block and
/// return its rate-shift amount (else 0).
fn get_dsd_rate_shifter(buffer: &[u8]) -> u32 {
  let mut pos = 0usize;
  let mut bcount = buffer.len() as i64;
  while bcount >= 2 {
    let meta_id = buffer[pos];
    let c1 = buffer[pos + 1];
    pos += 2;
    let mut meta_bc = (c1 as i64) << 1;
    bcount -= 2;

    if meta_id & ID_LARGE != 0 {
      if bcount < 2 {
        return 0;
      }
      let c1b = buffer[pos];
      let c2 = buffer[pos + 1];
      pos += 2;
      meta_bc += ((c1b as i64) << 9) + ((c2 as i64) << 17);
      bcount -= 2;
    }

    if bcount < meta_bc {
      return 0;
    }

    if (meta_id & ID_UNIQUE) == ID_DSD_BLOCK && meta_bc != 0 && buffer[pos] <= 31 {
      return buffer[pos] as u32;
    }

    bcount -= meta_bc;
    pos += meta_bc as usize;
  }
  0
}

/// Port of `read_next_header` (`../mkvtoolnix/src/common/wavpack.cpp:54-90`):
/// scan forward from `start` for a valid 32-byte WavPack block header, skipping
/// junk byte-by-byte and giving up only after more than 1 MiB has been skipped.
/// On success returns `(header_start, header)` and leaves the cursor positioned
/// immediately after the located 32-byte header (so a payload read can follow);
/// returns `Ok(None)` when no header is found within the skip budget or EOF is
/// reached (mirrors the `-1` return).
fn read_next_header(src: &mut FileSource, start: u64) -> Result<Option<(u64, WavpackHeader)>, ParseError> {
  const MAX_SKIP: u64 = 1 << 20;
  let mut scan = start;
  let mut skipped = 0u64;
  loop {
    src.seek_to(scan)?;
    let mut hdr = [0u8; HEADER_SIZE];
    let n = src.read_at_most(&mut hdr)?;
    if n < HEADER_SIZE {
      return Ok(None);
    }
    if &hdr[..4] == b"wvpk" {
      if let Some(h) = parse_header(&hdr) {
        if is_valid_header(&h) {
          // Reposition to right after the header for any subsequent payload read.
          src.seek_to(scan + HEADER_SIZE as u64)?;
          return Ok(Some((scan, h)));
        }
      }
    }
    // Advance to the next 'w' in this window (mkvtoolnix scans byte-by-byte for
    // the next sync), or past the whole window if none is present.
    let next_w = hdr[1..]
      .iter()
      .position(|&b| b == b'w')
      .map(|i| i + 1)
      .unwrap_or(HEADER_SIZE);
    skipped += next_w as u64;
    if skipped > MAX_SKIP {
      return Ok(None);
    }
    scan += next_w as u64;
  }
}

/// Port of `parse_frame` for identification: walk the consecutive blocks of the
/// first segment, accumulating channels and decoding format fields from the
/// initial block.  Block boundaries are located with `read_next_header`, which
/// resynchronises across padding / junk gaps between blocks (PARSER-253).
#[cfg(test)]
fn parse_frame(src: &mut FileSource) -> Result<Option<WavpackMeta>, ParseError> {
  parse_frame_with_deadline(src, None)
}

fn parse_frame_with_deadline(
  src: &mut FileSource,
  deadline: Option<&Deadline>,
) -> Result<Option<WavpackMeta>, ParseError> {
  let mut meta = WavpackMeta::default();
  let mut pos = 0u64;
  let mut can_leave = false;
  let mut first = true;

  while !can_leave {
    if let Some(deadline) = deadline {
      deadline.check("wavpack::frame")?;
    }
    let (header_start, h) = match read_next_header(src, pos)? {
      Some(found) => found,
      None => {
        // No valid header within the skip budget. For the first block this is
        // an unrecognised file; afterwards, stop with what we already have.
        if first {
          return Ok(None);
        }
        break;
      }
    };
    first = false;
    if meta.version == 0 {
      meta.version = h.version;
    }
    meta.total_samples = h.total_samples;

    if h.block_samples != 0 {
      let flags = h.flags;
      meta.channel_count += if flags & MONO_FLAG != 0 { 1 } else { 2 };

      if flags & INITIAL_BLOCK != 0 {
        let mut non_standard_rate = 0u32;
        let mut dsd_rate_shifter = 0u32;
        let mut sample_rate = (flags & SRATE_MASK) >> SRATE_LSB;

        if sample_rate == 15 || flags & DSD_FLAG != 0 {
          // ck_size - sizeof(header_t)(32) + 8 = ck_size - 24.
          let adjusted = (h.ck_size as i64 - HEADER_SIZE as i64 + 8).max(0) as u64;
          // Cursor is positioned right after the 32-byte header.
          let payload = src.read_vec_capped(adjusted, MAX_BLOCK_PAYLOAD)?;
          if sample_rate == 15 {
            non_standard_rate = get_non_standard_rate(&payload);
          }
          if flags & DSD_FLAG != 0 {
            dsd_rate_shifter = get_dsd_rate_shifter(&payload);
          }
        }

        if sample_rate < 15 {
          sample_rate = SAMPLE_RATES[sample_rate as usize];
        } else if non_standard_rate != 0 {
          sample_rate = non_standard_rate;
        }
        if flags & DSD_FLAG != 0 {
          sample_rate <<= dsd_rate_shifter;
        }
        meta.sample_rate = sample_rate;

        meta.bits_per_sample = if flags & (INT32_DATA | FLOAT_DATA) != 0 {
          32
        } else {
          ((flags & BYTES_STORED) + 1) << 3
        };
        meta.samples_per_block = h.block_samples;
        // Reset to this block's channel count (the segment accumulates
        // from here across subsequent non-initial blocks).
        meta.channel_count = if flags & MONO_FLAG != 0 { 1 } else { 2 };

        if flags & FINAL_BLOCK != 0 {
          can_leave = true;
        }
      } else if flags & FINAL_BLOCK != 0 {
        can_leave = true;
      }
    }

    if !can_leave {
      // Advance to the next block: full frame size is ck_size + 8, measured
      // from where this header actually started (post-resync).
      pos = header_start.saturating_add(h.ck_size as u64 + 8);
    }
  }

  Ok(Some(meta))
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WavpackReader;

impl Reader for WavpackReader {
  fn name(&self) -> &'static str {
    "wavpack"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; HEADER_SIZE];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < HEADER_SIZE {
      return Ok(false);
    }
    match parse_header(&head) {
      Some(h) => Ok((h.version >> 8) == 4),
      None => Ok(false),
    }
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    src.seek_to(0)?;
    let meta = parse_frame_with_deadline(src, Some(deadline))?.ok_or(ParseError::Unrecognised)?;
    // The probe already required a valid version-4 first block; an empty
    // walk (no channels) means nothing decodable was found.
    if meta.channel_count == 0 {
      return Err(ParseError::Unrecognised);
    }

    out.container.format = ContainerFormat::Wavpack;
    out.container.recognized = true;
    out.container.supported = true;

    if meta.sample_rate > 0 && meta.total_samples != u32::MAX {
      let ns = (meta.total_samples as u128) * 1_000_000_000 / meta.sample_rate as u128;
      out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
    }

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let audio = AudioTrackProperties {
      channels: Some(meta.channel_count),
      sampling_frequency: if meta.sample_rate == 0 {
        None
      } else {
        Some(meta.sample_rate as f64)
      },
      bit_depth: if meta.bits_per_sample == 0 {
        None
      } else {
        Some(meta.bits_per_sample)
      },
      ..AudioTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: "A_WAVPACK4".to_string(),
        name: Some("WavPack".to_string()),
        codec_private: Some(CodecPrivate::from_bytes(&meta.version.to_le_bytes())),
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
pub(crate) fn build_block(
  sample_rate_idx: u32,
  flags_extra: u32,
  mono: bool,
  initial: bool,
  final_block: bool,
  total_samples: u32,
  block_samples: u32,
  payload: &[u8],
) -> Vec<u8> {
  let mut flags = (sample_rate_idx & 0xf) << SRATE_LSB;
  flags |= flags_extra;
  if mono {
    flags |= MONO_FLAG;
  }
  if initial {
    flags |= INITIAL_BLOCK;
  }
  if final_block {
    flags |= FINAL_BLOCK;
  }
  // ck_size = (full frame size) - 8 = 32 + payload - 8 = 24 + payload, made even.
  let mut ck_size = 24 + payload.len() as u32;
  if ck_size & 1 != 0 {
    ck_size += 1;
  }
  let mut bytes = vec![0u8; HEADER_SIZE];
  bytes[..4].copy_from_slice(b"wvpk");
  bytes[4..8].copy_from_slice(&ck_size.to_le_bytes());
  bytes[8..10].copy_from_slice(&0x0407u16.to_le_bytes()); // version 4.x
  bytes[12..16].copy_from_slice(&total_samples.to_le_bytes());
  bytes[20..24].copy_from_slice(&block_samples.to_le_bytes());
  bytes[24..28].copy_from_slice(&flags.to_le_bytes());
  bytes.extend_from_slice(payload);
  // Pad to the even ck_size.
  bytes.resize(8 + ck_size as usize, 0);
  bytes
}

#[cfg(test)]
pub(crate) fn build_wavpack_header(
  sample_rate: u32,
  bps_index: u8,
  mono: bool,
  total_samples: u32,
  block_samples: u32,
) -> Vec<u8> {
  let sr_index = SAMPLE_RATES.iter().position(|&s| s == sample_rate).unwrap_or(15) as u32;
  build_block(
    sr_index,
    bps_index as u32 & BYTES_STORED,
    mono,
    true,
    true,
    total_samples,
    block_samples,
    &[],
  )
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn parse_header_decodes_basic_fields() {
    let bytes = build_wavpack_header(44_100, 2, false, 88_200, 1024);
    let h = parse_header(&bytes).unwrap();
    assert_eq!(h.version, 0x0407);
    assert_eq!(h.total_samples, 88_200);
    assert!(is_valid_header(&h));
  }

  // ---- PARSER-017: version 4 enforcement --------------------------------

  #[test]
  fn probe_rejects_non_version_4() {
    let mut bytes = build_wavpack_header(44_100, 2, false, 1, 1);
    bytes[8..10].copy_from_slice(&0x0510u16.to_le_bytes()); // version 5
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!WavpackReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_version_4() {
    let bytes = build_wavpack_header(44_100, 2, false, 1, 1);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(WavpackReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_non_wvpk() {
    let mut bytes = build_wavpack_header(48_000, 1, false, 1, 1);
    bytes[0] = b'X';
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!WavpackReader.probe(&mut s).unwrap());
  }

  #[test]
  fn single_stereo_block() {
    let bytes = build_wavpack_header(44_100, 2, false, 88_200, 1024);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 2);
    assert_eq!(meta.sample_rate, 44_100);
    assert_eq!(meta.bits_per_sample, 24);
  }

  #[test]
  fn mono_block() {
    let bytes = build_wavpack_header(48_000, 1, true, 96_000, 1024);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 1);
  }

  // ---- PARSER-018: multichannel accumulation ----------------------------

  #[test]
  fn multichannel_accumulates_channels() {
    // 5.1 layout: initial stereo, middle stereo, final stereo → 6 channels.
    let mut bytes = build_block(9, 1, false, true, false, 88_200, 1024, &[0u8; 8]);
    bytes.extend(build_block(9, 1, false, false, false, 88_200, 1024, &[0u8; 8]));
    bytes.extend(build_block(9, 1, false, false, true, 88_200, 1024, &[0u8; 8]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 6);
    assert_eq!(meta.sample_rate, 44_100);
  }

  // ---- PARSER-253: resynchronise across junk/padding between blocks -----

  #[test]
  fn resync_skips_junk_between_blocks() {
    // initial stereo (non-final) + junk gap + final stereo. The old exact-offset
    // walk stopped at the junk and reported 2 channels; resync recovers all 4.
    let mut bytes = build_block(9, 1, false, true, false, 88_200, 1024, &[0u8; 8]);
    bytes.extend_from_slice(&[0xAB, 0xCD, 0xEF, 0x12]); // junk, no 'w' byte
    bytes.extend(build_block(9, 1, false, false, true, 88_200, 1024, &[0u8; 8]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 4);
    assert_eq!(meta.sample_rate, 44_100);
  }

  #[test]
  fn resync_gives_up_when_no_following_block() {
    // initial non-final stereo block followed by bytes that never resync to a
    // 'wvpk' header → stop with the channels gathered so far (no fabrication).
    let mut bytes = build_block(9, 1, false, true, false, 88_200, 1024, &[0u8; 8]);
    bytes.extend_from_slice(&[0x00u8; 64]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 2);
  }

  #[test]
  fn multichannel_mixed_mono_blocks() {
    // initial stereo + mono + mono → 4 channels.
    let mut bytes = build_block(10, 1, false, true, false, 0, 1024, &[0u8; 4]);
    bytes.extend(build_block(10, 1, true, false, false, 0, 1024, &[0u8; 4]));
    bytes.extend(build_block(10, 1, true, false, true, 0, 1024, &[0u8; 4]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 4);
    assert_eq!(meta.sample_rate, 48_000);
  }

  #[test]
  fn multichannel_walks_past_old_1024_block_limit() {
    let mut bytes = build_block(9, 1, true, true, false, 88_200, 1024, &[]);
    for _ in 0..1024 {
      bytes.extend(build_block(9, 1, true, false, false, 88_200, 1024, &[]));
    }
    bytes.extend(build_block(9, 1, true, false, true, 88_200, 1024, &[]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.channel_count, 1026);
    assert_eq!(meta.sample_rate, 44_100);
  }

  // ---- PARSER-019: non-standard sample rate -----------------------------

  #[test]
  fn non_standard_sample_rate_from_metadata() {
    // ID_SAMPLE_RATE sub-block carrying 50000 Hz (3-byte even size).
    // meta_id = ID_SAMPLE_RATE (no ID_ODD_SIZE), size word = 2 → meta_bc = 4.
    let rate: u32 = 50_000;
    let mut payload = vec![ID_SAMPLE_RATE, 2];
    payload.extend_from_slice(&rate.to_le_bytes()); // 4 bytes
    let bytes = build_block(15, 1, false, true, true, 100_000, 1024, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.sample_rate, 50_000);
  }

  #[test]
  fn index_15_without_metadata_keeps_raw_index() {
    // mkvtoolnix only remaps the rate when the index is < 15 or an
    // ID_SAMPLE_RATE block is present; otherwise the raw index 15 is left
    // in place (it is not coerced to 0).
    let bytes = build_block(15, 1, false, true, true, u32::MAX, 1024, &[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.sample_rate, 15);
  }

  #[test]
  fn float_data_is_32_bit() {
    let bytes = build_block(10, FLOAT_DATA, false, true, true, 0, 1024, &[]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let meta = parse_frame(&mut s).unwrap().unwrap();
    assert_eq!(meta.bits_per_sample, 32);
  }

  #[test]
  fn read_headers_populates_track_and_duration() {
    let bytes = build_wavpack_header(44_100, 1, false, 88_200, 1024);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.wv", 0);
    WavpackReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.bit_depth, Some(16));
    assert_eq!(a.channels, Some(2));
    assert_eq!(out.tracks[0].codec.codec_private.as_ref().unwrap().hex, "0704");
    assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
  }

  #[test]
  fn read_headers_multichannel_reports_six() {
    let mut bytes = build_block(9, 1, false, true, false, 88_200, 1024, &[0u8; 8]);
    bytes.extend(build_block(9, 1, false, false, false, 88_200, 1024, &[0u8; 8]));
    bytes.extend(build_block(9, 1, false, false, true, 88_200, 1024, &[0u8; 8]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.wv", 0);
    WavpackReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].properties.audio.as_ref().unwrap().channels, Some(6));
  }

  #[test]
  fn read_headers_handles_unknown_total_samples() {
    let bytes = build_wavpack_header(44_100, 1, false, u32::MAX, 1024);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.wv", 0);
    WavpackReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.properties.duration.is_none());
  }
}
