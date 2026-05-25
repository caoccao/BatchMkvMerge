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
//!
//! PARSER-164: the subtype field is decoded, not discarded.  `r_ogm.cpp:484-513`
//! dispatches video by FOURCC (AVC vs. MS-compatible VfW) and audio by the
//! hexadecimal WAVE format tag (PCM / MP3 / AC-3 / AAC); an unknown audio tag
//! makes mkvmerge drop the stream rather than emit a generic placeholder.

use crate::media_metadata::codec::fourcc;
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
    return sniff_video(packet, &subtype);
  }
  if stream_type == STREAM_TYPE_AUDIO {
    return sniff_audio(packet, &subtype);
  }
  if stream_type == STREAM_TYPE_TEXT {
    return Some(BitstreamMetadata::subtitle("S_OGM_TEXT", "OGM Text"));
  }
  None
}

/// `mtx::avc::is_avc_fourcc` (`avc/util.cpp:615-620`): case-insensitive `avc*`,
/// `h264`, or `x264`.
fn is_avc_fourcc(fourcc: &[u8]) -> bool {
  let lower: Vec<u8> = fourcc.iter().map(|b| b.to_ascii_lowercase()).collect();
  lower.starts_with(b"avc") || lower.starts_with(b"h264") || lower.starts_with(b"x264")
}

fn sniff_video(packet: &[u8], subtype: &[u8; 8]) -> Option<BitstreamMetadata> {
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

  // The first four subtype bytes are the video FOURCC.
  let fourcc_bytes = &subtype[..4];
  let (codec_id, codec_name) = if is_avc_fourcc(fourcc_bytes) {
    // ogm_v_avc_demuxer_c → V_MPEG4/ISO/AVC (r_ogm.cpp:487-488, 1326).
    ("V_MPEG4/ISO/AVC".to_string(), "AVC/H.264".to_string())
  } else {
    // ogm_v_mscomp_demuxer_c → VfW track keyed on the FOURCC
    // (r_ogm.cpp:490, 1357-1364).  Mirror the AVI reader's convention of
    // reporting the raw FOURCC as the codec id with a friendly name.
    let fourcc_str = fourcc_to_string(fourcc_bytes);
    let name = fourcc::lookup(&fourcc_str)
      .map(|e| e.name.to_string())
      .unwrap_or_else(|| fourcc_str.clone());
    (fourcc_str, name)
  };

  let mut metadata = BitstreamMetadata::video_only(codec_id, codec_name);
  // Both the VfW and AVC OGM video demuxers are ms_compat (the AVC demuxer
  // derives from the mscomp demuxer and inherits the flag) — PARSER-165.
  metadata.ms_compat = true;
  metadata.frame_duration_ns = frame_duration_ns;
  metadata.video = Some(VideoTrackProperties {
    pixel_dimensions: pixel,
    display_dimensions: pixel,
    default_duration_ns: frame_duration_ns,
    ..VideoTrackProperties::default()
  });
  Some(metadata)
}

fn sniff_audio(packet: &[u8], subtype: &[u8; 8]) -> Option<BitstreamMetadata> {
  // The first four subtype bytes are a hex string carrying the WAVE format
  // tag, e.g. "00ff" → 0x00ff (r_ogm.cpp:493-510).
  let tag_str = std::str::from_utf8(&subtype[..4]).ok()?;
  let format_tag = u32::from_str_radix(tag_str.trim_matches('\0'), 16).ok()?;
  let (codec_id, codec_name) = match format_tag {
    0x0001 => ("A_PCM/INT/LIT", "PCM"),
    0x0050 | 0x0055 => ("A_MPEG/L3", "MP3"),
    0x2000 => ("A_AC3", "AC-3"),
    0x00ff => ("A_AAC", "AAC"),
    // Unknown audio format tag — mkvmerge warns and ignores the stream
    // rather than emitting a generic OGM track.
    _ => return None,
  };

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
  let mut metadata = BitstreamMetadata::audio_only(codec_id, codec_name);
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
  Some(metadata)
}

/// Render a FOURCC's printable bytes as a string, dropping NULs.
fn fourcc_to_string(bytes: &[u8]) -> String {
  bytes.iter().filter(|&&b| b != 0).map(|&b| b as char).collect()
}

