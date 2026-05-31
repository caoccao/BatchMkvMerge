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

//! FLV tag header + audio/video tag classification.  Port of the
//! `flv_tag_c` / `process_audio_tag` / `process_video_tag` paths in
//! `r_flv.cpp`.

pub const TAG_AUDIO: u8 = 0x08;
pub const TAG_VIDEO: u8 = 0x09;
pub const TAG_SCRIPT: u8 = 0x12;
pub const TAG_HEADER_LEN: usize = 11;
pub const PREVIOUS_TAG_SIZE_LEN: usize = 4;

/// `flv_tag_c::read` — reads the 4-byte previous-tag-size word, then the
/// 11-byte tag header.  We return the data-area start offset relative to
/// the start of the previous-tag-size word.
/// Flag bit marking a tag as encrypted (filter applied) — mirrors
/// `flv_tag_c::is_encrypted` (`r_flv.cpp:93-97`), which tests `m_flags & 0x20`.
pub const TAG_FLAG_ENCRYPTED: u8 = 0x20;

#[derive(Debug, Clone, Copy)]
pub struct FlvTagHeader {
  pub tag_type: u8,
  pub encrypted: bool,
  pub data_size: u32,
  pub timestamp_ms: u32,
}

impl FlvTagHeader {
  pub const TOTAL_LEN: usize = PREVIOUS_TAG_SIZE_LEN + TAG_HEADER_LEN;

  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < Self::TOTAL_LEN {
      return None;
    }
    let off = PREVIOUS_TAG_SIZE_LEN;
    let flags = bytes[off];
    let data_size = u32::from_be_bytes([0, bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
    let ts = u32::from_be_bytes([0, bytes[off + 4], bytes[off + 5], bytes[off + 6]]);
    let ts_ext = bytes[off + 7] as u32;
    // 3-byte stream id at off+8..off+11 — ignored.
    Some(Self {
      tag_type: flags,
      encrypted: (flags & TAG_FLAG_ENCRYPTED) == TAG_FLAG_ENCRYPTED,
      data_size,
      timestamp_ms: (ts_ext << 24) | ts,
    })
  }

  /// `flv_tag_c::is_encrypted` (`r_flv.cpp:93-97`).  mkvtoolnix's
  /// `process_tag` (`:799-800`) returns early for encrypted tags so they
  /// are never decoded as audio/video.
  pub fn is_encrypted(&self) -> bool {
    self.encrypted
  }
  pub fn is_audio(&self) -> bool {
    self.tag_type == TAG_AUDIO
  }
  pub fn is_video(&self) -> bool {
    self.tag_type == TAG_VIDEO
  }
  pub fn is_script(&self) -> bool {
    self.tag_type == TAG_SCRIPT
  }
}

/// Decoded first byte of an audio tag payload.
///
/// `audiotag_header & 0xF0 >> 4` — sound format (10 = AAC, 2 = MP3, 14 = MP3 8 kHz).
/// `audiotag_header & 0x0C >> 2` — sample rate index (5512 / 11025 / 22050 / 44100).
/// `audiotag_header & 0x02 >> 1` — sample size (0 = 8, 1 = 16 bits).
/// `audiotag_header & 0x01     ` — channel type (0 = mono, 1 = stereo).
#[derive(Debug, Clone, Copy)]
pub struct AudioTagFlags {
  pub format: u8,
  pub rate_index: u8,
  pub size_index: u8,
  pub type_index: u8,
}

impl AudioTagFlags {
  pub fn parse(byte: u8) -> Self {
    Self {
      format: (byte & 0xF0) >> 4,
      rate_index: (byte & 0x0C) >> 2,
      size_index: (byte & 0x02) >> 1,
      type_index: byte & 0x01,
    }
  }

  pub fn channels(&self) -> u32 {
    if self.type_index == 0 { 1 } else { 2 }
  }

  pub fn sample_rate(&self) -> Option<u32> {
    match self.rate_index {
      0 => Some(5_512),
      1 => Some(11_025),
      2 => Some(22_050),
      3 => Some(44_100),
      _ => None,
    }
  }

  pub fn bits_per_sample(&self) -> u8 {
    if self.size_index == 0 { 8 } else { 16 }
  }

