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

//! MPEG-1/2/2.5 Audio reader (Layer I/II/III). Pure-Rust port of
//! `mkvtoolnix/src/common/mp3.cpp` + `src/input/r_mp3.cpp`.
//!
//! Layer I and Layer II are now decoded alongside Layer III with the correct
//! frame sizing and Matroska codec ID (PARSER-014), and the probe requires
//! [`MIN_CONFIRM_FRAMES`] back-to-back frames whose version/layer/channel/
//! sample-rate agree (PARSER-015), matching `find_consecutive_mp3_headers`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::endian::get_u32_be;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const PROBE_BYTES: usize = 128 * 1024;
/// `r_mp3.cpp::read_headers` confirms a stream with five consecutive headers.
const MIN_CONFIRM_FRAMES: u32 = 5;

/// `mp3_bitrates_mpeg1[layer-1][index]` (kbps).
const BITRATES_MPEG1: [[u32; 16]; 3] = [
  [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0],
  [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0],
  [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0],
];

/// `mp3_bitrates_mpeg2[layer-1][index]` (kbps), shared by MPEG-2 and 2.5.
const BITRATES_MPEG2: [[u32; 16]; 3] = [
  [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0],
  [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
  [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0],
];

/// `mp3_sampling_freqs[version-1][index]` (Hz).
const SAMPLING_FREQS: [[u32; 4]; 3] = [
  [44100, 48000, 32000, 0],
  [22050, 24000, 16000, 0],
  [11025, 12000, 8000, 0],
];

/// `mp3_samples_per_channel[version-1][layer-1]`.
const SAMPLES_PER_CHANNEL: [[u32; 3]; 3] = [[384, 1152, 1152], [384, 1152, 576], [384, 1152, 576]];

/// Port of `mp3_header_t`. `version`: 1 = MPEG-1, 2 = MPEG-2, 3 = MPEG-2.5.
/// `layer`: 1 = Layer I, 2 = Layer II, 3 = Layer III.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp3Header {
  pub version: u8,
  pub layer: u8,
  pub bitrate: u32,
  pub sampling_frequency: u32,
  pub padding: u32,
  pub channel_mode: u8,
  pub channels: u32,
  pub samples_per_channel: u32,
  pub framesize: usize,
  pub is_tag: bool,
}

/// Port of `decode_mp3_header`. Handles ID3v2 (`ID3`) / ID3v1 (`TAG`) markers
/// as tag "frames" and decodes MPEG audio frame headers otherwise.
pub fn decode_mp3_header(buf: &[u8]) -> Option<Mp3Header> {
  if buf.len() >= 3 && &buf[0..3] == b"ID3" {
    if buf.len() < 10 {
      return None;
    }
    let mut framesize: usize = 0;
    for &b in &buf[6..10] {
      framesize <<= 7;
      framesize |= (b & 0x7f) as usize;
    }
    framesize += 10;
    if buf[3] >= 4 && (buf[5] & 0x10) == 0x10 {
      framesize += 10;
    }
    return Some(tag_header(framesize));
  }

  if buf.len() >= 3 && &buf[0..3] == b"TAG" {
    return Some(tag_header(128));
  }

  if buf.len() < 4 {
    return None;
  }
  let header = get_u32_be(buf);

  let raw_version = ((header >> 19) & 3) as u8;
  let layer = 4 - ((header >> 17) & 3) as u8;

  if raw_version == 1 {
    return None; // reserved MPEG version
  }
  if layer == 4 {
    return None; // reserved layer
  }

  // 0 → 3 (MPEG-2.5), 3 → 1 (MPEG-1), else unchanged (2 → MPEG-2).
  let version = if raw_version == 0 {
    3
  } else if raw_version == 3 {
    1
  } else {
    raw_version
  };

  let bitrate_index = ((header >> 12) & 15) as usize;
  let bitrate = if version == 1 {
    BITRATES_MPEG1[(layer - 1) as usize][bitrate_index]
  } else {
    BITRATES_MPEG2[(layer - 1) as usize][bitrate_index]
  };
  let sampling_frequency = SAMPLING_FREQS[(version - 1) as usize][((header >> 10) & 3) as usize];
  let padding = ((header >> 9) & 1) as u32;
  let channel_mode = ((header >> 6) & 3) as u8;
  let channels = if channel_mode == 3 { 1 } else { 2 };
  let samples_per_channel = SAMPLES_PER_CHANNEL[(version - 1) as usize][(layer - 1) as usize];

  if sampling_frequency == 0 {
    return None;
  }

  let framesize = if layer == 3 {
    (if version == 1 { 144000 } else { 72000 }) * bitrate as usize / sampling_frequency as usize + padding as usize
  } else if layer == 2 {
    144000 * bitrate as usize / sampling_frequency as usize + padding as usize
  } else {
    (12000 * bitrate as usize / sampling_frequency as usize + padding as usize) * 4
  };

  if framesize == 0 {
    return None;
  }

  Some(Mp3Header {
    version,
    layer,
    bitrate,
    sampling_frequency,
    padding,
    channel_mode,
    channels,
    samples_per_channel,
    framesize,
    is_tag: false,
  })
}

