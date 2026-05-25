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

//! Top-level `CoreAudioReader`. Pure-Rust port of
//! `mkvtoolnix/src/input/r_coreaudio.cpp`.
//!
//! Chunks are scanned through the whole file via seeks (PARSER-029) so `desc`,
//! `pakt`, and `kuki` are found regardless of position. The `pakt` packet
//! table feeds the duration and the `kuki` ALAC magic cookie becomes the
//! codec-private blob (PARSER-030). `supported` is set only for ALAC, matching
//! mkvtoolnix, which marks every other CAF codec unsupported (PARSER-031).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::endian::{get_u32_be, get_u64_be};
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::caf::{self, CAFF_MAGIC};

const MAX_CHUNKS: usize = 4096;
const MAX_CHUNK_READ: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct Chunk {
  ctype: [u8; 4],
  data_pos: u64,
  size: u64,
}

/// Port of `scan_chunks` — walk `type(4) + size(u64 BE)` headers through the
/// file. A zero size means "to end of file".
fn scan_chunks(src: &mut FileSource, file_size: u64) -> Result<Vec<Chunk>, ParseError> {
  let mut chunks = Vec::new();
  let mut pos = 8u64; // after "caff" + version(2) + flags(2)
  while chunks.len() < MAX_CHUNKS {
    src.seek_to(pos)?;
    let mut hdr = [0u8; 12];
    if src.read_at_most(&mut hdr)? < 12 {
      break;
    }
    let ctype = [hdr[0], hdr[1], hdr[2], hdr[3]];
    let raw_size = get_u64_be(&hdr[4..]);
    let data_pos = pos + 12;
    let remaining = file_size.saturating_sub(data_pos);
    let size = if raw_size == 0 {
      remaining
    } else {
      raw_size.min(remaining)
    };
    chunks.push(Chunk { ctype, data_pos, size });
    let next = data_pos.saturating_add(size);
    if next <= pos || next >= file_size {
      break;
    }
    pos = next;
  }
  Ok(chunks)
}

fn find_chunk<'a>(chunks: &'a [Chunk], ctype: &[u8; 4]) -> Option<&'a Chunk> {
  chunks.iter().find(|c| &c.ctype == ctype)
}