  /// FOURCC + display name for the format byte's codec.
  pub fn codec(&self) -> Option<(&'static str, &'static str)> {
    match self.format {
      0 => Some(("PCMP", "Linear PCM (platform endian)")),
      1 => Some(("ADPC", "ADPCM")),
      2 | 14 => Some(("MP3 ", "MP3")),
      3 => Some(("LPCM", "Linear PCM (little-endian)")),
      4 => Some(("NEL1", "Nellymoser 16 kHz mono")),
      5 => Some(("NEL5", "Nellymoser 8 kHz mono")),
      6 => Some(("NELL", "Nellymoser")),
      7 => Some(("ALAW", "G.711 A-law")),
      8 => Some(("ULAW", "G.711 µ-law")),
      10 => Some(("AAC ", "AAC")),
      11 => Some(("SPEX", "Speex")),
      13 => Some(("DEV ", "Device-specific")),
      _ => None,
    }
  }
}

/// Video codec id (lower 4 bits of the first byte of a video tag payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodecId {
  SorensonH263,  // 2
  ScreenVideo,   // 3
  Vp6,           // 4
  Vp6Alpha,      // 5
  ScreenVideoV2, // 6
  H264,          // 7
  H265,          // 12 (Adobe-extended)
}

impl VideoCodecId {
  pub fn from_byte(b: u8) -> Option<Self> {
    match b {
      2 => Some(Self::SorensonH263),
      3 => Some(Self::ScreenVideo),
      4 => Some(Self::Vp6),
      5 => Some(Self::Vp6Alpha),
      6 => Some(Self::ScreenVideoV2),
      7 => Some(Self::H264),
      12 => Some(Self::H265),
      _ => None,
    }
  }