#[cfg(test)]
pub(crate) fn build_video_header_fourcc(fourcc: &[u8; 4], width: u32, height: u32, frame_duration_units: u64) -> Vec<u8> {
  let mut p = vec![0u8; 53];
  p[0] = 0x01;
  p[1..9].copy_from_slice(STREAM_TYPE_VIDEO);
  p[9..13].copy_from_slice(fourcc);
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
pub(crate) fn build_video_header(width: u32, height: u32, frame_duration_units: u64) -> Vec<u8> {
  build_video_header_fourcc(b"H264", width, height, frame_duration_units)
}

#[cfg(test)]
pub(crate) fn build_audio_header_tag(format_tag: &[u8; 4], sample_rate: u32, channels: u16, bps: u16) -> Vec<u8> {
  let mut p = vec![0u8; 49];
  p[0] = 0x01;
  p[1..9].copy_from_slice(STREAM_TYPE_AUDIO);
  p[9..13].copy_from_slice(format_tag); // subtype = format-tag hex string
  p[13..17].copy_from_slice(b"\0\0\0\0");
  p[17..21].copy_from_slice(&49u32.to_le_bytes());
  p[21..29].copy_from_slice(&10_000u64.to_le_bytes());
  p[29..37].copy_from_slice(&(sample_rate as u64).to_le_bytes());
  p[45..47].copy_from_slice(&channels.to_le_bytes());
  p[47..49].copy_from_slice(&bps.to_le_bytes());
  p
}

#[cfg(test)]
pub(crate) fn build_audio_header(sample_rate: u32, channels: u16, bps: u16) -> Vec<u8> {
  build_audio_header_tag(b"00ff", sample_rate, channels, bps)
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
  fn sniffs_ogm_avc_video() {
    // PARSER-164: an AVC FOURCC maps to the real AVC codec, not generic V_OGM.
    let pkt = build_video_header(1920, 1080, 416_667);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "V_MPEG4/ISO/AVC");
    assert_eq!(m.codec_name, "AVC/H.264");
    assert!(m.ms_compat);
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
  fn sniffs_ogm_vfw_video_by_fourcc() {
    // PARSER-164: a non-AVC FOURCC produces an MS-compatible VfW track keyed
    // on the FOURCC with a resolved codec name.
    let pkt = build_video_header_fourcc(b"XVID", 640, 480, 400_000);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "XVID");
    assert_eq!(m.codec_name, "Xvid");
    assert!(m.ms_compat);
  }

  #[test]
  fn unknown_vfw_fourcc_keeps_raw_name() {
    let pkt = build_video_header_fourcc(b"ZZZZ", 320, 240, 400_000);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "ZZZZ");
    assert_eq!(m.codec_name, "ZZZZ");
    assert!(m.ms_compat);
  }

  #[test]
  fn sniffs_ogm_aac_audio() {
    // PARSER-164: format tag 0x00ff → AAC.
    let pkt = build_audio_header(48000, 2, 16);
    let m = sniff(&pkt).unwrap();
    assert_eq!(m.codec_id, "A_AAC");
    assert!(!m.ms_compat);
    let a = m.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.bit_depth, Some(16));
  }

  #[test]
  fn sniffs_ogm_audio_format_tags() {
    // PARSER-164: PCM / MP3 / AC-3 format tags all map to real codecs.
    assert_eq!(sniff(&build_audio_header_tag(b"0001", 44100, 2, 16)).unwrap().codec_id, "A_PCM/INT/LIT");
    assert_eq!(sniff(&build_audio_header_tag(b"0050", 44100, 2, 0)).unwrap().codec_id, "A_MPEG/L3");
    assert_eq!(sniff(&build_audio_header_tag(b"0055", 44100, 2, 0)).unwrap().codec_id, "A_MPEG/L3");
    assert_eq!(sniff(&build_audio_header_tag(b"2000", 48000, 6, 0)).unwrap().codec_id, "A_AC3");
  }

  #[test]
  fn unknown_audio_format_tag_is_rejected() {
    // PARSER-164: an unknown WAVE format tag drops the stream entirely.
    let pkt = build_audio_header_tag(b"abcd", 48000, 2, 16);
    assert!(sniff(&pkt).is_none());
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
