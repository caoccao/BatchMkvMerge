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

//! `esds` — MPEG-4 Elementary Stream Descriptor (ISO/IEC 14496-1).
//!
//! Used by AAC, MP3-in-MP4 and other MPEG-4-system streams.  The descriptor
//! is a nested TLV tree of MPEG-4 BER-encoded objects.  We walk just enough
//! of it to extract:
//!
//! - `objectTypeIndication` (e.g. 0x40 = AAC).
//! - `streamType` / `bufferSizeDB` / `maxBitrate` / `avgBitrate`.
//! - `DecoderSpecificInfo` (AudioSpecificConfig for AAC).
//!
//! AAC AudioSpecificConfig bytes are decoded through the shared AAC parser so
//! MP4, raw AAC, FLV and RealMedia all agree on object type, sample rate,
//! channel layout, SBR/PS flags and malformed-input semantics.

use crate::media_metadata::audio::aac;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::AudioCodecConfig;

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::moov::trak::TrackBuilder;

use super::hex_encode;

const TAG_ES_DESCRIPTOR: u8 = 0x03;
const TAG_DECODER_CONFIG: u8 = 0x04;
const TAG_DEC_SPECIFIC_INFO: u8 = 0x05;

pub fn parse(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  parse_with_cap(src, header, builder, u64::MAX)
}

pub fn parse_with_cap(
  src: &mut FileSource,
  header: &BoxHeader,
  builder: &mut TrackBuilder,
  payload_cap: u64,
) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, payload_cap)?;
  if payload.len() < 4 {
    return Ok(());
  }
  // 4-byte FullBox header (version + flags).
  let body = &payload[4..];
  let mut cursor = Cursor { data: body, pos: 0 };
  let mut cfg = AudioCodecConfig::default();
  cfg.raw_hex = Some(hex_encode(&payload));
  let mut object_type: Option<u8> = None;
  let mut decoder_specific_len: Option<usize> = None;
  let mut decoder_specific_data: Option<Vec<u8>> = None;
  walk(
    &mut cursor,
    &mut cfg,
    &mut object_type,
    &mut decoder_specific_len,
    &mut decoder_specific_data,
  )?;

  if object_type.is_some_and(is_aac_object_type) {
    let asc = decoder_specific_data
      .as_ref()
      .filter(|data| data.len() >= 2)
      .cloned()
      .unwrap_or_else(|| create_default_aac_audio_specific_config(builder));
    if let Some(header) = aac::parse_audio_specific_config_bytes(&asc) {
      apply_aac_header_to_builder(builder, &header);
      cfg = aac::codec_config_from_header(&header, &asc);
      decoder_specific_len = Some(asc.len());
      decoder_specific_data = Some(asc);
    }
  }

  builder.audio_codec_config = Some(cfg);
  builder.esds_object_type = object_type;
  // PARSER-177: record the DecoderSpecificInfo length for the reader's
  // verification gates (MP4V / VobSub).
  builder.esds_decoder_specific_len = decoder_specific_len;
  // PARSER-230: keep the raw DecoderSpecificInfo bytes so the verification pass
  // can unlace Vorbis-in-MP4 private data.
  builder.esds_decoder_specific_data = decoder_specific_data;
  builder.codec_private_hex = Some(hex_encode(&payload));
  Ok(())
}

struct Cursor<'a> {
  data: &'a [u8],
  pos: usize,
}

