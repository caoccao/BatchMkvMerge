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

#[derive(Debug, Clone)]
struct Chunk {
  ctype: [u8; 4],
  data_pos: u64,
  size: u64,
}

/// Port of `scan_chunks` — walk `type(4) + size(u64 BE)` headers through the
/// file. mkvtoolnix treats a declared size of zero as a file-sized chunk; the
/// later exact body read then fails because the body starts after the chunk
/// header and cannot contain that many bytes.
fn scan_chunks(src: &mut FileSource, file_size: u64, deadline: &Deadline) -> Result<Vec<Chunk>, ParseError> {
  let mut chunks = Vec::new();
  let mut pos = 8u64; // after "caff" + version(2) + flags(2)
  loop {
    deadline.check("coreaudio::scan_chunks")?;
    src.seek_to(pos)?;
    let mut hdr = [0u8; 12];
    if src.read_at_most(&mut hdr)? < 12 {
      break;
    }
    let ctype = [hdr[0], hdr[1], hdr[2], hdr[3]];
    let raw_size = get_u64_be(&hdr[4..]);
    let data_pos = pos + 12;
    let size = if raw_size == 0 { file_size } else { raw_size };
    chunks.push(Chunk { ctype, data_pos, size });
    let Some(next) = data_pos.checked_add(size) else {
      break;
    };
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

fn require_chunk<'a>(chunks: &'a [Chunk], ctype: &[u8; 4], name: &'static str) -> Result<&'a Chunk, ParseError> {
  find_chunk(chunks, ctype).ok_or(ParseError::Malformed {
    format: "coreaudio",
    offset: 0,
    reason: format!("missing {name} chunk"),
  })
}

fn read_chunk_body(src: &mut FileSource, chunk: &Chunk) -> Result<Vec<u8>, ParseError> {
  if chunk.size == 0 {
    return Err(ParseError::Malformed {
      format: "coreaudio",
      offset: chunk.data_pos,
      reason: "zero-sized required chunk".to_string(),
    });
  }
  src.seek_to(chunk.data_pos)?;
  let mut buf = vec![0u8; chunk.size as usize];
  src.read_exact(&mut buf)?;
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
    Ok(read == 4 && head.eq_ignore_ascii_case(&CAFF_MAGIC))
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    src.seek_to(0)?;
    let mut magic = [0u8; 8];
    if src.read_at_most(&mut magic)? < 8 || !magic[..4].eq_ignore_ascii_case(&CAFF_MAGIC) {
      return Err(ParseError::Unrecognised);
    }
    let file_size = src.length().unwrap_or(u64::MAX);
    let chunks = scan_chunks(src, file_size, deadline)?;

    let desc_chunk = require_chunk(&chunks, b"desc", "desc")?;
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

    let pakt = require_chunk(&chunks, b"pakt", "pakt")?;
    let _data = require_chunk(&chunks, b"data", "data")?;
    let pakt_body = read_chunk_body(src, pakt)?;
    if pakt_body.len() < 24 {
      return Err(ParseError::Malformed {
        format: "coreaudio",
        offset: pakt.data_pos,
        reason: "truncated pakt chunk".to_string(),
      });
    }

    // mkvtoolnix's `identify()` reports a non-ALAC CAF as a recognised but
    // unsupported container and returns before emitting any track
    // (`r_coreaudio.cpp:34-48`).  Mirror that: claim the container, emit no
    // track.  PARSER-188.
    if !is_alac {
      return Ok(());
    }

    // pakt → total frame count → duration.
    if description.sample_rate > 0.0 {
      // num_packets(u64) · num_valid_frames(u64) · priming(u32) · remainder(u32).
      let valid_frames = get_u64_be(&pakt_body[8..]);
      let priming = get_u32_be(&pakt_body[16..]) as u64;
      let remainder = get_u32_be(&pakt_body[20..]) as u64;
      let total_frames = valid_frames.saturating_add(priming).saturating_add(remainder);
      let ns = (total_frames as u128) * 1_000_000_000 / description.sample_rate as u128;
      out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
    }

    // kuki → ALAC magic cookie → codec_private.  Only reached for ALAC.
    let mut codec_private = None;
    if let Some(kuki) = find_chunk(&chunks, b"kuki") {
      let body = read_chunk_body(src, kuki)?;
      let cookie = caf::convert_alac_cookie(&body).ok_or(ParseError::Malformed {
        format: "coreaudio",
        offset: kuki.data_pos,
        reason: "invalid ALAC magic cookie".to_string(),
      })?;
      codec_private = Some(CodecPrivate::from_bytes(&cookie));
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
  fn probe_accepts_ascii_case_variants_of_caff_magic() {
    let mut bytes = build_caf(b"alac", 48_000.0, 2, 24);
    bytes[0..4].copy_from_slice(b"CAFF");
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(CoreAudioReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::CoreAudio);
  }

  #[test]
  fn probe_rejects_other_magic() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
    assert!(!CoreAudioReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_alac_track() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"alac", 48_000.0, 2, 24);
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
    assert_eq!(out.tracks[0].codec.id, "CAF/alac");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("ALAC (Apple Lossless)"));
  }

  // ---- PARSER-031 / PARSER-188: only ALAC is supported, and a non-ALAC
  // (unsupported) CAF is reported as a recognised container with NO track,
  // mirroring mkvtoolnix's `identify()` which calls
  // `id_result_container_unsupported` and returns before `id_result_track`.

  #[test]
  fn lpcm_is_recognised_but_unsupported_with_no_track() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::CoreAudio);
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty(), "non-ALAC CAF must emit no track");
  }

  #[test]
  fn aac_caf_is_recognised_but_unsupported_with_no_track() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_caf(b"aac ", 44_100.0, 2, 16);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
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
    bytes.extend_from_slice(b"pakt");
    bytes.extend_from_slice(&24u64.to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&4u64.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.supported);
    assert_eq!(out.tracks[0].properties.audio.as_ref().unwrap().channels, Some(2));
  }

  #[test]
  fn scans_past_four_thousand_chunks_before_required_headers() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    for _ in 0..4096 {
      bytes.extend_from_slice(b"free");
      bytes.extend_from_slice(&1u64.to_be_bytes());
      bytes.push(0);
    }
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&32u64.to_be_bytes());
    bytes.extend_from_slice(&48_000.0f64.to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 12]);
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&16u32.to_be_bytes());
    bytes.extend_from_slice(b"pakt");
    bytes.extend_from_slice(&24u64.to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&4u64.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 4]);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.supported);
    assert_eq!(out.tracks[0].codec.id, "CAF/alac");
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
    // data: edit_count plus enough payload for the packet table relationship.
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&4u64.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
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
  fn read_headers_requires_pakt_and_data_chunks() {
    use crate::media_metadata::deadline::Deadline;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&32u64.to_be_bytes());
    bytes.extend_from_slice(&48_000.0f64.to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 12]);
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&16u32.to_be_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));

    let mut bytes = build_caf(b"alac", 48_000.0, 2, 16);
    let data_pos = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes.truncate(data_pos);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_headers_rejects_desc_that_extends_past_eof() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&64u64.to_be_bytes());
    bytes.extend_from_slice(&48_000.0f64.to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 12]);
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&16u32.to_be_bytes());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("truncated-desc.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn read_headers_rejects_zero_sized_required_chunk() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&0u64.to_be_bytes());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("zero-desc.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. } | ParseError::OversizedElement { .. }));
  }

  #[test]
  fn invalid_alac_kuki_is_malformed() {
    use crate::media_metadata::deadline::Deadline;
    let mut bytes = build_caf(b"alac", 48_000.0, 2, 16);
    bytes.extend_from_slice(b"kuki");
    bytes.extend_from_slice(&8u64.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.caf", 0);
    let err = CoreAudioReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
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
