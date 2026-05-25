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

//! OGM legacy stream headers (the pre-Theora / pre-Vorbis Ogg wrapper used
//! by the OGM tooling).  Layout:
//!
//! ```text
//! u8  0x01
//! 8   stream_type      ("video\0\0\0", "audio\0\0\0", "text\0\0\0\0")
//! 8   subtype          (FOURCC for video, hex format-tag string for audio)
//! u32 size             (LE — header size)
//! u64 time_unit        (LE — 100-ns units per frame)
//! u64 samples_per_unit (LE)
//! u32 default_len      (LE)
//! u32 buffer_size      (LE)
//! u16 bits_per_sample  (LE — audio only)
//! u16 padding
//! ...
//! ```

use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use super::BitstreamMetadata;

const MIN_LEN: usize = 1 + 8 + 8 + 4 + 8 + 8 + 4 + 4 + 4;

const STREAM_TYPE_VIDEO: &[u8; 8] = b"video\0\0\0";
const STREAM_TYPE_AUDIO: &[u8; 8] = b"audio\0\0\0";
const STREAM_TYPE_TEXT: &[u8; 8] = b"text\0\0\0\0";

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
  if packet.len() < MIN_LEN || packet[0] != 0x01 {
    return None;
  }
  let stream_type: &[u8; 8] = packet[1..9].try_into().ok()?;
  let subtype: [u8; 8] = packet[9..17].try_into().ok()?;
  if stream_type == STREAM_TYPE_VIDEO {
    let time_unit = u64::from_le_bytes(packet[21..29].try_into().ok()?);
    let samples_per_unit = u64::from_le_bytes(packet[29..37].try_into().ok()?);
    let frame_duration_ns = if time_unit > 0 && samples_per_unit > 0 {
      // time_unit is in 100-ns units, samples_per_unit is frames per
      // time_unit ⇒ ns/frame = time_unit * 100 / samples_per_unit
      Some((time_unit as u128 * 100 / samples_per_unit as u128) as u64)
    } else {
      None
    };
    // OGM video header continues with width/height at offset 45/49 (LE u32).
    let (width, height) = if packet.len() >= 53 {
      let w = u32::from_le_bytes(packet[45..49].try_into().ok()?);
      let h = u32::from_le_bytes(packet[49..53].try_into().ok()?);
      (w, h)
    } else {
      (0, 0)
    };
    let pixel = if width > 0 && height > 0 {
      Some(Dimensions2D { width, height })
    } else {
      None
    };
    let mut metadata = BitstreamMetadata::video_only("V_OGM", "OGM Video");
    metadata.frame_duration_ns = frame_duration_ns;
    metadata.video = Some(VideoTrackProperties {
      pixel_dimensions: pixel,
      display_dimensions: pixel,
      default_duration_ns: frame_duration_ns,
      ..VideoTrackProperties::default()
    });
    // FOURCC is in subtype; render lossily.
    let _ = subtype;
    return Some(metadata);
  }
  if stream_type == STREAM_TYPE_AUDIO {
    let sample_rate = u64::from_le_bytes(packet[29..37].try_into().ok()?);
    let channels = if packet.len() >= 47 {
      u16::from_le_bytes(packet[45..47].try_into().ok()?) as u32
    } else {
      0
    };
    let bits_per_sample = if packet.len() >= 49 {
      u16::from_le_bytes(packet[47..49].try_into().ok()?) as u32
    } else {
      0
    };
    let mut metadata = BitstreamMetadata::audio_only("A_OGM", "OGM Audio");
    metadata.audio = Some(AudioTrackProperties {
      channels: if channels == 0 { None } else { Some(channels) },
      sampling_frequency: if sample_rate == 0 {
        None
      } else {
        Some(sample_rate as f64)
      },
      bit_depth: if bits_per_sample == 0 {
        None
      } else {
        Some(bits_per_sample)
      },
      ..AudioTrackProperties::default()
    });
    return Some(metadata);
  }
  if stream_type == STREAM_TYPE_TEXT {
    return Some(BitstreamMetadata::subtitle("S_OGM_TEXT", "OGM Text"));
  }
  None
}

#[cfg(test)]
pub(crate) fn build_video_header(width: u32, height: u32, frame_duration_units: u64) -> Vec<u8> {
  let mut p = vec![0u8; 53];
  p[0] = 0x01;
  p[1..9].copy_from_slice(STREAM_TYPE_VIDEO);
  p[9..13].copy_from_slice(b"H264");
  // size (4 bytes), then time_unit/samples_per_unit (8 bytes each)
  let header_size: u32 = 53;
  p[17..21].copy_from_slice(&header_size.to_le_bytes());
  p[21..29].copy_from_slice(&frame_duration_units.to_le_bytes());
  p[29..37].copy_from_slice(&1u64.to_le_bytes()); // samples_per_unit
  p[45..49].copy_from_slice(&width.to_le_bytes());
  p[49..53].copy_from_slice(&height.to_le_bytes());
  p
}

#[cfg(test)]
pub(crate) fn build_audio_header(sample_rate: u32, channels: u16, bps: u16) -> Vec<u8> {
  let mut p = vec![0u8; 49];
  p[0] = 0x01;
  p[1..9].copy_from_slice(STREAM_TYPE_AUDIO);
  p[9..17].copy_from_slice(b"00FF\0\0\0\0"); // subtype = format-tag hex string
  p[17..21].copy_from_slice(&49u32.to_le_bytes());
  p[21..29].copy_from_slice(&10_000u64.to_le_bytes());
  p[29..37].copy_from_slice(&(sample_rate as u64).to_le_bytes());
  p[45..47].copy_from_slice(&channels.to_le_bytes());
  p[47..49].copy_from_slice(&bps.to_le_bytes());
  p
}

#[cfg(test)]
pub(crate) fn build_text_header() -> Vec<u8> {
  let mut p = vec![0u8; MIN_LEN];
  p[0] = 0x01;
  p[1..9].copy_from_slice(STREAM_TYPE_TEXT);
  p
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn sniffs_ogm_video() {
    let pkt = build_video_header(1920, 1080, 416_667);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "V_OGM");
    let v = m.video.unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    // 416_667 * 100 / 1 = 41_666_700 ns
    assert_eq!(v.default_duration_ns, Some(41_666_700));
  }

  #[test]
  fn sniffs_ogm_audio() {
    let pkt = build_audio_header(48000, 2, 16);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "A_OGM");
    let a = m.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.bit_depth, Some(16));
  }

  #[test]
  fn sniffs_ogm_text() {
    let pkt = build_text_header();
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "S_OGM_TEXT");
    assert!(m.video.is_none());
    assert!(m.audio.is_none());
  }

  #[test]
  fn rejects_non_ogm_stream_type() {
    let mut pkt = vec![0u8; MIN_LEN];
    pkt[0] = 0x01;
    pkt[1..9].copy_from_slice(b"junkjunk");
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn rejects_wrong_packet_type_byte() {
    let mut pkt = build_video_header(640, 480, 1000);
    pkt[0] = 0x02;
    assert!(sniff(&pkt).is_none());
  }

  #[test]
  fn rejects_short_packet() {
    assert!(sniff(&[0x01]).is_none());
  }

  #[test]
  fn video_header_zero_timebase_yields_no_duration() {
    let pkt = build_video_header(640, 480, 0);
    let m = sniff(&pkt).unwrap();
    assert!(m.frame_duration_ns.is_none());
  }
}
