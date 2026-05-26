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

//! VC-1 (SMPTE 421M) elementary stream reader.
//!
//! Advanced-profile sequence-layer start code: `0x00 0x00 0x01 0x0F`.
//! After the start code:
//!
//! ```text
//! 2 bits profile     (3 = Advanced)
//! 3 bits level
//! 2 bits colordiff_format
//! 3 bits frmrtq_postproc
//! 5 bits bitrtq_postproc
//! 1 bit  postprocflag
//! 12 bits max_coded_width (in macroblocks - 1)
//! 12 bits max_coded_height (in macroblocks - 1)
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 20 * 1024 * 1024;
const SEQUENCE_HEADER_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0x0F];
const ENTRYPOINT_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0x0E];
const FRAME_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, 0x0D];

/// SMPTE 421M profile id for the Advanced profile.  mkvtoolnix's
/// `parse_sequence_header` (`common/vc1.cpp`) returns false for any other
/// profile (PARSER-243).
const PROFILE_ADVANCED: u64 = 3;

/// `s_framerate_nr` / `s_framerate_dr` from `common/vc1.cpp`.
const FRAMERATE_NR: [u32; 5] = [24, 25, 30, 50, 60];
const FRAMERATE_DR: [u32; 2] = [1000, 1001];

#[derive(Debug, Clone, Copy)]
pub struct SequenceHeader {
  pub profile: u8,
  pub level: u8,
  pub max_coded_width: u32,
  pub max_coded_height: u32,
  /// Bitstream display dimensions, present only when `display_info_flag` is
  /// set (`common/vc1.cpp`).  `None` falls back to the coded dimensions.
  pub display_width: Option<u32>,
  pub display_height: Option<u32>,
  /// Default frame duration in ns, derived from the frame-rate syntax inside
  /// the display-info block when `framerate_flag` is set and the values are
  /// valid (`p_vc1.cpp:73`, `r_mpeg_ts.cpp`-style `1e9 * num / den`).
  pub default_duration_ns: Option<u64>,
}

/// Find the next `00 00 01` start-code prefix at or after `from`, or the end of
/// the buffer when none follows.
fn next_start_code(bytes: &[u8], from: usize) -> usize {
  let mut i = from;
  while i + 3 <= bytes.len() {
    if bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1 {
      return i;
    }
    i += 1;
  }
  bytes.len()
}

/// Extract the raw bit-stream unit (start code through the byte before the next
/// start code) for `start_code`, mirroring mkvtoolnix's `get_raw_sequence_header`
/// / `get_raw_entrypoint` byte ranges.
fn raw_unit(bytes: &[u8], start_code: [u8; 4]) -> Option<Vec<u8>> {
  let pos = bytes.windows(4).position(|w| w == start_code)?;
  let end = next_start_code(bytes, pos + 4);
  Some(bytes[pos..end].to_vec())
}

