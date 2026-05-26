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

//! Decoders for the per-track `type_specific_data` block in an `MDPR`
//! chunk.  Mirrors `real_video_props_t` / `real_audio_v4_props_t` /
//! `real_audio_v5_props_t` in `lib/librmff/librmff.h`.

pub const VIDEO_PROPS_FOURCC: [u8; 4] = *b"VIDO";
pub const AUDIO_PROPS_FOURCC: [u8; 4] = [b'.', b'r', b'a', 0xFD];

/// All fields big-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoProps {
  pub fourcc: [u8; 4],
  pub width: u16,
  pub height: u16,
  pub bpp: u16,
  pub fps_q16: u32,
}

impl VideoProps {
  pub const PROPS_LEN: usize = 32;

  /// Returns the FPS as a floating-point value (fixed-point Q16.16).
  pub fn fps(&self) -> f64 {
    let int_part = ((self.fps_q16 >> 16) & 0xFFFF) as f64;
    let frac_part = (self.fps_q16 & 0xFFFF) as f64;
    int_part + frac_part / 65536.0
  }

  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < Self::PROPS_LEN {
      return None;
    }
    // size (4 BE) | fourcc1 = "VIDO" (4 BE) | fourcc2 (4 BE)
    if bytes[4..8] != VIDEO_PROPS_FOURCC {
      return None;
    }
    Some(Self {
      fourcc: [bytes[8], bytes[9], bytes[10], bytes[11]],
      width: u16::from_be_bytes([bytes[12], bytes[13]]),
      height: u16::from_be_bytes([bytes[14], bytes[15]]),
      bpp: u16::from_be_bytes([bytes[16], bytes[17]]),
      fps_q16: u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioProps {
  pub fourcc: [u8; 4],
  pub version: u16,
  pub sample_rate: u32,
  pub sample_size: u16,
  pub channels: u16,
  pub extra_data: Vec<u8>,
}

impl AudioProps {
  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < 6 || bytes[..4] != AUDIO_PROPS_FOURCC {
      return None;
    }
    let version = u16::from_be_bytes([bytes[4], bytes[5]]);
    match version {
      3 => Some(Self {
        fourcc: *b"14_4",
        version: 3,
        sample_rate: 8_000,
        sample_size: 16,
        channels: 1,
        extra_data: Vec::new(),
      }),
      4 => parse_v4(bytes),
      5 => parse_v5(bytes),
      _ => None,
    }
  }
}

fn parse_v4(bytes: &[u8]) -> Option<AudioProps> {
  // real_audio_v4_props_t layout (packed):
  //   fourcc1(0..4) version1(4..6) unknown1(6..8) fourcc2(8..12)
  //   stream_length(12..16) version2(16..18) header_size(18..22) flavor(22..24)
  //   coded_frame_size(24..28) unknown3(28..32) unknown4(32..36) unknown5(36..40)
  //   sub_packet_h(40..42) frame_size(42..44) sub_packet_size(44..46) unknown6(46..48)
  //   sample_rate(48..50) unknown8(50..52) sample_size(52..54) channels(54..56)
  // Followed by two Pascal strings; second is the codec FOURCC.
  const PROPS_LEN: usize = 56;
  if bytes.len() < PROPS_LEN {
    return None;
  }
  let sample_rate = u16::from_be_bytes([bytes[48], bytes[49]]) as u32;
  let sample_size = u16::from_be_bytes([bytes[52], bytes[53]]);
  let channels = u16::from_be_bytes([bytes[54], bytes[55]]);

  // Walk the two Pascal strings — the second one's payload is the FOURCC.
  let mut p = PROPS_LEN;
  if p >= bytes.len() {
    return None;
  }
  let desc_len = bytes[p] as usize;
  p += 1 + desc_len;
  if p >= bytes.len() {
    return None;
  }
  let fourcc_len = bytes[p] as usize;
  p += 1;
  if fourcc_len != 4 || p + 4 > bytes.len() {
    return None;
  }
  let fourcc = [bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]];
  p += 4;

  Some(AudioProps {
    fourcc,
    version: 4,
    sample_rate,
    sample_size,
    channels,
    extra_data: bytes[p..].to_vec(),
  })
}

fn parse_v5(bytes: &[u8]) -> Option<AudioProps> {
  // real_audio_v5_props_t layout (packed):
  //   ... up through unknown6(46..48), then 6 bytes unknown7(48..54),
  //   sample_rate(54..56), unknown8(56..58), sample_size(58..60),
  //   channels(60..62), genr(62..66), fourcc3(66..70).
  const PROPS_LEN: usize = 70;
  if bytes.len() < PROPS_LEN {
    return None;
  }
  let sample_rate = u16::from_be_bytes([bytes[54], bytes[55]]) as u32;
  let sample_size = u16::from_be_bytes([bytes[58], bytes[59]]);
  let channels = u16::from_be_bytes([bytes[60], bytes[61]]);
  let fourcc = [bytes[66], bytes[67], bytes[68], bytes[69]];
  // PARSER-269: mkvtoolnix skips four bytes past the v5 props struct before
  // cloning the extra data — `extra_data = ts_data + 4 + sizeof(props)` guarded
  // by `(sizeof(real_audio_v5_props_t) + 4) < ts_size` (`r_real.cpp:216-217`).
  // The skipped field would otherwise be misread as the RAAC/RACP wrapper's
  // big-endian length prefix, breaking `apply_real_aac_config`.
  let extra_start = PROPS_LEN + 4;
  let extra_data = if bytes.len() > extra_start {
    bytes[extra_start..].to_vec()
  } else {
    Vec::new()
  };
  Some(AudioProps {
    fourcc,
    version: 5,
    sample_rate,
    sample_size,
    channels,
    extra_data,
  })
}

#[cfg(test)]
pub(crate) fn build_video_props(fourcc: &[u8; 4], width: u16, height: u16, fps: f64) -> Vec<u8> {
  let mut buf = vec![0u8; 32];
  buf[0..4].copy_from_slice(&(32u32).to_be_bytes()); // size
  buf[4..8].copy_from_slice(&VIDEO_PROPS_FOURCC);
  buf[8..12].copy_from_slice(fourcc);
  buf[12..14].copy_from_slice(&width.to_be_bytes());
  buf[14..16].copy_from_slice(&height.to_be_bytes());
  buf[16..18].copy_from_slice(&24u16.to_be_bytes()); // bpp
  let fps_q16 = ((fps.trunc() as u32) << 16) | (((fps.fract() * 65536.0).round() as u32) & 0xFFFF);
  buf[24..28].copy_from_slice(&fps_q16.to_be_bytes());
  buf
}

#[cfg(test)]
pub(crate) fn build_audio_v3() -> Vec<u8> {
  let mut buf = vec![0u8; 16];
  buf[0..4].copy_from_slice(&AUDIO_PROPS_FOURCC);
  buf[4..6].copy_from_slice(&3u16.to_be_bytes());
  buf
}

#[cfg(test)]
pub(crate) fn build_audio_v4(sample_rate: u16, channels: u16, sample_size: u16, fourcc: &[u8; 4]) -> Vec<u8> {
  let mut buf = vec![0u8; 56];
  buf[0..4].copy_from_slice(&AUDIO_PROPS_FOURCC);
  buf[4..6].copy_from_slice(&4u16.to_be_bytes());
  buf[48..50].copy_from_slice(&sample_rate.to_be_bytes());
  buf[52..54].copy_from_slice(&sample_size.to_be_bytes());
  buf[54..56].copy_from_slice(&channels.to_be_bytes());
  // 4-byte description string + 4-byte fourcc string
  buf.push(4);
  buf.extend_from_slice(b"Int4");
  buf.push(4);
  buf.extend_from_slice(fourcc);
  buf
}

#[cfg(test)]
pub(crate) fn build_audio_v5(sample_rate: u16, channels: u16, sample_size: u16, fourcc: &[u8; 4]) -> Vec<u8> {
  let mut buf = vec![0u8; 70];
  buf[0..4].copy_from_slice(&AUDIO_PROPS_FOURCC);
  buf[4..6].copy_from_slice(&5u16.to_be_bytes());
  buf[54..56].copy_from_slice(&sample_rate.to_be_bytes());
  buf[58..60].copy_from_slice(&sample_size.to_be_bytes());
  buf[60..62].copy_from_slice(&channels.to_be_bytes());
  buf[62..66].copy_from_slice(b"genr");
  buf[66..70].copy_from_slice(fourcc);
  buf
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_video_props_with_fps_quarter() {
    let bytes = build_video_props(b"RV40", 1280, 720, 30.0);
    let v = VideoProps::parse(&bytes).unwrap();
    assert_eq!(v.fourcc, *b"RV40");
    assert_eq!(v.width, 1280);
    assert_eq!(v.height, 720);
    assert!((v.fps() - 30.0).abs() < 1e-6);
  }

  #[test]
  fn parses_video_props_fractional_fps() {
    let bytes = build_video_props(b"RV30", 320, 240, 23.976);
    let v = VideoProps::parse(&bytes).unwrap();
    assert!((v.fps() - 23.976).abs() < 1e-3);
  }

  #[test]
  fn video_props_rejects_short_input() {
    assert!(VideoProps::parse(&[0u8; 8]).is_none());
  }

  #[test]
  fn video_props_rejects_wrong_signature() {
    let mut bytes = build_video_props(b"RV40", 320, 240, 25.0);
    bytes[4] = b'X';
    assert!(VideoProps::parse(&bytes).is_none());
  }

  #[test]
  fn audio_props_v3_hardcodes_14_4_codec() {
    let bytes = build_audio_v3();
    let a = AudioProps::parse(&bytes).unwrap();
    assert_eq!(a.version, 3);
    assert_eq!(a.fourcc, *b"14_4");
    assert_eq!(a.sample_rate, 8_000);
    assert_eq!(a.channels, 1);
    assert_eq!(a.sample_size, 16);
  }

  #[test]
  fn audio_props_v4_decodes_sample_rate_and_fourcc() {
    let bytes = build_audio_v4(44_100, 2, 16, b"cook");
    let a = AudioProps::parse(&bytes).unwrap();
    assert_eq!(a.version, 4);
    assert_eq!(a.fourcc, *b"cook");
    assert_eq!(a.sample_rate, 44_100);
    assert_eq!(a.channels, 2);
    assert_eq!(a.sample_size, 16);
    assert!(a.extra_data.is_empty());
  }

  #[test]
  fn audio_props_v4_rejects_unexpected_fourcc_length() {
    // The fourcc-length byte sits right after the description string.
    // build_audio_v4 lays out: bytes[..56] packed struct, [56] desc_len,
    // [57..61] desc, [61] fourcc_len, [62..66] fourcc.
    let mut bytes = build_audio_v4(22_050, 1, 16, b"cook");
    bytes[61] = 5; // fourcc_len wrong (must be 4)
    assert!(AudioProps::parse(&bytes).is_none());
  }

  #[test]
  fn audio_props_v5_decodes_fourcc3() {
    let mut bytes = build_audio_v5(48_000, 6, 16, b"raac");
    // PARSER-269: extra data begins four bytes past the v5 props struct.
    bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // skipped 4 bytes
    bytes.extend_from_slice(&[0x12, 0x10]);
    let a = AudioProps::parse(&bytes).unwrap();
    assert_eq!(a.version, 5);
    assert_eq!(a.fourcc, *b"raac");
    assert_eq!(a.sample_rate, 48_000);
    assert_eq!(a.channels, 6);
    // The four skipped bytes must not leak into extra_data.
    assert_eq!(a.extra_data, vec![0x12, 0x10]);
  }

  #[test]
  fn audio_props_v5_extra_data_empty_when_only_skip_bytes_present() {
    // PARSER-269: when only the 4 skipped bytes follow the props struct (no
    // trailing payload), extra_data is empty — mirroring the strict
    // `(sizeof(props) + 4) < ts_size` guard in r_real.cpp:216.
    let mut bytes = build_audio_v5(48_000, 2, 16, b"raac");
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    let a = AudioProps::parse(&bytes).unwrap();
    assert!(a.extra_data.is_empty());
  }

  #[test]
  fn audio_props_rejects_wrong_signature() {
    let mut bytes = build_audio_v3();
    bytes[0] = b'X';
    assert!(AudioProps::parse(&bytes).is_none());
  }

  #[test]
  fn audio_props_rejects_unknown_version() {
    let mut bytes = vec![0u8; 16];
    bytes[0..4].copy_from_slice(&AUDIO_PROPS_FOURCC);
    bytes[4..6].copy_from_slice(&99u16.to_be_bytes());
    assert!(AudioProps::parse(&bytes).is_none());
  }
}