fn tag_header(framesize: usize) -> Mp3Header {
  Mp3Header {
    version: 0,
    layer: 0,
    bitrate: 0,
    sampling_frequency: 0,
    padding: 0,
    channel_mode: 0,
    channels: 0,
    samples_per_channel: 0,
    framesize,
    is_tag: true,
  }
}

const FOURCC_RIFF: u32 = 0x5249_4646;

/// Port of `find_mp3_header`. Returns the byte offset of the first candidate
/// MPEG audio / tag header, applying the same field-validity pre-filters.
pub fn find_mp3_header(buf: &[u8]) -> Option<usize> {
  let size = buf.len();
  if size < 4 {
    return None;
  }
  let mut pos = 0usize;
  while pos + 4 < size {
    if buf[pos] == b'I' && buf[pos + 1] == b'D' && buf[pos + 2] == b'3' {
      if pos + 10 >= size {
        return None;
      }
      return Some(pos);
    }
    if buf[pos] == b'T' && buf[pos + 1] == b'A' && buf[pos + 2] == b'G' {
      return Some(pos);
    }

    let header = get_u32_be(&buf[pos..]);
    if header == FOURCC_RIFF
            || (header & 0xffe0_0000) != 0xffe0_0000      // sync
            || ((header >> 17) & 3) == 0                  // layer reserved
            || ((header >> 12) & 0xf) == 0xf              // bitrate bad
            || ((header >> 12) & 0xf) == 0                // bitrate free
            || ((header >> 10) & 0x3) == 0x3              // sample rate reserved
            || ((header >> 19) & 3) == 0x1                // version reserved
            || (header & 0xffff_0000) == 0xfffe_0000
    {
      pos += 1;
      continue;
    }
    return Some(pos);
  }
  None
}

/// PARSER-184: decode the first MPEG-1/2 audio (MP2/MP3) header in `buf` and
/// return `(channels, sampling_frequency)`.  Mirrors mkvtoolnix's
/// `qtmp4_demuxer_c::derive_track_params_from_mp3_audio_bitstream`
/// (`r_qtmp4.cpp:3552-3565`): `find_mp3_header` + `decode_mp3_header`, ignoring
/// tag "frames" (which carry no audio parameters).  Returns `None` when no
/// decodable audio frame header is found.
pub fn first_header_params(buf: &[u8]) -> Option<(u32, u32)> {
  let offset = find_mp3_header(buf)?;
  let header = decode_mp3_header(&buf[offset..])?;
  if header.is_tag || header.sampling_frequency == 0 {
    return None;
  }
  Some((header.channels, header.sampling_frequency))
}