/// Port of `mtx::vc1::parse_sequence_header` (`common/vc1.cpp`).  Decodes the
/// Advanced-profile sequence header — coded dimensions, the optional display
/// dimensions, and the frame-rate-derived default duration.  Returns `None`
/// for a non-Advanced profile (PARSER-243) or a truncated header, matching
/// upstream's `return false` / exception path.
pub fn decode_sequence_header(bytes: &[u8]) -> Option<SequenceHeader> {
  let pos = bytes.windows(4).position(|w| w == SEQUENCE_HEADER_CODE)?;
  // Parse the whole sequence-header unit (up to the next start code) so the
  // display-info / frame-rate fields are reachable.
  let end = next_start_code(bytes, pos + 4);
  let body = &bytes[pos + 4..end];
  let mut r = BitReader::from_rbsp(body);

  let profile = r.read_bits(2).ok()?;
  if profile != PROFILE_ADVANCED {
    return None; // PARSER-243: only Advanced profile is a valid VC-1 sequence header.
  }
  let level = r.read_bits(3).ok()? as u8;
  let _chroma_format = r.read_bits(2).ok()?;
  let _frame_rtq_postproc = r.read_bits(3).ok()?;
  let _bit_rtq_postproc = r.read_bits(5).ok()?;
  let _postproc_flag = r.read_bit().ok()?;
  let max_coded_width = ((r.read_bits(12).ok()? as u32) + 1) << 1;
  let max_coded_height = ((r.read_bits(12).ok()? as u32) + 1) << 1;
  let _pulldown_flag = r.read_bit().ok()?;
  let _interlace_flag = r.read_bit().ok()?;
  let _tf_counter_flag = r.read_bit().ok()?;
  let _f_inter_p_flag = r.read_bit().ok()?;
  let _reserved = r.read_bit().ok()?;
  let _psf_mode_flag = r.read_bit().ok()?;
  let display_info_flag = r.read_bit().ok()?;

  let mut display_width = None;
  let mut display_height = None;
  let mut default_duration_ns = None;

  if display_info_flag {
    display_width = Some((r.read_bits(14).ok()? as u32) + 1);
    display_height = Some((r.read_bits(14).ok()? as u32) + 1);

    if r.read_bit().ok()? {
      // aspect_ratio_flag
      let aspect_ratio_idx = r.read_bits(4).ok()?;
      if aspect_ratio_idx == 15 {
        let _ar_width = r.read_bits(8).ok()?;
        let _ar_height = r.read_bits(8).ok()?;
      }
      // 1..=13 select a predefined ratio; all other indices are ignored (the
      // ratio is not needed for identify, only display dimensions).
    }

    if r.read_bit().ok()? {
      // framerate_flag
      if r.read_bit().ok()? {
        // Explicit numerator 32 / denominator form.
        let den = (r.read_bits(16).ok()? as u64) + 1;
        if den != 0 {
          default_duration_ns = Some(1_000_000_000u64 * 32 / den);
        }
      } else {
        let nr = r.read_bits(8).ok()? as usize;
        let dr = r.read_bits(4).ok()? as usize;
        if (1..FRAMERATE_NR.len() + 1).contains(&nr) && (1..FRAMERATE_DR.len() + 1).contains(&dr) {
          let num = FRAMERATE_DR[dr - 1] as u64;
          let den = (FRAMERATE_NR[nr - 1] as u64) * 1000;
          if den != 0 {
            default_duration_ns = Some(1_000_000_000u64 * num / den);
          }
        }
      }
    }

    if r.read_bit().ok()? {
      // color description present
      let _color_prim = r.read_bits(8).ok()?;
      let _transfer_char = r.read_bits(8).ok()?;
      let _matrix_coef = r.read_bits(8).ok()?;
    }
  }

  Some(SequenceHeader {
    profile: profile as u8,
    level,
    max_coded_width,
    max_coded_height,
    display_width,
    display_height,
    default_duration_ns,
  })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Vc1Reader;

impl Reader for Vc1Reader {
  fn name(&self) -> &'static str {
    "vc1"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(
      read >= 4
        && (buf[..4] == SEQUENCE_HEADER_CODE || buf[..4] == ENTRYPOINT_START_CODE || buf[..4] == FRAME_START_CODE)
        && decode_sequence_header(&buf[..read]).is_some(),
    )
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let mut buf = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut buf)?;
    let header = decode_sequence_header(&buf[..read]).ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Vc1;
    out.container.recognized = true;
    out.container.supported = true;

    let pixel = Dimensions2D {
      width: header.max_coded_width,
      height: header.max_coded_height,
    };
    // PARSER-244: display dimensions come from the bitstream `display_info`
    // block when present, else fall back to the coded dimensions
    // (`p_vc1.cpp:61-68`).
    let display = match (header.display_width, header.display_height) {
      (Some(w), Some(h)) => Dimensions2D { width: w, height: h },
      _ => pixel,
    };

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let video = VideoTrackProperties {
      pixel_dimensions: Some(pixel),
      display_dimensions: Some(display),
      default_duration_ns: header.default_duration_ns,
      ..VideoTrackProperties::default()
    };
    let _ = (header.profile, header.level);

    // PARSER-244: store the raw sequence-header (plus entry-point) bit-stream
    // units as codec private, mirroring `headers_found`'s
    // `m_raw_headers = raw_seqhdr + raw_entrypoint` (`p_vc1.cpp:118-127`).
    let codec_private = collect_raw_headers(&buf[..read]).map(|bytes| CodecPrivate::from_bytes(&bytes));

    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_VC1".to_string(),
        name: Some("VC-1".to_string()),
        codec_private,
      },
      properties: TrackProperties {
        common,
        video: Some(video),
        ..TrackProperties::default()
      },
    });
    Ok(())
  }
}

