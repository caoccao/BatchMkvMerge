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

//! Registration descriptor (tag 0x05) — ISO/IEC 13818-1 §2.6.8.
//!
//! Body layout: 4-byte FourCC `format_identifier` followed by optional
//! private bytes.  mkvtoolnix's `parse_registration_pmt_descriptor`
//! (`r_mpeg_ts.cpp:907-940`) treats the FourCC `"HDMV"` as a Blu-ray HDMV
//! registration and uses the trailing bytes to derive the underlying codec;
//! every other FourCC is looked up directly as a codec FourCC.  PARSER-090.

use crate::media_metadata::codec::TrackKind;

/// Decoded registration descriptor.  `format_identifier` is the raw 4-byte
/// FourCC as ASCII (e.g. `"HDMV"`, `"VC-1"`, `"AC-3"`).  When the FourCC is
/// `"HDMV"`, [`hdmv_stream_coding_type`] carries the 8-bit
/// `stream_coding_type` byte from the descriptor body (offset 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationDescriptor {
  pub format_identifier: String,
  pub hdmv_stream_coding_type: Option<u8>,
}

pub fn decode(body: &[u8]) -> Option<RegistrationDescriptor> {
  if body.len() < 4 {
    return None;
  }
  let format_identifier = String::from_utf8_lossy(&body[..4]).into_owned();
  // The Blu-ray HDMV registration is structured: 4-byte FourCC ("HDMV") +
  // 1 stuffing byte (must be 0xFF) + 1 stream_coding_type byte + flags.
  // See `determine_codec_for_hdmv_registration_descriptor` for the layout.
  let hdmv_stream_coding_type = if format_identifier == "HDMV" && body.len() >= 8 && body[4] == 0xFF {
    Some(body[5])
  } else {
    None
  };
  Some(RegistrationDescriptor {
    format_identifier,
    hdmv_stream_coding_type,
  })
}

/// Translate a Blu-ray HDMV `stream_coding_type` byte into the canonical
/// Matroska codec id used by the rest of the parser.  Mirrors
/// `codec_c::look_up_bluray_stream_coding_type` in
/// `mkvtoolnix/src/common/codec.cpp`.
pub fn hdmv_codec(stream_coding_type: u8) -> Option<(&'static str, &'static str, TrackKind)> {
  Some(match stream_coding_type {
    0x02 => ("V_MPEG12", "MPEG-1/2", TrackKind::Video),
    0x1B => ("V_MPEG4/ISO/AVC", "AVC/H.264/MPEG-4p10", TrackKind::Video),
    0x24 => ("V_MPEGH/ISO/HEVC", "HEVC/H.265/MPEG-H", TrackKind::Video),
    0xEA => ("V_VC1", "VC-1", TrackKind::Video),
    0x80 => ("A_PCM", "LPCM", TrackKind::Audio),
    0x81 => ("A_AC3", "AC-3", TrackKind::Audio),
    0x82 | 0x85 | 0x86 => ("A_DTS", "DTS", TrackKind::Audio),
    0x83 => ("A_TRUEHD", "TrueHD", TrackKind::Audio),
    0x84 | 0x87 => ("A_EAC3", "E-AC-3", TrackKind::Audio),
    0x90 => ("S_HDMV/PGS", "HDMV PGS", TrackKind::Subtitle),
    0x92 => ("S_HDMV/TEXTST", "HDMV Text Subtitles", TrackKind::Subtitle),
    _ => return None,
  })
}