  /// FourCC assigned by mkvtoolnix's `process_video_tag` paths.  Only
  /// H.264 (`process_video_tag_avc` `:593`), H.265 (`process_video_tag_hevc`
  /// `:630`), and the generic codecs Sorenson H.263 / VP6 / VP6-alpha
  /// (`process_video_tag_generic` `:667-669`) receive a FourCC.  Screen
  /// Video and Screen Video v2 fall into the `else { m_headers_read = true; }`
  /// branch (`:740-741`) and get NO FourCC, so `flv_track_c::is_valid`
  /// (`:173-177`) rejects them and they are erased (`:282`).
  pub fn fourcc(self) -> Option<&'static str> {
    match self {
      Self::SorensonH263 => Some("FLV1"),
      Self::Vp6 => Some("VP6F"),
      Self::Vp6Alpha => Some("VP6A"),
      Self::H264 => Some("AVC1"),
      Self::H265 => Some("HVC1"),
      Self::ScreenVideo | Self::ScreenVideoV2 => None,
    }
  }

  pub fn display_name(self) -> &'static str {
    match self {
      Self::SorensonH263 => "Sorenson H.263 (Flash version)",
      Self::ScreenVideo => "Screen Video",
      Self::Vp6 => "On2 VP6 (Flash version)",
      Self::Vp6Alpha => "On2 VP6 (Flash version with alpha channel)",
      Self::ScreenVideoV2 => "Screen Video v2",
      Self::H264 => "AVC/H.264",
      Self::H265 => "HEVC/H.265/MPEG-H",
    }
  }

  pub fn codec_id(self) -> &'static str {
    match self {
      Self::H264 => "V_MPEG4/ISO/AVC",
      Self::H265 => "V_MPEGH/ISO/HEVC",
      Self::Vp6 => "V_VP6F",
      Self::Vp6Alpha => "V_VP6A",
      Self::SorensonH263 => "V_FLV1",
      Self::ScreenVideo => "V_FSV1",
      Self::ScreenVideoV2 => "V_FSV2",
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn build_tag_header(tag_type: u8, data_size: u32, timestamp: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(FlvTagHeader::TOTAL_LEN);
    buf.extend_from_slice(&0u32.to_be_bytes()); // previous_tag_size
    buf.push(tag_type);
    // data_size: 3 bytes BE
    buf.push(((data_size >> 16) & 0xFF) as u8);
    buf.push(((data_size >> 8) & 0xFF) as u8);
    buf.push((data_size & 0xFF) as u8);
    // timestamp: 3 bytes BE
    buf.push(((timestamp >> 16) & 0xFF) as u8);
    buf.push(((timestamp >> 8) & 0xFF) as u8);
    buf.push((timestamp & 0xFF) as u8);
    // timestamp_ext
    buf.push((timestamp >> 24) as u8);
    // stream id (3 bytes, always 0)
    buf.extend_from_slice(&[0u8; 3]);
    buf
  }

  #[test]
  fn flv_tag_header_decodes_audio_tag() {
    let bytes = build_tag_header(TAG_AUDIO, 128, 1234);
    let h = FlvTagHeader::parse(&bytes).unwrap();
    assert!(h.is_audio());
    assert_eq!(h.data_size, 128);
    assert_eq!(h.timestamp_ms, 1234);
  }

  #[test]
  fn flv_tag_header_decodes_video_tag() {
    let bytes = build_tag_header(TAG_VIDEO, 4096, 50);
    let h = FlvTagHeader::parse(&bytes).unwrap();
    assert!(h.is_video());
    assert_eq!(h.data_size, 4096);
  }

  #[test]
  fn flv_tag_header_decodes_script_tag() {
    let bytes = build_tag_header(TAG_SCRIPT, 200, 0);
    let h = FlvTagHeader::parse(&bytes).unwrap();
    assert!(h.is_script());
  }

  #[test]
  fn flv_tag_header_rejects_short_input() {
    assert!(FlvTagHeader::parse(&[0u8; 8]).is_none());
  }

  #[test]
  fn flv_tag_header_decodes_extended_timestamp() {
    // 32-bit timestamp split across the 24-bit ts + 8-bit ts_ext fields.
    let bytes = build_tag_header(TAG_AUDIO, 0, 0x01_23_45_67);
    let h = FlvTagHeader::parse(&bytes).unwrap();
    assert_eq!(h.timestamp_ms, 0x01_23_45_67);
  }

  #[test]
  fn audio_tag_flags_decode_aac_44k_stereo_16bit() {
    // format=10 (AAC), rate=3 (44.1k), size=1 (16-bit), type=1 (stereo)
    let byte = (10 << 4) | (3 << 2) | (1 << 1) | 1;
    let f = AudioTagFlags::parse(byte);
    assert_eq!(f.format, 10);
    assert_eq!(f.sample_rate(), Some(44_100));
    assert_eq!(f.channels(), 2);
    assert_eq!(f.bits_per_sample(), 16);
    let (fourcc, _) = f.codec().unwrap();
    assert_eq!(fourcc, "AAC ");
  }

  #[test]
  fn audio_tag_flags_decode_mp3_22k_mono_8bit() {
    // format=2 (MP3), rate=2 (22.05k), size=0 (8-bit), type=0 (mono)
    let byte = (2 << 4) | (2 << 2);
    let f = AudioTagFlags::parse(byte);
    assert_eq!(f.format, 2);
    assert_eq!(f.sample_rate(), Some(22_050));
    assert_eq!(f.channels(), 1);
    assert_eq!(f.bits_per_sample(), 8);
    let (fourcc, _) = f.codec().unwrap();
    assert_eq!(fourcc, "MP3 ");
  }

  #[test]
  fn audio_tag_flags_codec_returns_none_for_unknown_format() {
    let byte = 12 << 4; // format 12 — reserved
    let f = AudioTagFlags::parse(byte);
    assert!(f.codec().is_none());
  }

  #[test]
  fn audio_tag_flags_rate_table_covers_all_indices() {
    assert_eq!(AudioTagFlags::parse(0).sample_rate(), Some(5_512));
    assert_eq!(AudioTagFlags::parse(1 << 2).sample_rate(), Some(11_025));
    assert_eq!(AudioTagFlags::parse(2 << 2).sample_rate(), Some(22_050));
    assert_eq!(AudioTagFlags::parse(3 << 2).sample_rate(), Some(44_100));
  }

  #[test]
  fn video_codec_id_from_byte_recognises_documented_codecs() {
    assert_eq!(VideoCodecId::from_byte(2), Some(VideoCodecId::SorensonH263));
    assert_eq!(VideoCodecId::from_byte(4), Some(VideoCodecId::Vp6));
    assert_eq!(VideoCodecId::from_byte(7), Some(VideoCodecId::H264));
    assert_eq!(VideoCodecId::from_byte(12), Some(VideoCodecId::H265));
    assert_eq!(VideoCodecId::from_byte(0), None);
  }

  #[test]
  fn video_codec_id_codec_ids_match_matroska_convention() {
    assert_eq!(VideoCodecId::H264.codec_id(), "V_MPEG4/ISO/AVC");
    assert_eq!(VideoCodecId::H265.codec_id(), "V_MPEGH/ISO/HEVC");
    assert_eq!(VideoCodecId::Vp6.codec_id(), "V_VP6F");
  }

  #[test]
  fn video_codec_id_screen_video_has_no_fourcc() {
    // mkvtoolnix only assigns a FourCC to H.264 / H.265 / Sorenson H.263 /
    // VP6 / VP6-alpha (`r_flv.cpp:589-684`).  Screen Video variants get NO
    // FourCC, so `flv_track_c::is_valid` (`:173-177`) rejects them.
    assert_eq!(VideoCodecId::ScreenVideo.fourcc(), None);
    assert_eq!(VideoCodecId::ScreenVideoV2.fourcc(), None);
  }

  #[test]
  fn video_codec_id_fourcc_only_for_supported_codecs() {
    assert_eq!(VideoCodecId::SorensonH263.fourcc(), Some("FLV1"));
    assert_eq!(VideoCodecId::Vp6.fourcc(), Some("VP6F"));
    assert_eq!(VideoCodecId::Vp6Alpha.fourcc(), Some("VP6A"));
    assert_eq!(VideoCodecId::H264.fourcc(), Some("AVC1"));
    assert_eq!(VideoCodecId::H265.fourcc(), Some("HVC1"));
  }

  #[test]
  fn video_codec_id_recognises_screen_and_vp6_alpha_codecs() {
    assert_eq!(VideoCodecId::from_byte(3), Some(VideoCodecId::ScreenVideo));
    assert_eq!(VideoCodecId::from_byte(5), Some(VideoCodecId::Vp6Alpha));
    assert_eq!(VideoCodecId::from_byte(6), Some(VideoCodecId::ScreenVideoV2));
  }

  #[test]
  fn video_codec_id_display_names_cover_every_variant() {
    for v in [
      VideoCodecId::SorensonH263,
      VideoCodecId::ScreenVideo,
      VideoCodecId::Vp6,
      VideoCodecId::Vp6Alpha,
      VideoCodecId::ScreenVideoV2,
      VideoCodecId::H264,
      VideoCodecId::H265,
    ] {
      assert!(!v.display_name().is_empty());
      assert!(!v.codec_id().is_empty());
      // Only the FourCC-bearing codecs (everything except the Screen Video
      // variants) yield a non-empty FourCC.
      match v {
        VideoCodecId::ScreenVideo | VideoCodecId::ScreenVideoV2 => assert!(v.fourcc().is_none()),
        _ => assert!(v.fourcc().is_some_and(|f| !f.is_empty())),
      }
    }
  }

  #[test]
  fn audio_tag_flags_codec_table_covers_documented_formats() {
    // Exercise the remaining branches of `AudioTagFlags::codec` so the
    // table doesn't bit-rot when a new entry is added.
    for format in 0u8..=14 {
      let byte = format << 4;
      let f = AudioTagFlags::parse(byte);
      if matches!(format, 9 | 12) {
        // Reserved / device-specific without a public name in
        // mkvtoolnix's table.
        continue;
      }
      // Every documented index produces a codec record.
      assert!(f.codec().is_some(), "format {format}");
    }
  }

  #[test]
  fn audio_tag_flags_size_index_drives_bits_per_sample() {
    let f = AudioTagFlags::parse(0);
    assert_eq!(f.bits_per_sample(), 8);
    let f = AudioTagFlags::parse(0b10);
    assert_eq!(f.bits_per_sample(), 16);
  }

  #[test]
  fn flv_tag_header_rejects_clear_tag_with_reserved_high_bits() {
    // mkvtoolnix compares the full tag flags for non-encrypted tags; reserved
    // high bits must not be masked into a real audio tag.
    let mut bytes = build_tag_header(TAG_AUDIO, 16, 0);
    bytes[4] |= 0x40; // reserved high bit, not encrypted
    let h = FlvTagHeader::parse(&bytes).unwrap();
    assert!(!h.is_audio());
  }

  #[test]
  fn flv_tag_header_detects_encrypted_flag() {
    // A clear tag has the encryption bit unset.
    let clear = FlvTagHeader::parse(&build_tag_header(TAG_AUDIO, 16, 0)).unwrap();
    assert!(!clear.is_encrypted());
    // Setting bit 0x20 (filter/encryption) flags the tag as encrypted
    // (`flv_tag_c::is_encrypted`, `r_flv.cpp:93-97`).
    let mut bytes = build_tag_header(TAG_VIDEO, 16, 0);
    bytes[4] |= TAG_FLAG_ENCRYPTED;
    let enc = FlvTagHeader::parse(&bytes).unwrap();
    assert!(enc.is_encrypted());
    // Classification remains exact; readers skip encrypted tags before
    // checking audio/video/script kind.
    assert!(!enc.is_video());
  }
}