/// Port of `find_consecutive_mp3_headers`. Skips leading tags, then requires
/// `num` consecutive frames whose version/layer/channels/sample-rate agree.
/// Returns `(offset, first_header)`.
pub fn find_consecutive_mp3_headers(buf: &[u8], num: u32) -> Option<(usize, Mp3Header)> {
  let size = buf.len();
  let mut base = 0usize;

  // Find the first non-tag header, skipping any tags.
  let (first_pos, mut mp3header) = loop {
    let pos = find_mp3_header(&buf[base..])?;
    match decode_mp3_header(&buf[base + pos..]) {
      Some(h) if !h.is_tag => break (pos, h),
      Some(h) => base += h.framesize.max(1),
      None => return None,
    }
    if base >= size {
      return None;
    }
  };

  if num == 1 {
    return Some((base + first_pos, mp3header));
  }

  base += first_pos;

  loop {
    let mut offset = mp3header.framesize;
    let mut i = 0u32;
    while i < num - 1 {
      if size.saturating_sub(base + offset) < 4 {
        break;
      }
      if find_mp3_header(&buf[base + offset..]) == Some(0) {
        match decode_mp3_header(&buf[base + offset..]) {
          Some(new_header)
            if new_header.version == mp3header.version
              && new_header.layer == mp3header.layer
              && new_header.channels == mp3header.channels
              && new_header.sampling_frequency == mp3header.sampling_frequency =>
          {
            offset += new_header.framesize;
            i += 1;
            continue;
          }
          _ => break,
        }
      }
      break;
    }

    if i == num - 1 {
      return Some((base, mp3header));
    }

    base += 1;
    let pos = find_mp3_header(&buf[base.min(size)..])?;
    if let Some(h) = decode_mp3_header(&buf[base + pos..]) {
      mp3header = h;
    }
    base += pos;

    if base >= size.saturating_sub(5) {
      break;
    }
  }

  None
}