/// Map a generic registration FourCC to a Matroska codec id when mkvtoolnix
/// would (the FourCC equals one of the codec FourCCs `codec_c::look_up`
/// recognises).  Returns `None` for unknown FourCCs.
pub fn codec_for_fourcc(fourcc: &str) -> Option<(&'static str, &'static str, TrackKind)> {
  let key = normalise_fourcc(fourcc)?;
  Some(match key.as_str() {
    "AV01" => ("V_AV1", "AV1", TrackKind::Video),
    "CVID" => ("V_CINEPAK", "Cinepak", TrackKind::Video),
    "DRAC" => ("V_DIRAC", "Dirac", TrackKind::Video),
    "APCH" | "APCN" | "APCS" | "APCO" | "AP4H" => ("V_PRORES", "ProRes", TrackKind::Video),
    "SVQI" | "SVQ1" => ("V_SVQ1", "Sorenson v1", TrackKind::Video),
    "SVQ3" => ("V_SVQ3", "Sorenson v3", TrackKind::Video),
    "THEO" | "THRA" => ("V_THEORA", "Theora", TrackKind::Video),
    "VC-1" | "WVC1" => ("V_VC1", "VC-1", TrackKind::Video),
    "VP80" => ("V_VP8", "VP8", TrackKind::Video),
    "VP90" | "VP09" => ("V_VP9", "VP9", TrackKind::Video),
    "MP4A" | "RAAC" | "RACP" => ("A_AAC", "AAC", TrackKind::Audio),
    "ALAC" => ("A_ALAC", "ALAC", TrackKind::Audio),
    "ATRC" => ("A_REAL/ATRC", "ATRAC3", TrackKind::Audio),
    "COOK" => ("A_REAL/COOK", "G2/Cook", TrackKind::Audio),
    "28_8" => ("A_REAL/28_8", "LD-CELP", TrackKind::Audio),
    "FLAC" => ("A_FLAC", "FLAC", TrackKind::Audio),
    "MLP " => ("A_MLP", "MLP", TrackKind::Audio),
    "MP2A" => ("A_MPEG/L2", "MP2", TrackKind::Audio),
    "LAME" | "MPGA" => ("A_MPEG/L3", "MP3", TrackKind::Audio),
    "OPUS" => ("A_OPUS", "Opus", TrackKind::Audio),
    "TWOS" => ("A_PCM/INT/BIG", "PCM", TrackKind::Audio),
    "SOWT" | "RAW " | "LPCM" | "IN24" => ("A_PCM/INT/LIT", "PCM", TrackKind::Audio),
    "QDM2" => ("A_QUICKTIME/QDM2", "QDMC", TrackKind::Audio),
    "RALF" => ("A_REAL/RALF", "RealAudio-Lossless", TrackKind::Audio),
    "SIPR" => ("A_REAL/SIPR", "Sipro/ACELP-NET", TrackKind::Audio),
    "TTA1" => ("A_TTA1", "TrueAudio", TrackKind::Audio),
    "TRHD" | "MLPA" => ("A_TRUEHD", "TrueHD", TrackKind::Audio),
    "LPCJ" | "14_4" => ("A_REAL/14_4", "VSELP", TrackKind::Audio),
    "VORB" => ("A_VORBIS", "Vorbis", TrackKind::Audio),
    "WVPK" => ("A_WAVPACK4", "WavPack4", TrackKind::Audio),
    "KATE" => ("S_KATE", "Kate", TrackKind::Subtitle),
    "TX3G" => ("S_TX3G", "Timed Text", TrackKind::Subtitle),
    "USF " => ("S_TEXT/USF", "UniversalSubtitleFormat", TrackKind::Subtitle),
    key if is_avc_fourcc(key) => ("V_MPEG4/ISO/AVC", "AVC/H.264/MPEG-4p10", TrackKind::Video),
    key if is_hevc_fourcc(key) => ("V_MPEGH/ISO/HEVC", "HEVC/H.265/MPEG-H", TrackKind::Video),
    key if is_mpeg12_fourcc(key) => ("V_MPEG12", "MPEG-1/2", TrackKind::Video),
    key if is_mpeg4_part2_fourcc(key) => ("V_MPEG4/ISO/ASP", "MPEG-4p2", TrackKind::Video),
    key if is_realvideo_fourcc(key) => realvideo_codec(key),
    key if is_aac_fourcc(key) => ("A_AAC", "AAC", TrackKind::Audio),
    key if is_ac3_fourcc(key) => ("A_AC3", "AC-3", TrackKind::Audio),
    key if is_dts_fourcc(key) => ("A_DTS", "DTS", TrackKind::Audio),
    key if is_mp2_fourcc(key) => ("A_MPEG/L2", "MP2", TrackKind::Audio),
    key if is_mp3_fourcc(key) => ("A_MPEG/L3", "MP3", TrackKind::Audio),
    key if is_pcm_fourcc(key) => ("A_PCM/INT/LIT", "PCM", TrackKind::Audio),
    _ => return None,
  })
}

fn normalise_fourcc(fourcc: &str) -> Option<String> {
  let cleaned: String = fourcc.chars().filter(|c| !c.is_ascii_control()).collect();
  if cleaned.len() == 4 {
    Some(cleaned.to_ascii_uppercase())
  } else {
    None
  }
}

fn is_avc_fourcc(key: &str) -> bool {
  key.starts_with("AVC") || matches!(key, "H264" | "X264")
}

fn is_hevc_fourcc(key: &str) -> bool {
  matches!(key, "HEVC" | "HVC1" | "HEV1" | "H265" | "X265" | "DVH1" | "DVHE")
}

fn is_mpeg12_fourcc(key: &str) -> bool {
  let b = key.as_bytes();
  matches!(key, "MPEG" | "MPG1" | "MPG2" | "MPGV" | "MP1V" | "MP2V" | "H262")
    || (b.len() == 4 && b[0] == b'M' && matches!(b[1], b'1' | b'2') && b[2] == b'V')
}

fn is_mpeg4_part2_fourcc(key: &str) -> bool {
  matches!(key, "3IV2" | "XVID" | "XVIX" | "DIVX" | "DX50" | "FMP4" | "MP4V")
}