impl<'a> Cursor<'a> {
  fn read_u8(&mut self) -> Option<u8> {
    let b = *self.data.get(self.pos)?;
    self.pos += 1;
    Some(b)
  }
  fn read_u16_be(&mut self) -> Option<u16> {
    if self.pos + 2 > self.data.len() {
      return None;
    }
    let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
    self.pos += 2;
    Some(v)
  }
  fn read_u32_be(&mut self) -> Option<u32> {
    if self.pos + 4 > self.data.len() {
      return None;
    }
    let v = u32::from_be_bytes([
      self.data[self.pos],
      self.data[self.pos + 1],
      self.data[self.pos + 2],
      self.data[self.pos + 3],
    ]);
    self.pos += 4;
    Some(v)
  }
  fn slice(&self, len: usize) -> Option<&'a [u8]> {
    self.data.get(self.pos..self.pos + len)
  }
  fn skip(&mut self, n: usize) {
    self.pos = (self.pos + n).min(self.data.len());
  }
  /// MPEG-4 BER-encoded length (max 4 bytes, 7-bit chunks).
  fn read_ber_length(&mut self) -> Option<usize> {
    let mut value = 0usize;
    for _ in 0..4 {
      let b = self.read_u8()?;
      value = (value << 7) | ((b & 0x7F) as usize);
      if b & 0x80 == 0 {
        return Some(value);
      }
    }
    Some(value)
  }
}

fn walk(
  cursor: &mut Cursor,
  cfg: &mut AudioCodecConfig,
  object_type_out: &mut Option<u8>,
  decoder_specific_len_out: &mut Option<usize>,
  decoder_specific_data_out: &mut Option<Vec<u8>>,
) -> Result<(), ParseError> {
  while let Some(tag) = cursor.read_u8() {
    let len = match cursor.read_ber_length() {
      Some(l) => l,
      None => return Ok(()),
    };
    let body_end = cursor.pos.checked_add(len).ok_or_else(|| ParseError::Malformed {
      format: "mp4",
      offset: cursor.pos as u64,
      reason: "esds descriptor length overflow".to_string(),
    })?;
    if body_end > cursor.data.len() {
      return Err(ParseError::Malformed {
        format: "mp4",
        offset: cursor.pos as u64,
        reason: format!("truncated esds descriptor tag {tag:#x} body"),
      });
    }
    match tag {
      TAG_ES_DESCRIPTOR => {
        let _esid = cursor.read_u16_be();
        let flags = cursor.read_u8().unwrap_or(0);
        // streamDependenceFlag(1) | URL_Flag(1) | OCRStreamFlag(1) | streamPriority(5)
        if flags & 0x80 != 0 {
          cursor.skip(2); // dependsOn_ES_ID
        }
        if flags & 0x40 != 0 {
          if let Some(url_len) = cursor.read_u8() {
            cursor.skip(url_len as usize);
          }
        }
        if flags & 0x20 != 0 {
          cursor.skip(2); // OCR_ES_Id
        }
        // recurse into the rest of the ES descriptor
        let mut inner = Cursor {
          data: &cursor.data[cursor.pos..body_end],
          pos: 0,
        };
        walk(
          &mut inner,
          cfg,
          object_type_out,
          decoder_specific_len_out,
          decoder_specific_data_out,
        )?;
        cursor.pos = body_end;
      }
      TAG_DECODER_CONFIG => {
        let object_type = cursor.read_u8().unwrap_or(0);
        *object_type_out = Some(object_type);
        let _stream_type = cursor.read_u8();
        let _buffer = cursor.read_u8().is_some() && cursor.read_u8().is_some() && cursor.read_u8().is_some();
        let max_bitrate = cursor.read_u32_be();
        let avg_bitrate = cursor.read_u32_be();
        cfg.profile_name = Some(format_object_type(object_type).to_string());
        let _ = (max_bitrate, avg_bitrate); // identification doesn't expose bitrates
        // recurse to pick up the nested DecSpecificInfo
        let mut inner = Cursor {
          data: &cursor.data[cursor.pos..body_end],
          pos: 0,
        };
        walk(
          &mut inner,
          cfg,
          object_type_out,
          decoder_specific_len_out,
          decoder_specific_data_out,
        )?;
        cursor.pos = body_end;
      }
      TAG_DEC_SPECIFIC_INFO => {
        // PARSER-177: record the DecoderSpecificInfo length (mkvtoolnix's
        // `esds.decoder_config`) so the verification pass can gate MP4V /
        // VobSub tracks on its presence / size.
        *decoder_specific_len_out = Some(len);
        // PARSER-230: retain the raw bytes for Vorbis-in-MP4 unlacing.
        *decoder_specific_data_out = Some(cursor.slice(len).expect("validated descriptor body").to_vec());
        cursor.pos = body_end;
      }
      _ => {
        cursor.pos = body_end;
      }
    }
  }
  Ok(())
}