/// Matroska codec ID (and display name) for the MPEG audio layer.  Mirrors
/// `mp3_header_t::get_codec()` so containers that default an MPEG-audio row to
/// `A_MPEG/L3` can specialise it to the actual layer once a frame header is
/// decoded (PARSER-250 / PARSER-252).
pub fn codec_for_layer(layer: u8) -> (&'static str, &'static str) {
  match layer {
    1 => ("A_MPEG/L1", "MP1"),
    2 => ("A_MPEG/L2", "MP2"),
    _ => ("A_MPEG/L3", "MP3"),
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Mp3Reader;

impl Reader for Mp3Reader {
  fn name(&self) -> &'static str {
    "mp3"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut probe)?;
    src.seek_to(0)?;
    if read < 4 {
      return Ok(false);
    }
    let (start, _end) = id3v2::payload_bounds(&probe[..read]);
    Ok(find_consecutive_mp3_headers(&probe[start..read], MIN_CONFIRM_FRAMES).is_some())
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
    let (_offset, frame) = find_consecutive_mp3_headers(bytes, MIN_CONFIRM_FRAMES).ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Mp3;
    out.container.recognized = true;
    out.container.supported = true;

    let (codec_id, codec_name) = codec_for_layer(frame.layer);

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let audio = AudioTrackProperties {
      channels: Some(frame.channels),
      sampling_frequency: Some(frame.sampling_frequency as f64),
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
pub(crate) fn build_mp3_frame(version: u8, layer: u8, bitrate_kbps: u32, sample_rate: u32, mono: bool) -> Vec<u8> {
  // version: 1=MPEG1, 2=MPEG2, 3=MPEG2.5; layer: 1/2/3.
  let raw_version = match version {
    1 => 0b11,
    2 => 0b10,
    3 => 0b00,
    _ => unreachable!(),
  };
  let raw_layer = match layer {
    1 => 0b11,
    2 => 0b10,
    3 => 0b01,
    _ => unreachable!(),
  };
  let bitrate_table = if version == 1 {
    &BITRATES_MPEG1[(layer - 1) as usize]
  } else {
    &BITRATES_MPEG2[(layer - 1) as usize]
  };
  let bitrate_index = bitrate_table.iter().position(|&b| b == bitrate_kbps).unwrap() as u32;
  let sr_index = SAMPLING_FREQS[(version - 1) as usize]
    .iter()
    .position(|&s| s == sample_rate)
    .unwrap() as u32;

  // AAAAAAAA AAABBCCD EEEEFFGH IIJJKLMM
  let mut header: u32 = 0xffe0_0000;
  header |= (raw_version as u32) << 19;
  header |= (raw_layer as u32) << 17;
  header |= 1 << 16; // protection bit set (no CRC)
  header |= bitrate_index << 12;
  header |= sr_index << 10;
  let channel_mode: u32 = if mono { 3 } else { 0 };
  header |= channel_mode << 6;

  let head = header.to_be_bytes();
  let frame = decode_mp3_header(&head).unwrap();
  let mut bytes = Vec::with_capacity(frame.framesize);
  bytes.extend_from_slice(&head);
  bytes.resize(frame.framesize.max(4), 0);
  bytes
}

#[cfg(test)]
pub(crate) fn build_mp3_frame_v1(bitrate_kbps: u32, sample_rate: u32, mono: bool) -> Vec<u8> {
  build_mp3_frame(1, 3, bitrate_kbps, sample_rate, mono)
}

#[cfg(test)]
pub(crate) fn build_mp3_stream(frames: usize, bitrate: u32, sample_rate: u32) -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..frames {
    bytes.extend(build_mp3_frame_v1(bitrate, sample_rate, false));
  }
  bytes
}

#[cfg(test)]
fn build_layer_stream(frames: usize, version: u8, layer: u8, bitrate: u32, sample_rate: u32) -> Vec<u8> {
  let mut bytes = Vec::new();
  for _ in 0..frames {
    bytes.extend(build_mp3_frame(version, layer, bitrate, sample_rate, false));
  }
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn decodes_mpeg1_layer3_128kbps_44100() {
    let frame = build_mp3_frame_v1(128, 44_100, false);
    let f = decode_mp3_header(&frame).unwrap();
    assert_eq!(f.version, 1);
    assert_eq!(f.layer, 3);
    assert_eq!(f.bitrate, 128);
    assert_eq!(f.sampling_frequency, 44_100);
    assert_eq!(f.channels, 2);
    assert_eq!(f.framesize, 417);
  }

  // ---- PARSER-014: Layer I and Layer II support -------------------------

  #[test]
  fn decodes_layer1() {
    let frame = build_mp3_frame(1, 1, 256, 44_100, false);
    let f = decode_mp3_header(&frame).unwrap();
    assert_eq!(f.layer, 1);
    // Layer I: (12000 * 256 / 44100 + 0) * 4 = 69 * 4 = 276.
    assert_eq!(f.framesize, (12000 * 256 / 44100) * 4);
    assert_eq!(f.samples_per_channel, 384);
    let (id, name) = codec_for_layer(f.layer);
    assert_eq!(id, "A_MPEG/L1");
    assert_eq!(name, "MP1");
  }

  #[test]
  fn decodes_layer2() {
    let frame = build_mp3_frame(1, 2, 128, 48_000, false);
    let f = decode_mp3_header(&frame).unwrap();
    assert_eq!(f.layer, 2);
    assert_eq!(f.framesize, 144000 * 128 / 48000);
    assert_eq!(f.samples_per_channel, 1152);
    let (id, name) = codec_for_layer(f.layer);
    assert_eq!(id, "A_MPEG/L2");
    assert_eq!(name, "MP2");
  }

  #[test]
  fn decodes_mpeg2_layer3() {
    let frame = build_mp3_frame(2, 3, 64, 22_050, false);
    let f = decode_mp3_header(&frame).unwrap();
    assert_eq!(f.version, 2);
    assert_eq!(f.layer, 3);
    // MPEG-2 Layer III uses 72000.
    assert_eq!(f.framesize, 72000 * 64 / 22050);
    assert_eq!(f.samples_per_channel, 576);
  }

  #[test]
  fn decodes_mpeg25_layer3() {
    let frame = build_mp3_frame(3, 3, 32, 8_000, false);
    let f = decode_mp3_header(&frame).unwrap();
    assert_eq!(f.version, 3);
    assert_eq!(f.sampling_frequency, 8_000);
  }

  #[test]
  fn rejects_reserved_version_and_layer() {
    // Reserved version (01).
    let header: u32 = 0xffe0_0000 | (0b01 << 19) | (0b01 << 17) | (1 << 16) | (4 << 12) | (0 << 10);
    assert!(decode_mp3_header(&header.to_be_bytes()).is_none());
    // Reserved layer (00).
    let header: u32 = 0xffe0_0000 | (0b11 << 19) | (0b00 << 17) | (1 << 16) | (4 << 12);
    assert!(decode_mp3_header(&header.to_be_bytes()).is_none());
  }

  #[test]
  fn rejects_reserved_sample_rate() {
    let mut frame = build_mp3_frame_v1(128, 44_100, false);
    frame[2] |= 0x0C; // sample_rate_index = 3
    assert!(decode_mp3_header(&frame).is_none());
  }

  #[test]
  fn mono_channel_mode() {
    let f = decode_mp3_header(&build_mp3_frame_v1(96, 48_000, true)).unwrap();
    assert_eq!(f.channels, 1);
  }

  #[test]
  fn decodes_id3v2_tag_marker() {
    // 'ID3', version 4, flags with footer bit, synchsafe size 1 → 10+10+1.
    let buf = [b'I', b'D', b'3', 4, 0, 0x10, 0, 0, 0, 1];
    let h = decode_mp3_header(&buf).unwrap();
    assert!(h.is_tag);
    assert_eq!(h.framesize, 10 + 10 + 1);
  }

  #[test]
  fn decodes_id3v1_tag_marker() {
    let h = decode_mp3_header(b"TAGxxx").unwrap();
    assert!(h.is_tag);
    assert_eq!(h.framesize, 128);
  }

  #[test]
  fn find_mp3_header_skips_riff_and_garbage() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(build_mp3_frame_v1(128, 44_100, false));
    assert_eq!(find_mp3_header(&bytes), Some(4));
  }

  // ---- PARSER-015: matching consecutive frames --------------------------

  #[test]
  fn consecutive_requires_matching_frames() {
    let bytes = build_mp3_stream(6, 128, 44_100);
    assert_eq!(
      find_consecutive_mp3_headers(&bytes, MIN_CONFIRM_FRAMES).map(|(o, _)| o),
      Some(0)
    );
  }

  #[test]
  fn consecutive_rejects_mismatched_followups() {
    // One 44100 frame followed by 48000 frames — should NOT confirm at 0
    // because the followups disagree on sample rate.
    let mut bytes = build_mp3_frame_v1(128, 44_100, false);
    for _ in 0..6 {
      bytes.extend(build_mp3_frame_v1(128, 48_000, false));
    }
    // The 48000 run (5 of them) still confirms at its own offset; the key
    // is that the mixed boundary is not a valid 5-in-a-row of matching
    // frames starting at 0.
    let first = build_mp3_frame_v1(128, 44_100, false).len();
    assert_eq!(
      find_consecutive_mp3_headers(&bytes, MIN_CONFIRM_FRAMES).map(|(o, _)| o),
      Some(first)
    );
  }

  #[test]
  fn consecutive_returns_none_for_too_few() {
    let bytes = build_mp3_stream(2, 128, 44_100);
    assert!(find_consecutive_mp3_headers(&bytes, MIN_CONFIRM_FRAMES).is_none());
  }

  #[test]
  fn consecutive_num_one_is_first_header() {
    let bytes = build_mp3_frame_v1(128, 44_100, false);
    assert_eq!(find_consecutive_mp3_headers(&bytes, 1).map(|(o, _)| o), Some(0));
  }

  #[test]
  fn consecutive_skips_leading_id3_tag() {
    let mut bytes = vec![b'I', b'D', b'3', 3, 0, 0, 0, 0, 0, 0];
    bytes.extend(build_mp3_stream(6, 128, 44_100));
    // The tag (framesize 10) is skipped, frames confirm right after it.
    assert_eq!(
      find_consecutive_mp3_headers(&bytes, MIN_CONFIRM_FRAMES).map(|(o, _)| o),
      Some(10)
    );
  }

  #[test]
  fn probe_accepts_clean_stream() {
    let bytes = build_mp3_stream(10, 128, 44_100);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Mp3Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_layer2_stream() {
    let bytes = build_layer_stream(10, 1, 2, 128, 48_000);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Mp3Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_accepts_mp3_after_id3v2_header() {
    let mut bytes = id3v2::build_id3v2_tag(false, 64);
    bytes.extend(build_mp3_stream(10, 128, 44_100));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Mp3Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 1024]));
    assert!(!Mp3Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_layer1_emits_correct_codec() {
    let bytes = build_layer_stream(10, 1, 1, 256, 44_100);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp1", 0);
    Mp3Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_MPEG/L1");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("MP1"));
  }

  #[test]
  fn read_headers_populates_audio_track() {
    let bytes = build_mp3_stream(10, 128, 44_100);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp3", 0);
    Mp3Reader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Mp3);
    assert_eq!(out.tracks[0].codec.id, "A_MPEG/L3");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  #[test]
  fn read_headers_rejects_garbage() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 1024]));
    let mut out = MediaMetadata::new("x.mp3", 0);
    assert!(
      Mp3Reader
        .read_headers(&mut s, &Deadline::new(60_000), &mut out)
        .is_err()
    );
  }
}