/// Concatenate the raw sequence-header and (when present) entry-point bit-stream
/// units, matching mkvtoolnix's `m_raw_headers` codec private (PARSER-244).
fn collect_raw_headers(bytes: &[u8]) -> Option<Vec<u8>> {
  let mut out = raw_unit(bytes, SEQUENCE_HEADER_CODE)?;
  if let Some(entrypoint) = raw_unit(bytes, ENTRYPOINT_START_CODE) {
    out.extend_from_slice(&entrypoint);
  }
  Some(out)
}

#[cfg(test)]
pub(crate) fn build_sequence_header(max_coded_width: u32, max_coded_height: u32) -> Vec<u8> {
  // Hand-pack the bits: profile=3, level=4, colordiff=1, frmrtq=0,
  // bitrtq=0, postproc=0, max_w_mb = (w/2)-1, max_h_mb = (h/2)-1, then the
  // post-dimension flags with display_info_flag = 0.
  let mut w = BitWriter::new();
  w.write_bits(3, 2); // profile = Advanced
  w.write_bits(4, 3); // level = 4
  w.write_bits(1, 2); // colordiff_format
  w.write_bits(0, 3); // frmrtq
  w.write_bits(0, 5); // bitrtq
  w.write_bit(false); // postproc
  w.write_bits(((max_coded_width / 2) - 1) as u64, 12);
  w.write_bits(((max_coded_height / 2) - 1) as u64, 12);
  w.write_bit(false); // pulldown
  w.write_bit(false); // interlace
  w.write_bit(false); // tf_counter
  w.write_bit(false); // f_inter_p
  w.write_bit(false); // reserved
  w.write_bit(false); // psf_mode
  w.write_bit(false); // display_info_flag = 0
  let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
  bytes.extend(w.into_bytes());
  bytes
}

/// Build an Advanced-profile sequence header that carries a display-info block
/// with the given display dimensions and a 25 fps frame rate (nr=2, dr=1).
#[cfg(test)]
pub(crate) fn build_sequence_header_with_display(
  max_coded_width: u32,
  max_coded_height: u32,
  display_width: u32,
  display_height: u32,
) -> Vec<u8> {
  let mut w = BitWriter::new();
  w.write_bits(3, 2); // profile = Advanced
  w.write_bits(4, 3); // level
  w.write_bits(1, 2); // colordiff
  w.write_bits(0, 3); // frmrtq
  w.write_bits(0, 5); // bitrtq
  w.write_bit(false); // postproc
  w.write_bits(((max_coded_width / 2) - 1) as u64, 12);
  w.write_bits(((max_coded_height / 2) - 1) as u64, 12);
  w.write_bit(false); // pulldown
  w.write_bit(false); // interlace
  w.write_bit(false); // tf_counter
  w.write_bit(false); // f_inter_p
  w.write_bit(false); // reserved
  w.write_bit(false); // psf_mode
  w.write_bit(true); // display_info_flag = 1
  w.write_bits((display_width - 1) as u64, 14);
  w.write_bits((display_height - 1) as u64, 14);
  w.write_bit(false); // aspect_ratio_flag = 0
  w.write_bit(true); // framerate_flag = 1
  w.write_bit(false); // use nr/dr table form
  w.write_bits(2, 8); // nr → s_framerate_nr[1] = 25
  w.write_bits(1, 4); // dr → s_framerate_dr[0] = 1000
  w.write_bit(false); // color description present = 0
  w.write_bit(false); // hrd_param_flag = 0
  let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
  bytes.extend(w.into_bytes());
  bytes
}

/// Build a non-Advanced (Simple, profile 0) sequence header — rejected by
/// `decode_sequence_header` (PARSER-243).
#[cfg(test)]
pub(crate) fn build_simple_profile_header() -> Vec<u8> {
  let mut w = BitWriter::new();
  w.write_bits(0, 2); // profile = Simple
  w.write_bits(0, 30); // remaining padding bits
  let mut bytes = SEQUENCE_HEADER_CODE.to_vec();
  bytes.extend(w.into_bytes());
  bytes
}

#[cfg(test)]
struct BitWriter {
  buf: Vec<u8>,
  bit_index: u8,
}