fn read_chunk_body(src: &mut FileSource, chunk: &Chunk) -> Result<Vec<u8>, ParseError> {
  src.seek_to(chunk.data_pos)?;
  let want = chunk.size.min(MAX_CHUNK_READ);
  let mut buf = vec![0u8; want as usize];
  let n = src.read_at_most(&mut buf)?;
  buf.truncate(n);
  Ok(buf)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CoreAudioReader;

impl Reader for CoreAudioReader {
  fn name(&self) -> &'static str {
    "coreaudio"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; 4];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    Ok(read == 4 && head == CAFF_MAGIC)
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    _deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    src.seek_to(0)?;
    let mut magic = [0u8; 8];
    if src.read_at_most(&mut magic)? < 8 || magic[..4] != CAFF_MAGIC {
      return Err(ParseError::Unrecognised);
    }
    let file_size = src.length().unwrap_or(u64::MAX);
    let chunks = scan_chunks(src, file_size)?;

    let desc_chunk = find_chunk(&chunks, b"desc").ok_or(ParseError::Malformed {
      format: "coreaudio",
      offset: 0,
      reason: "missing desc chunk".to_string(),
    })?;
    let desc_body = read_chunk_body(src, desc_chunk)?;
    let description = caf::decode_desc(&desc_body).ok_or(ParseError::Malformed {
      format: "coreaudio",
      offset: desc_chunk.data_pos,
      reason: "truncated desc chunk".to_string(),
    })?;

    // mkvtoolnix only supports ALAC inside CAF (PARSER-031).
    let is_alac = description.format_id.eq_ignore_ascii_case(b"alac");

    out.container.format = ContainerFormat::CoreAudio;
    out.container.recognized = true;
    out.container.supported = is_alac;

    // pakt → total frame count → duration.
    if let Some(pakt) = find_chunk(&chunks, b"pakt") {
      let body = read_chunk_body(src, pakt)?;
      if body.len() >= 24 && description.sample_rate > 0.0 {
        // num_packets(u64) · num_valid_frames(u64) · priming(u32) · remainder(u32).
        let valid_frames = get_u64_be(&body[8..]);
        let priming = get_u32_be(&body[16..]) as u64;
        let remainder = get_u32_be(&body[20..]) as u64;
        let total_frames = valid_frames.saturating_add(priming).saturating_add(remainder);
        let ns = (total_frames as u128) * 1_000_000_000 / description.sample_rate as u128;
        out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
      }
    }

    // kuki → ALAC magic cookie → codec_private.
    let mut codec_private = None;
    if is_alac {
      if let Some(kuki) = find_chunk(&chunks, b"kuki") {
        let body = read_chunk_body(src, kuki)?;
        if let Some(cookie) = caf::convert_alac_cookie(&body) {
          codec_private = Some(CodecPrivate::from_bytes(&cookie));
        }
      }
    }

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let mut audio = AudioTrackProperties::default();
    if description.sample_rate > 0.0 {
      audio.sampling_frequency = Some(description.sample_rate);
    }
    if description.channels > 0 {
      audio.channels = Some(description.channels);
    }
    if description.bits_per_channel > 0 {
      audio.bit_depth = Some(description.bits_per_channel);
    }
    let codec_id = format!("CAF/{}", caf::fourcc_string(&description.format_id));
    let codec_name = codec_name_for(&description.format_id);
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: codec_id,
        name: Some(codec_name.to_string()),
        codec_private,
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

fn codec_name_for(format_id: &[u8; 4]) -> &'static str {
  match format_id {
    b"lpcm" => "PCM",
    b"alac" => "ALAC (Apple Lossless)",
    b"aac " => "AAC",
    b"ulaw" => "G.711 \u{00B5}-law",
    b"alaw" => "G.711 A-law",
    b"MAC3" => "MACE 3:1",
    b"MAC6" => "MACE 6:1",
    b"ima4" => "IMA ADPCM",
    b".mp1" => "MPEG-1 Layer I",
    b".mp2" => "MPEG-1 Layer II",
    b".mp3" => "MP3",
    b"ac-3" => "AC-3",
    _ => "CoreAudio",
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::coreaudio::caf::build_caf;
  use std::io::Cursor;

  #[test]
  fn probe_accepts_caff_magic() {
    let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(CoreAudioReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_other_magic() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
    assert!(!CoreAudioReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_lpcm_track() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::CoreAudio);
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(24));
    assert_eq!(a.sampling_frequency, Some(48_000.0));
    assert_eq!(out.tracks[0].codec.id, "CAF/lpcm");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("PCM"));
  }

  // ---- PARSER-031: only ALAC is supported -------------------------------

  #[test]
  fn lpcm_is_recognised_but_unsupported() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
  }

  #[test]
  fn alac_is_supported() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"alac", 44_100.0, 2, 16);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.supported);
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("ALAC (Apple Lossless)"));
  }

  // ---- PARSER-029 / 030: late chunks, pakt, kuki ------------------------

  #[test]
  fn finds_desc_after_large_data_chunk() {
    // free chunk (96 KiB) before desc — beyond the old 64 KiB window.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"free");
    bytes.extend_from_slice(&(96u64 * 1024).to_be_bytes());
    bytes.extend(vec![0u8; 96 * 1024]);
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&32u64.to_be_bytes());
    bytes.extend_from_slice(&48_000.0f64.to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 4]); // flags
    bytes.extend_from_slice(&[0u8; 4]); // bytes_per_packet
    bytes.extend_from_slice(&1024u32.to_be_bytes());
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&16u32.to_be_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.supported);
    assert_eq!(out.tracks[0].properties.audio.as_ref().unwrap().channels, Some(2));
  }

  #[test]
  fn pakt_yields_duration_and_kuki_codec_private() {
    use crate::media_metadata::deadline::Deadline;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    // desc (alac, 48000)
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&32u64.to_be_bytes());
    bytes.extend_from_slice(&48_000.0f64.to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 12]); // flags + bytes_per_packet + frames_per_packet
    bytes.extend_from_slice(&2u32.to_be_bytes()); // channels
    bytes.extend_from_slice(&16u32.to_be_bytes()); // bits
    // pakt: num_packets, valid_frames=96000, priming=0, remainder=0
    bytes.extend_from_slice(b"pakt");
    bytes.extend_from_slice(&24u64.to_be_bytes());
    bytes.extend_from_slice(&10u64.to_be_bytes()); // num_packets
    bytes.extend_from_slice(&96_000u64.to_be_bytes()); // valid frames → 2s
    bytes.extend_from_slice(&0u32.to_be_bytes()); // priming
    bytes.extend_from_slice(&0u32.to_be_bytes()); // remainder
    // kuki: new-style ALAC config (24 bytes)
    let mut cfg = vec![0u8; caf::ALAC_CONFIG_SIZE];
    cfg[11] = 2; // num_channels
    cfg[5] = 16; // bit_depth
    bytes.extend_from_slice(b"kuki");
    bytes.extend_from_slice(&(cfg.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&cfg);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
    assert!(out.tracks[0].codec.codec_private.is_some());
  }

  #[test]
  fn codec_name_for_table_covers_common_formats() {
    assert_eq!(codec_name_for(b"lpcm"), "PCM");
    assert_eq!(codec_name_for(b"aac "), "AAC");
    assert_eq!(codec_name_for(b".mp3"), "MP3");
    assert_eq!(codec_name_for(b"ac-3"), "AC-3");
    assert_eq!(codec_name_for(b"XXXX"), "CoreAudio");
  }

  #[test]
  fn read_headers_returns_malformed_without_desc_chunk() {
    use crate::media_metadata::deadline::Deadline;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