fn is_realvideo_fourcc(key: &str) -> bool {
  let b = key.as_bytes();
  b.len() == 4 && b[0] == b'R' && b[1] == b'V' && matches!(b[2], b'1' | b'2' | b'3' | b'4') && b[3].is_ascii_digit()
}

fn realvideo_codec(key: &str) -> (&'static str, &'static str, TrackKind) {
  match key.as_bytes()[2] {
    b'1' => ("V_REAL/RV10", "RealVideo", TrackKind::Video),
    b'2' => ("V_REAL/RV20", "RealVideo", TrackKind::Video),
    b'3' => ("V_REAL/RV30", "RealVideo", TrackKind::Video),
    _ => ("V_REAL/RV40", "RealVideo", TrackKind::Video),
  }
}

fn is_aac_fourcc(key: &str) -> bool {
  key.starts_with("AAC")
}

fn is_ac3_fourcc(key: &str) -> bool {
  matches!(key, "AC-3" | "SAC3" | "EAC3" | "EC-3" | "A52 " | "A52B" | "DNET") || key.starts_with("AC3")
}

fn is_dts_fourcc(key: &str) -> bool {
  let b = key.as_bytes();
  b.len() == 4 && b.starts_with(b"DTS") && matches!(b[3], b' ' | b'B' | b'C' | b'E' | b'H' | b'L')
}

fn is_mp2_fourcc(key: &str) -> bool {
  key.starts_with("MP2") || matches!(key, ".MP1" | ".MP2")
}

fn is_mp3_fourcc(key: &str) -> bool {
  key.starts_with("MP3") || key == ".MP3"
}

fn is_pcm_fourcc(key: &str) -> bool {
  key.starts_with("PCM")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hdmv_registration_extracts_stream_coding_type() {
    let body = [b'H', b'D', b'M', b'V', 0xFF, 0x81, 0x00, 0x00];
    let r = decode(&body).unwrap();
    assert_eq!(r.format_identifier, "HDMV");
    assert_eq!(r.hdmv_stream_coding_type, Some(0x81));
  }

  #[test]
  fn hdmv_without_stuffing_byte_drops_coding_type() {
    let body = [b'H', b'D', b'M', b'V', 0x00, 0x81];
    let r = decode(&body).unwrap();
    assert_eq!(r.format_identifier, "HDMV");
    assert!(r.hdmv_stream_coding_type.is_none());
  }

  #[test]
  fn other_fourcc_decoded_without_hdmv_fields() {
    let body = [b'V', b'C', b'-', b'1'];
    let r = decode(&body).unwrap();
    assert_eq!(r.format_identifier, "VC-1");
    assert!(r.hdmv_stream_coding_type.is_none());
  }

  #[test]
  fn truncated_body_rejected() {
    assert!(decode(&[b'H', b'D']).is_none());
  }

  #[test]
  fn hdmv_codec_lookup_recognises_blu_ray_types() {
    assert_eq!(hdmv_codec(0x81).unwrap().0, "A_AC3");
    assert_eq!(hdmv_codec(0x83).unwrap().0, "A_TRUEHD");
    assert_eq!(hdmv_codec(0x90).unwrap().0, "S_HDMV/PGS");
    assert_eq!(hdmv_codec(0x02).unwrap().0, "V_MPEG12");
    assert!(hdmv_codec(0x01).is_none());
    assert!(hdmv_codec(0x20).is_none());
    assert!(hdmv_codec(0x91).is_none());
    assert!(hdmv_codec(0x00).is_none());
  }

  #[test]
  fn codec_for_fourcc_maps_known_identifiers() {
    assert_eq!(codec_for_fourcc("AC-3").unwrap().0, "A_AC3");
    assert_eq!(codec_for_fourcc("VC-1").unwrap().0, "V_VC1");
    assert_eq!(codec_for_fourcc("avc1").unwrap().0, "V_MPEG4/ISO/AVC");
    assert_eq!(codec_for_fourcc("MP4V").unwrap().0, "V_MPEG4/ISO/ASP");
    assert_eq!(codec_for_fourcc("MPGV").unwrap().0, "V_MPEG12");
    assert_eq!(codec_for_fourcc("MP4A").unwrap().0, "A_AAC");
    assert_eq!(codec_for_fourcc("TX3G").unwrap().0, "S_TX3G");
    assert!(codec_for_fourcc("XYZW").is_none());
  }

  #[test]
  fn codec_for_fourcc_drops_non_upstream_registration_aliases() {
    assert!(codec_for_fourcc("BSSD").is_none());
    assert!(codec_for_fourcc("DTS1").is_none());
    assert_eq!(codec_for_fourcc("EAC3").unwrap().0, "A_AC3");
  }
}