#[cfg(test)]
impl BitWriter {
  fn new() -> Self {
    Self {
      buf: Vec::new(),
      bit_index: 0,
    }
  }
  fn write_bit(&mut self, b: bool) {
    if self.bit_index == 0 {
      self.buf.push(0);
    }
    if b {
      let last = self.buf.len() - 1;
      self.buf[last] |= 1 << (7 - self.bit_index);
    }
    self.bit_index = (self.bit_index + 1) % 8;
  }
  fn write_bits(&mut self, value: u64, n: u32) {
    for i in 0..n {
      self.write_bit((value >> (n - 1 - i)) & 1 != 0);
    }
  }
  fn into_bytes(mut self) -> Vec<u8> {
    while self.bit_index != 0 {
      self.write_bit(false);
    }
    self.buf
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn decodes_advanced_profile_1080p() {
    let bytes = build_sequence_header(1920, 1080);
    let h = decode_sequence_header(&bytes).unwrap();
    assert_eq!(h.profile, 3);
    assert_eq!(h.level, 4);
    assert_eq!(h.max_coded_width, 1920);
    assert_eq!(h.max_coded_height, 1080);
  }

  #[test]
  fn probe_accepts_initial_vc1_markers_when_sequence_header_is_present() {
    let bytes = build_sequence_header(1920, 1080);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Vc1Reader.probe(&mut s).unwrap());

    let mut entrypoint_first = ENTRYPOINT_START_CODE.to_vec();
    entrypoint_first.extend_from_slice(&[0u8; 8]);
    entrypoint_first.extend(build_sequence_header(1920, 1080));
    let mut s = FileSource::from_reader_for_test(Cursor::new(entrypoint_first));
    assert!(Vc1Reader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAA; 16]));
    assert!(!Vc1Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_entrypoint_without_sequence_header() {
    let bytes = ENTRYPOINT_START_CODE.to_vec();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!Vc1Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_emits_vc1_track() {
    let bytes = build_sequence_header(1280, 720);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.vc1", 0);
    Vc1Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Vc1);
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1280,
        height: 720
      })
    );
    // No display-info block → display == coded dimensions.
    assert_eq!(v.display_dimensions, v.pixel_dimensions);
    // Codec private carries the raw sequence-header unit.
    assert!(out.tracks[0].codec.codec_private.is_some());
  }

  // ---- PARSER-243: non-Advanced profiles rejected ----------------------

  #[test]
  fn non_advanced_profile_is_rejected() {
    let bytes = build_simple_profile_header();
    assert!(decode_sequence_header(&bytes).is_none());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!Vc1Reader.probe(&mut s).unwrap());
  }

  // ---- PARSER-244: display dimensions, frame rate, codec private --------

  #[test]
  fn display_info_drives_display_dimensions_and_default_duration() {
    // Coded 1920x1088, display 1920x1080, 25 fps → 40 ms default duration.
    let bytes = build_sequence_header_with_display(1920, 1088, 1920, 1080);
    let h = decode_sequence_header(&bytes).unwrap();
    assert_eq!(h.max_coded_width, 1920);
    assert_eq!(h.max_coded_height, 1088);
    assert_eq!(h.display_width, Some(1920));
    assert_eq!(h.display_height, Some(1080));
    assert_eq!(h.default_duration_ns, Some(40_000_000));

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.vc1", 0);
    Vc1Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1088 }));
    assert_eq!(v.display_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
    assert_eq!(v.default_duration_ns, Some(40_000_000));
  }

  #[test]
  fn codec_private_appends_entrypoint_unit() {
    // Sequence header followed by an entry-point unit: codec private must hold
    // both (raw_seqhdr + raw_entrypoint).
    let seq = build_sequence_header(1280, 720);
    let mut entrypoint = ENTRYPOINT_START_CODE.to_vec();
    entrypoint.extend_from_slice(&[0x12, 0x34, 0x56]);
    let mut bytes = seq.clone();
    bytes.extend_from_slice(&entrypoint);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.vc1", 0);
    Vc1Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let cp = out.tracks[0].codec.codec_private.as_ref().unwrap();
    // seqhdr unit length + entrypoint unit length (7 bytes: 4 start code + 3).
    assert_eq!(cp.length as usize, seq.len() + entrypoint.len());
  }
}