fn is_aac_object_type(object_type: u8) -> bool {
  matches!(object_type, 0x40 | 0x66 | 0x67 | 0x68)
}

fn apply_aac_header_to_builder(builder: &mut TrackBuilder, header: &aac::AacHeader) {
  let audio = builder.audio.get_or_insert_with(Default::default);
  let existing_channels = audio.channels.unwrap_or(0);
  if existing_channels != 8 || header.channels != 7 {
    audio.channels = if header.channels > 0 {
      Some(header.channels)
    } else {
      None
    };
  }
  audio.sampling_frequency = if header.sample_rate > 0 {
    Some(header.sample_rate as f64)
  } else {
    None
  };
  audio.output_sampling_frequency = if header.output_sample_rate > 0 {
    Some(header.output_sample_rate as f64)
  } else {
    None
  };
}

fn create_default_aac_audio_specific_config(builder: &TrackBuilder) -> Vec<u8> {
  let sample_rate = builder
    .audio
    .as_ref()
    .and_then(|a| a.sampling_frequency)
    .filter(|r| *r >= 0.0 && *r <= u32::MAX as f64)
    .map(|r| r.round() as u32)
    .unwrap_or(0);
  let channels = builder.audio.as_ref().and_then(|a| a.channels).unwrap_or(0);
  build_audio_specific_config(/* profile = AAC Main */ 0, sample_rate, channels)
}

fn build_audio_specific_config(profile: u32, sample_rate: u32, channels: u32) -> Vec<u8> {
  let object_type = profile + 1;
  let sample_rate_index = sampling_frequency_index(sample_rate);
  let channel_config = channel_configuration(channels);
  let mut w = BitWriter::default();
  w.write_bits(object_type as u64, 5);
  w.write_bits(sample_rate_index as u64, 4);
  if sample_rate_index == 0x0f {
    w.write_bits(sample_rate as u64, 24);
  }
  w.write_bits(channel_config as u64, 4);
  w.into_bytes()
}

fn sampling_frequency_index(sample_rate: u32) -> u8 {
  if sample_rate == 0 {
    return 0;
  }
  const TABLE: [u32; 16] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350, 0, 0, 0,
  ];
  for (idx, rate) in TABLE.iter().copied().enumerate() {
    if rate != 0 && sample_rate >= rate.saturating_sub(1000) {
      return idx as u8;
    }
  }
  0
}

fn channel_configuration(channels: u32) -> u8 {
  const TABLE: [u32; 21] = [0, 1, 2, 3, 4, 5, 6, 8, 0, 3, 4, 7, 8, 24, 8, 12, 10, 12, 14, 12, 14];
  TABLE.iter().position(|c| *c == channels).unwrap_or(0) as u8
}

#[derive(Default)]
struct BitWriter {
  buf: Vec<u8>,
  bit_index: u8,
}

impl BitWriter {
  fn write_bit(&mut self, bit: bool) {
    if self.bit_index == 0 {
      self.buf.push(0);
    }
    if bit {
      let last = self.buf.len() - 1;
      self.buf[last] |= 1 << (7 - self.bit_index);
    }
    self.bit_index = (self.bit_index + 1) % 8;
  }

  fn write_bits(&mut self, value: u64, bits: u32) {
    for i in 0..bits {
      self.write_bit(((value >> (bits - 1 - i)) & 1) != 0);
    }
  }

  fn into_bytes(mut self) -> Vec<u8> {
    while self.bit_index != 0 {
      self.write_bit(false);
    }
    self.buf
  }
}

fn format_object_type(idc: u8) -> &'static str {
  match idc {
    0x40 => "AAC",
    0x41 => "AAC main",
    0x6B => "MP3 (MPEG-1 Layer III)",
    0x69 => "MP3 (MPEG-2 Layer III)",
    0x67 => "MPEG-2 AAC",
    0xA5 => "AC-3",
    0xA6 => "E-AC-3",
    0xA9 => "DTS",
    0xDD => "Vorbis",
    _ => "Unknown",
  }
}

#[cfg(test)]
pub(crate) fn build_esds_payload(object_type: u8, audio_specific_config: &[u8]) -> Vec<u8> {
  // FullBox header
  let mut p = vec![0u8; 4];
  // ES descriptor:  ES_ID(2) + flags(1) = 3 bytes header + DecoderConfig inline
  let dec_specific = {
    let mut v = vec![TAG_DEC_SPECIFIC_INFO];
    v.push(audio_specific_config.len() as u8);
    v.extend_from_slice(audio_specific_config);
    v
  };
  let dec_config = {
    let mut v = vec![TAG_DECODER_CONFIG];
    let body_len = 13 + dec_specific.len();
    v.push(body_len as u8); // 1-byte BER length
    v.push(object_type);
    v.push(0x15); // streamType + flags
    v.extend_from_slice(&[0u8; 3]); // bufferSizeDB
    v.extend_from_slice(&0u32.to_be_bytes()); // maxBitrate
    v.extend_from_slice(&0u32.to_be_bytes()); // avgBitrate
    v.extend_from_slice(&dec_specific);
    v
  };
  let es_descriptor = {
    let mut v = vec![TAG_ES_DESCRIPTOR];
    let body_len = 3 + dec_config.len();
    v.push(body_len as u8);
    v.extend_from_slice(&[0u8; 2]); // ES_ID
    v.push(0); // flags
    v.extend_from_slice(&dec_config);
    v
  };
  p.extend_from_slice(&es_descriptor);
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor as StdCursor;

  fn run(payload: Vec<u8>) -> TrackBuilder {
    let bytes = encode_box(b"esds", &payload);
    let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  fn run_result(payload: Vec<u8>) -> Result<TrackBuilder, ParseError> {
    let bytes = encode_box(b"esds", &payload);
    let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b)?;
    Ok(b)
  }

  fn run_with_audio(payload: Vec<u8>, channels: u32, sample_rate: f64) -> TrackBuilder {
    let bytes = encode_box(b"esds", &payload);
    let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    b.audio = Some(
      crate::media_metadata::model::track_properties_audio::AudioTrackProperties {
        channels: Some(channels),
        sampling_frequency: Some(sample_rate),
        ..Default::default()
      },
    );
    parse(&mut s, &h, &mut b).unwrap();
    b
  }

  fn aac_lc_specific_config(sample_rate_idx: u8, channels: u8) -> Vec<u8> {
    // 5 bits AOT (2 = AAC LC) + 4 bits sample_rate_idx + 4 bits channels
    let aot = 2u16;
    let value = (aot << 11) | ((sample_rate_idx as u16) << 7) | ((channels as u16) << 3);
    vec![(value >> 8) as u8, (value & 0xFF) as u8]
  }

  fn explicit_sbr_or_ps_config(aot: u32) -> Vec<u8> {
    let mut writer = BitWriter::default();
    writer.write_bits(u64::from(aot), 5);
    writer.write_bits(4, 4); // 44.1 kHz core rate
    writer.write_bits(2, 4); // stereo
    writer.write_bits(3, 4); // 48 kHz extension rate
    writer.write_bits(2, 5); // AAC LC extension object type
    writer.write_bits(0, 1); // frame_length_flag
    writer.write_bits(0, 1); // depends_on_core_coder
    writer.write_bits(0, 1); // extension_flag
    writer.into_bytes()
  }

  #[test]
  fn aac_object_type_decoded() {
    let asc = aac_lc_specific_config(4, 2); // 44.1k stereo
    let payload = build_esds_payload(0x40, &asc);
    let b = run(payload);
    let cfg = b.audio_codec_config.unwrap();
    assert_eq!(cfg.aac_object_type, Some(2));
    assert_eq!(cfg.aac_frame_length, Some(1024));
    assert_eq!(cfg.profile_name.as_deref(), Some("AAC LC"));
  }

  #[test]
  fn aac_sbr_extension_detected() {
    let asc = explicit_sbr_or_ps_config(5);
    let payload = build_esds_payload(0x40, &asc);
    let b = run(payload);
    let cfg = b.audio_codec_config.unwrap();
    assert_eq!(cfg.aac_sbr_present, Some(true));
    assert_eq!(cfg.aac_ps_present, Some(false));
  }

  #[test]
  fn aac_ps_extension_detected() {
    let asc = explicit_sbr_or_ps_config(29);
    let payload = build_esds_payload(0x40, &asc);
    let b = run(payload);
    let cfg = b.audio_codec_config.unwrap();
    assert_eq!(cfg.aac_ps_present, Some(true));
  }

  #[test]
  fn raw_hex_is_decoder_specific_info() {
    let asc = aac_lc_specific_config(4, 2);
    let payload = build_esds_payload(0x40, &asc);
    let b = run(payload);
    let raw = b.audio_codec_config.unwrap().raw_hex.unwrap();
    let decoded: Vec<u8> = (0..raw.len())
      .step_by(2)
      .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap())
      .collect();
    assert_eq!(decoded, asc);
  }

  #[test]
  fn payload_larger_than_sixty_four_kib_is_preserved() {
    let asc = aac_lc_specific_config(4, 2);
    let mut payload = build_esds_payload(0x40, &asc);
    payload.extend(vec![0; 70 * 1024]);
    let b = run(payload);
    assert!(b.codec_private_hex.unwrap().len() > 64 * 1024 * 2);
  }

  #[test]
  fn aac_config_updates_audio_fields_from_asc() {
    let asc = aac_lc_specific_config(3, 6); // 48k, 5.1
    let payload = build_esds_payload(0x40, &asc);
    let b = run_with_audio(payload, 2, 44_100.0);
    let audio = b.audio.unwrap();
    assert_eq!(audio.channels, Some(6));
    assert_eq!(audio.sampling_frequency, Some(48_000.0));
  }

  #[test]
  fn aac_missing_decoder_specific_synthesizes_default_asc() {
    let payload = build_esds_payload(0x40, &[]);
    let b = run_with_audio(payload, 2, 44_100.0);
    assert_eq!(b.esds_decoder_specific_len, Some(2));
    assert_eq!(b.esds_decoder_specific_data.as_deref(), Some(&[0x0a, 0x10][..]));
    let cfg = b.audio_codec_config.unwrap();
    assert_eq!(cfg.aac_object_type, Some(1));
    let audio = b.audio.unwrap();
    assert_eq!(audio.channels, Some(2));
    assert_eq!(audio.sampling_frequency, Some(44_100.0));
  }

  #[test]
  fn truncated_decoder_specific_info_is_malformed() {
    let payload = vec![
      0, 0, 0, 0, // FullBox header
      TAG_DEC_SPECIFIC_INFO,
      0x40, // declares 64 bytes
      0x12, 0x10,
    ];
    assert!(matches!(run_result(payload), Err(ParseError::Malformed { .. })));
  }

  #[test]
  fn empty_payload_is_noop() {
    // Box with only 4-byte FullBox header
    let bytes = encode_box(b"esds", &[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(StdCursor::new(bytes));
    let h = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    parse(&mut s, &h, &mut b).unwrap();
    assert!(b.audio_codec_config.is_some()); // raw_hex populated, others None
    let cfg = b.audio_codec_config.unwrap();
    assert!(cfg.aac_object_type.is_none());
  }

  #[test]
  fn ber_length_handles_multibyte() {
    // 0x81 0x01 = 7-bit length = 129 ... ensure no panic
    let mut cur = Cursor {
      data: &[0x81, 0x01, 0x00],
      pos: 0,
    };
    assert_eq!(cur.read_ber_length(), Some(129));
  }

  #[test]
  fn object_type_table() {
    assert_eq!(format_object_type(0x40), "AAC");
    assert_eq!(format_object_type(0xA5), "AC-3");
    assert_eq!(format_object_type(0xDD), "Vorbis");
    assert_eq!(format_object_type(0xFE), "Unknown");
  }
}
