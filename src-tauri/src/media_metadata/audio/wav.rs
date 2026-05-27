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

//! WAV reader. Pure-Rust port of `mkvtoolnix/src/input/r_wav.cpp`. Supports
//! classic `RIFF/WAVE`, `RF64/WAVE` (>4 GB), and Wave64 (PARSER-020). The
//! chunk structure is walked through the whole file via seeks rather than a
//! fixed 16 KiB window (PARSER-022), and `WAVE_FORMAT_EXTENSIBLE` (0xFFFE) is
//! unwrapped to the subformat GUID's `data1` codec tag (PARSER-021). The
//! payload byte total sums the lengths of *all* `data` chunks (PARSER-227),
//! matching `scan_chunks_wave`'s `m_bytes_in_data_chunks` accumulation, and a
//! huge `data` chunk whose 32-bit length wrapped is repaired from the file size
//! when another chunk follows it in a >4 GiB file (PARSER-254).  AC-3/DTS
//! payload classification probes the first non-empty data chunk with the same
//! consecutive-frame gates their elementary readers use.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::endian::{get_u16_le, get_u32_le, get_u64_le};
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::reader::Reader;

use super::{ac3, dts};

const WAVE_FORMAT_PCM: u32 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u32 = 0x0003;
const WAVE_FORMAT_DTS: u32 = 0x2001;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// `sizeof(alWAVEFORMATEXTENSIBLE)` = WAVEFORMATEX(18) + ext(2 + 4 + 16).
const WAVEFORMATEXTENSIBLE_SIZE: usize = 40;
/// Byte offset of the SubFormat GUID `data1` field inside the fmt chunk.
const SUBFORMAT_DATA1_OFFSET: usize = 24;
const FMT_READ_CAP: u64 = 4096;
const PAYLOAD_PROBE_CAP: u64 = 128 * 1024;

/// Wave64 RIFF GUID (`mtx::w64::g_guid_riff`).
const W64_GUID_RIFF: [u8; 16] = [
  b'r', b'i', b'f', b'f', 0x2e, 0x91, 0xcf, 0x11, 0xa5, 0xd6, 0x28, 0xdb, 0x04, 0xc1, 0x00, 0x00,
];
/// Wave64 WAVE GUID (`mtx::w64::g_guid_wave`).
const W64_GUID_WAVE: [u8; 16] = [
  b'w', b'a', b'v', b'e', 0xf3, 0xac, 0xd3, 0x11, 0x8c, 0xd1, 0x00, 0xc0, 0x4f, 0x8e, 0xdb, 0x8a,
];
/// Wave64 chunk header size (`sizeof(mtx::w64::chunk_t)`): 16-byte GUID + u64.
const W64_CHUNK_HEADER: u64 = 24;
/// Wave64 file header size (`sizeof(mtx::w64::header_t)`): chunk_t + 16-byte GUID.
const W64_HEADER: u64 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavType {
  Wave,
  Rf64,
  Wave64,
}

#[derive(Debug, Clone)]
pub struct WaveFormat {
  /// Resolved format tag — the SubFormat GUID `data1` when the fmt tag is
  /// `WAVE_FORMAT_EXTENSIBLE`, otherwise the raw `wFormatTag`.
  pub format_tag: u32,
  pub channels: u16,
  pub sample_rate: u32,
  pub avg_bytes_per_sec: u32,
  pub block_align: u16,
  pub bits_per_sample: u16,
  pub extra: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WavMetadata {
  pub wav_type: WavType,
  pub format: WaveFormat,
  pub data_bytes: u64,
  pub supported: bool,
}

#[derive(Debug, Clone)]
struct Chunk {
  id: [u8; 4],
  pos: u64,
  len: u64,
}

fn id_eq(id: &[u8; 4], want: &[u8; 4]) -> bool {
  id.eq_ignore_ascii_case(want)
}

/// Port of `wav_reader_c::determine_type`.
fn determine_type(head: &[u8]) -> Option<WavType> {
  if head.len() < W64_HEADER as usize {
    return None;
  }
  if &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
    return Some(WavType::Wave);
  }
  if &head[0..4] == b"RF64" && &head[8..12] == b"WAVE" {
    return Some(WavType::Rf64);
  }
  if head[0..16] == W64_GUID_RIFF && head[24..40] == W64_GUID_WAVE {
    return Some(WavType::Wave64);
  }
  None
}

/// Walk the RIFF chunk list (`scan_chunks_wave`). mkvtoolnix's WAV scanner
/// advances by the declared chunk length only; it does not consume a RIFF
/// word-alignment pad byte after odd-sized chunks.
#[cfg(test)]
fn scan_chunks_riff(src: &mut FileSource, file_size: u64) -> Result<Vec<Chunk>, ParseError> {
  scan_chunks_riff_with_deadline(src, file_size, None)
}

fn scan_chunks_riff_with_deadline(
  src: &mut FileSource,
  file_size: u64,
  deadline: Option<&Deadline>,
) -> Result<Vec<Chunk>, ParseError> {
  let mut chunks: Vec<Chunk> = Vec::new();
  let mut pos = 12u64; // after RIFF id + size + WAVE id
  loop {
    if let Some(deadline) = deadline {
      deadline.check("wav::scan_chunks_riff")?;
    }
    src.seek_to(pos)?;
    let mut hdr = [0u8; 8];
    if src.read_at_most(&mut hdr)? != 8 {
      break;
    }
    let mut id = [0u8; 4];
    id.copy_from_slice(&hdr[0..4]);
    let len = get_u32_le(&hdr[4..]) as u64;
    let data_pos = pos + 8;

    // PARSER-254: repair a huge `data` chunk whose 32-bit length wrapped or was
    // written incorrectly.  Mirroring `scan_chunks_wave`
    // (`../mkvtoolnix/src/input/r_wav.cpp`), when a non-`data` chunk follows a
    // `data` chunk and the file is larger than 4 GiB, recompute the previous
    // `data` chunk's length from `file_size - previous.pos` and stop scanning.
    if !id_eq(&id, b"data") && file_size > 0x1_0000_0000 {
      if let Some(prev) = chunks.last_mut() {
        if id_eq(&prev.id, b"data") {
          prev.len = file_size.saturating_sub(prev.pos);
          break;
        }
      }
    }

    chunks.push(Chunk { id, pos: data_pos, len });
    let next = data_pos.saturating_add(len);
    if next <= pos || next > file_size.max(data_pos) {
      break;
    }
    pos = next;
  }
  Ok(chunks)
}

/// Walk the Wave64 chunk list (`scan_chunks_wave64`).
fn scan_chunks_wave64_with_deadline(
  src: &mut FileSource,
  file_size: u64,
  deadline: Option<&Deadline>,
) -> Result<Vec<Chunk>, ParseError> {
  let mut chunks = Vec::new();
  let mut pos = W64_HEADER;
  loop {
    if let Some(deadline) = deadline {
      deadline.check("wav::scan_chunks_wave64")?;
    }
    src.seek_to(pos)?;
    let mut hdr = [0u8; W64_CHUNK_HEADER as usize];
    if src.read_at_most(&mut hdr)? != W64_CHUNK_HEADER as usize {
      break;
    }
    let mut id = [0u8; 4];
    id.copy_from_slice(&hdr[0..4]);
    let size = get_u64_le(&hdr[16..]);
    if size < W64_CHUNK_HEADER {
      break;
    }
    let len = size - W64_CHUNK_HEADER;
    let data_pos = pos + W64_CHUNK_HEADER;
    chunks.push(Chunk { id, pos: data_pos, len });
    let next = data_pos.saturating_add(len);
    if next <= pos || next > file_size.max(data_pos) {
      break;
    }
    pos = next;
  }
  Ok(chunks)
}

fn find_chunk<'a>(chunks: &'a [Chunk], want: &[u8; 4], require_non_empty: bool) -> Option<&'a Chunk> {
  chunks
    .iter()
    .find(|c| id_eq(&c.id, want) && (!require_non_empty || c.len != 0))
}

/// Parse the fmt chunk body, resolving `WAVE_FORMAT_EXTENSIBLE`.
fn parse_fmt(bytes: &[u8]) -> Option<WaveFormat> {
  if bytes.len() < 16 {
    return None;
  }
  let raw_tag = get_u16_le(&bytes[0..]);
  let channels = get_u16_le(&bytes[2..]);
  let sample_rate = get_u32_le(&bytes[4..]);
  let avg_bytes_per_sec = get_u32_le(&bytes[8..]);
  let block_align = get_u16_le(&bytes[12..]);
  let bits_per_sample = get_u16_le(&bytes[14..]);

  let format_tag = if raw_tag == WAVE_FORMAT_EXTENSIBLE && bytes.len() >= WAVEFORMATEXTENSIBLE_SIZE {
    get_u32_le(&bytes[SUBFORMAT_DATA1_OFFSET..])
  } else {
    raw_tag as u32
  };

  let extra = if bytes.len() >= 18 {
    let cb = get_u16_le(&bytes[16..]) as usize;
    if 18 + cb <= bytes.len() {
      bytes[18..18 + cb].to_vec()
    } else {
      Vec::new()
    }
  } else {
    Vec::new()
  };

  Some(WaveFormat {
    format_tag,
    channels,
    sample_rate,
    avg_bytes_per_sec,
    block_align,
    bits_per_sample,
    extra,
  })
}

/// Full parse over a [`FileSource`]: determine type, scan chunks, read fmt and
/// data. Mirrors `wav_reader_c::parse_file`.
#[cfg(test)]
fn parse_source(src: &mut FileSource) -> Result<Option<WavMetadata>, ParseError> {
  parse_source_with_deadline(src, None)
}

fn parse_source_with_deadline(
  src: &mut FileSource,
  deadline: Option<&Deadline>,
) -> Result<Option<WavMetadata>, ParseError> {
  src.seek_to(0)?;
  let mut head = [0u8; W64_HEADER as usize];
  let n = src.read_at_most(&mut head)?;
  let Some(wav_type) = determine_type(&head[..n]) else {
    return Ok(None);
  };
  let file_size = src.length().unwrap_or(u64::MAX);

  let chunks = match wav_type {
    WavType::Wave | WavType::Rf64 => scan_chunks_riff_with_deadline(src, file_size, deadline)?,
    WavType::Wave64 => scan_chunks_wave64_with_deadline(src, file_size, deadline)?,
  };

  // RF64: ds64 carries the real data size when the data chunk len is 0xFFFFFFFF.
  let ds64_data_size = if wav_type == WavType::Rf64 {
    if let Some(ds64) = find_chunk(&chunks, b"ds64", false) {
      if ds64.len >= 24 {
        src.seek_to(ds64.pos)?;
        let mut buf = [0u8; 24];
        if src.read_at_most(&mut buf)? == 24 {
          let _riff_size = get_u64_le(&buf[0..]);
          Some(get_u64_le(&buf[8..]))
        } else {
          None
        }
      } else {
        return Err(ParseError::Malformed {
          format: "wav",
          offset: ds64.pos,
          reason: "RF64 ds64 chunk is shorter than 24 bytes".to_string(),
        });
      }
    } else {
      return Err(ParseError::Malformed {
        format: "wav",
        offset: 12,
        reason: "RF64 file is missing mandatory ds64 chunk".to_string(),
      });
    }
  } else {
    None
  };

  let fmt_chunk = find_chunk(&chunks, b"fmt ", false).cloned();
  let Some(fmt_chunk) = fmt_chunk else {
    return Ok(None);
  };
  src.seek_to(fmt_chunk.pos)?;
  let fmt_bytes = src.read_vec_capped(fmt_chunk.len.min(FMT_READ_CAP), FMT_READ_CAP)?;
  let Some(mut format) = parse_fmt(&fmt_bytes) else {
    return Ok(None);
  };

  let first_data_chunk = find_chunk(&chunks, b"data", true).cloned();
  let Some(first_data_chunk) = first_data_chunk else {
    return Ok(None);
  };
  // PARSER-227: mkvmerge accumulates the lengths of *all* `data` chunks
  // (`m_bytes_in_data_chunks += new_chunk.len` in `scan_chunks_wave`), so files
  // with more than one data chunk report the full payload size.  RF64 overrides
  // the total with the ds64 `data_size` (`scan_chunks_rf64` sets
  // `m_bytes_in_data_chunks = m_ds64.data_size`).  The payload prefix is still
  // classified from the first data chunk, mirroring `find_chunk("data", 0,
  // false)` and the demuxer probe at that position.
  let data_bytes = match ds64_data_size {
    Some(ds64) => ds64,
    None => chunks
      .iter()
      .filter(|c| id_eq(&c.id, b"data"))
      .map(|c| c.len)
      .sum(),
  };
  let mut probe = vec![0u8; first_data_chunk.len.min(PAYLOAD_PROBE_CAP) as usize];
  src.seek_to(first_data_chunk.pos)?;
  let probe_len = src.read_at_most(&mut probe)?;
  probe.truncate(probe_len);
  let ac3_probe_ok = ac3::find_frame_sync(&probe).is_some();
  let dts_probe_ok = dts::find_consecutive_headers(&probe, 5).is_some() || dts::detect(&probe).is_some();
  if ac3_probe_ok {
    format.format_tag = 0x2000;
  } else if dts_probe_ok {
    format.format_tag = WAVE_FORMAT_DTS;
  }
  let supported = match format.format_tag {
    0x2000 => ac3_probe_ok,
    WAVE_FORMAT_DTS => dts_probe_ok,
    _ => is_supported_format(format.format_tag),
  };

  Ok(Some(WavMetadata {
    wav_type,
    format,
    data_bytes,
    supported,
  }))
}

fn codec_id_and_name(format_tag: u32) -> (String, &'static str) {
  let name = match format_tag {
    WAVE_FORMAT_PCM => "PCM",
    WAVE_FORMAT_IEEE_FLOAT => "IEEE Float",
    0x0002 => "ADPCM",
    0x0055 => "MP3",
    0x2000 => "AC-3",
    WAVE_FORMAT_DTS => "DTS",
    0x00FF => "AAC",
    _ => "Unknown",
  };
  let id = match format_tag {
    WAVE_FORMAT_PCM => "A_PCM/INT/LIT".to_string(),
    WAVE_FORMAT_IEEE_FLOAT => "A_PCM/FLOAT/IEEE".to_string(),
    0x2000 => "A_AC3".to_string(),
    WAVE_FORMAT_DTS => "A_DTS".to_string(),
    _ => format!("0x{format_tag:04X}"),
  };
  (id, name)
}

fn is_supported_format(format_tag: u32) -> bool {
  matches!(
    format_tag,
    WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT | 0x2000 | WAVE_FORMAT_DTS
  )
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WavReader;

impl Reader for WavReader {
  fn name(&self) -> &'static str {
    "wav"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; W64_HEADER as usize];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    Ok(determine_type(&head[..read]).is_some())
  }

  fn read_headers(
    &self,
    src: &mut FileSource,
    deadline: &Deadline,
    out: &mut MediaMetadata,
  ) -> Result<(), ParseError> {
    let metadata = parse_source_with_deadline(src, Some(deadline))?.ok_or(ParseError::Unrecognised)?;

    out.container.format = ContainerFormat::Wav;
    out.container.recognized = true;
    out.container.supported = metadata.supported;
    if !metadata.supported {
      return Ok(());
    }
    if metadata.format.sample_rate > 0 && metadata.format.block_align > 0 {
      let samples = metadata.data_bytes / metadata.format.block_align as u64;
      let ns = (samples as u128) * 1_000_000_000 / metadata.format.sample_rate as u128;
      out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
    }

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    let audio = AudioTrackProperties {
      channels: if metadata.format.channels == 0 {
        None
      } else {
        Some(metadata.format.channels as u32)
      },
      sampling_frequency: if metadata.format.sample_rate == 0 {
        None
      } else {
        Some(metadata.format.sample_rate as f64)
      },
      bit_depth: if metadata.format.bits_per_sample == 0 {
        None
      } else {
        Some(metadata.format.bits_per_sample as u32)
      },
      ..AudioTrackProperties::default()
    };
    let codec_private = if metadata.format.extra.is_empty() {
      None
    } else {
      Some(CodecPrivate::from_bytes(&metadata.format.extra))
    };
    let (codec_id, codec_name) = codec_id_and_name(metadata.format.format_tag);
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

#[cfg(test)]
fn fmt_pcm(sample_rate: u32, channels: u16, bits: u16) -> Vec<u8> {
  let block_align = channels * bits / 8;
  let mut fmt = Vec::new();
  fmt.extend_from_slice(&(WAVE_FORMAT_PCM as u16).to_le_bytes());
  fmt.extend_from_slice(&channels.to_le_bytes());
  fmt.extend_from_slice(&sample_rate.to_le_bytes());
  fmt.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
  fmt.extend_from_slice(&block_align.to_le_bytes());
  fmt.extend_from_slice(&bits.to_le_bytes());
  fmt
}

#[cfg(test)]
fn riff_wrap(payload_chunks: Vec<(&[u8; 4], Vec<u8>)>) -> Vec<u8> {
  let mut body = Vec::new();
  body.extend_from_slice(b"WAVE");
  for (id, data) in payload_chunks {
    body.extend_from_slice(id);
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&data);
    if data.len() & 1 != 0 {
      body.push(0);
    }
  }
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"RIFF");
  bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
  bytes.extend(body);
  bytes
}

#[cfg(test)]
pub(crate) fn build_wav(sample_rate: u32, channels: u16, bits: u16, data_bytes: u32) -> Vec<u8> {
  riff_wrap(vec![
    (b"fmt ", fmt_pcm(sample_rate, channels, bits)),
    (b"data", vec![0u8; data_bytes as usize]),
  ])
}

#[cfg(test)]
fn build_wav_extensible(subformat_tag: u32, channels: u16, bits: u16) -> Vec<u8> {
  let block_align = channels * bits / 8;
  let mut fmt = Vec::new();
  fmt.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
  fmt.extend_from_slice(&channels.to_le_bytes());
  fmt.extend_from_slice(&48_000u32.to_le_bytes());
  fmt.extend_from_slice(&(48_000 * block_align as u32).to_le_bytes());
  fmt.extend_from_slice(&block_align.to_le_bytes());
  fmt.extend_from_slice(&bits.to_le_bytes());
  fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
  fmt.extend_from_slice(&bits.to_le_bytes()); // wValidBitsPerSample
  fmt.extend_from_slice(&0u32.to_le_bytes()); // dwChannelMask
  fmt.extend_from_slice(&subformat_tag.to_le_bytes()); // GUID data1
  fmt.extend_from_slice(&[0u8; 12]); // rest of GUID
  riff_wrap(vec![(b"fmt ", fmt), (b"data", vec![0u8; 16])])
}

#[cfg(test)]
fn build_wave64(sample_rate: u32, channels: u16, bits: u16, data_bytes: usize) -> Vec<u8> {
  fn w64_chunk(id4: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut guid = [0u8; 16];
    guid[0..4].copy_from_slice(id4);
    // Reuse the wave-suffix bytes for non-riff chunks (id only matters for 4).
    guid[4..].copy_from_slice(&W64_GUID_WAVE[4..]);
    let size = (W64_CHUNK_HEADER as usize + body.len()) as u64;
    let mut out = Vec::new();
    out.extend_from_slice(&guid);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(body);
    out
  }
  let mut bytes = Vec::new();
  bytes.extend_from_slice(&W64_GUID_RIFF);
  // riff size (whole file) — value not validated by the reader.
  bytes.extend_from_slice(&0u64.to_le_bytes());
  bytes.extend_from_slice(&W64_GUID_WAVE);
  bytes.extend(w64_chunk(b"fmt ", &fmt_pcm(sample_rate, channels, bits)));
  bytes.extend(w64_chunk(b"data", &vec![0u8; data_bytes]));
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn parses_riff_wave_pcm() {
    let bytes = build_wav(48_000, 2, 24, 96_000);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.wav_type, WavType::Wave);
    assert_eq!(m.format.sample_rate, 48_000);
    assert_eq!(m.format.channels, 2);
    assert_eq!(m.format.bits_per_sample, 24);
    assert_eq!(m.data_bytes, 96_000);
    assert_eq!(m.format.format_tag, WAVE_FORMAT_PCM);
  }

  // ---- PARSER-020: Wave64 ----------------------------------------------

  #[test]
  fn probe_and_parse_wave64() {
    let bytes = build_wave64(48_000, 2, 16, 64);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(WavReader.probe(&mut s).unwrap());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.wav_type, WavType::Wave64);
    assert_eq!(m.format.sample_rate, 48_000);
    assert_eq!(m.format.channels, 2);
    assert_eq!(m.data_bytes, 64);
  }

  // ---- PARSER-021: WAVEFORMATEXTENSIBLE resolution ----------------------

  #[test]
  fn extensible_resolves_pcm_subformat() {
    let bytes = build_wav_extensible(WAVE_FORMAT_PCM, 6, 24);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    // Resolved to the subformat, not left as 0xFFFE.
    assert_eq!(m.format.format_tag, WAVE_FORMAT_PCM);
    let (id, name) = codec_id_and_name(m.format.format_tag);
    assert_eq!(id, "A_PCM/INT/LIT");
    assert_eq!(name, "PCM");
  }

  #[test]
  fn extensible_resolves_float_subformat() {
    let bytes = build_wav_extensible(WAVE_FORMAT_IEEE_FLOAT, 2, 32);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.format.format_tag, WAVE_FORMAT_IEEE_FLOAT);
  }

  // ---- PARSER-022: late fmt/data after a large chunk --------------------

  #[test]
  fn finds_fmt_and_data_after_large_junk_chunk() {
    // A 64 KiB JUNK chunk before fmt/data — beyond the old 16 KiB window.
    let junk = vec![0xAAu8; 64 * 1024];
    let bytes = riff_wrap(vec![
      (b"JUNK", junk),
      (b"fmt ", fmt_pcm(44_100, 2, 16)),
      (b"data", vec![0u8; 4000]),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.format.sample_rate, 44_100);
    assert_eq!(m.data_bytes, 4000);
  }

  #[test]
  fn scans_past_four_thousand_chunks_before_fmt_and_data() {
    let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
    for _ in 0..4096 {
      chunks.push((b"JUNK", Vec::new()));
    }
    chunks.push((b"fmt ", fmt_pcm(48_000, 2, 16)));
    chunks.push((b"data", vec![0u8; 192_000]));
    let bytes = riff_wrap(chunks);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.format.sample_rate, 48_000);
    assert_eq!(m.data_bytes, 192_000);
  }

  #[test]
  fn odd_length_riff_chunk_padding_is_not_consumed() {
    // mkvtoolnix's `scan_chunks_wave` advances by exactly `len`; it does not
    // skip the RIFF pad byte after an odd-sized chunk, so this padded file loses
    // alignment before `fmt `.
    let bytes = riff_wrap(vec![
      (b"JUNK", vec![1, 2, 3]),
      (b"fmt ", fmt_pcm(48_000, 1, 16)),
      (b"data", vec![0u8; 100]),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(parse_source(&mut s).unwrap().is_none());
  }

  #[test]
  fn rf64_uses_ds64_data_size() {
    let mut ds64 = Vec::new();
    ds64.extend_from_slice(&0u64.to_le_bytes()); // riff_size
    ds64.extend_from_slice(&500_000u64.to_le_bytes()); // data_size
    ds64.extend_from_slice(&0u64.to_le_bytes()); // sample_count
    let mut bytes = riff_wrap(vec![
      (b"ds64", ds64),
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", vec![0u8; 8]),
    ]);
    bytes[0..4].copy_from_slice(b"RF64");
    // Force the data chunk length to 0xFFFFFFFF so the ds64 override is used.
    // Locate the "data" chunk's 4-byte size field and overwrite it.
    let data_id_pos = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes[data_id_pos + 4..data_id_pos + 8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.wav_type, WavType::Rf64);
    assert_eq!(m.data_bytes, 500_000);
  }

  // ---- PARSER-227: accumulate all data chunks --------------------------

  #[test]
  fn accumulates_multiple_data_chunks() {
    // Two data chunks → the byte total is the sum, not just the first chunk.
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", vec![0u8; 96_000]),
      (b"data", vec![0u8; 96_000]),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.data_bytes, 192_000);
  }

  #[test]
  fn duration_covers_all_data_chunks() {
    // 192 000 bytes @ 48 kHz / stereo / 16-bit == 1 second, split across two
    // data chunks.
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", vec![0u8; 96_000]),
      (b"data", vec![0u8; 96_000]),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.properties.duration.unwrap().ns, 1_000_000_000);
  }

  // ---- PARSER-254: >4 GiB data-length repair ---------------------------

  #[test]
  fn scan_chunks_repairs_huge_data_length() {
    // RIFF/WAVE with fmt, a (deliberately short) data chunk, then a trailing
    // non-data chunk. With file_size > 4 GiB the data chunk length is recomputed
    // from the file size; the scan then stops. Driven via scan_chunks_riff so
    // the >4 GiB size can be supplied without allocating a huge buffer.
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", vec![0u8; 8]),
      (b"fact", vec![0u8; 4]),
    ]);
    let file_size = 0x1_0000_0000u64 + 1_000_000;
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let chunks = scan_chunks_riff(&mut s, file_size).unwrap();
    let data = chunks.iter().find(|c| id_eq(&c.id, b"data")).unwrap();
    assert_eq!(data.len, file_size - data.pos);
    // The repair breaks the scan, so the trailing chunk is never recorded.
    assert!(chunks.iter().all(|c| !id_eq(&c.id, b"fact")));
  }

  #[test]
  fn scan_chunks_does_not_repair_small_files() {
    // The same layout under 4 GiB keeps every chunk's declared length.
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", vec![0u8; 8]),
      (b"fact", vec![0u8; 4]),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    let chunks = scan_chunks_riff(&mut s, bytes.len() as u64).unwrap();
    let data = chunks.iter().find(|c| id_eq(&c.id, b"data")).unwrap();
    assert_eq!(data.len, 8);
    assert!(chunks.iter().any(|c| id_eq(&c.id, b"fact")));
  }

  #[test]
  fn rejects_non_wave_payload() {
    let mut bytes = build_wav(48_000, 2, 16, 12);
    bytes[8..12].copy_from_slice(b"AVI ");
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(parse_source(&mut s).unwrap().is_none());
  }

  #[test]
  fn probe_accepts_riff_wave() {
    let bytes = build_wav(48_000, 2, 16, 4);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(WavReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_too_short_riff_wave_header() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF\0\0\0\0WAVE".to_vec()));
    assert!(!WavReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_too_short_rf64_wave_header() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RF64\0\0\0\0WAVE".to_vec()));
    assert!(!WavReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_populates_audio_track_and_duration() {
    let bytes = build_wav(48_000, 2, 16, 192_000); // 1 second @ 48 kHz stereo 16-bit
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(16));
    assert_eq!(out.container.properties.duration.unwrap().ns, 1_000_000_000);
  }

  #[test]
  fn read_headers_marks_unsupported_tags_without_tracks() {
    let mut bytes = build_wav(48_000, 2, 16, 16);
    let fmt_id_pos = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
    bytes[fmt_id_pos + 8..fmt_id_pos + 10].copy_from_slice(&0x0055u16.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("mp3.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_probes_ac3_payload() {
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", crate::media_metadata::audio::ac3::build_ac3_stream(8, 0, 8)),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("ac3.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_AC3");
  }

  #[test]
  fn read_headers_probes_dts_payload() {
    let bytes = riff_wrap(vec![
      (b"fmt ", fmt_pcm(48_000, 2, 16)),
      (b"data", crate::media_metadata::audio::dts::build_dts_stream(6, 2, 13)),
    ]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("dts.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_DTS");
  }

  #[test]
  fn empty_data_chunk_does_not_produce_track() {
    let bytes = riff_wrap(vec![(b"fmt ", fmt_pcm(48_000, 2, 16)), (b"data", Vec::new())]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(parse_source(&mut s).unwrap().is_none());
  }

  #[test]
  fn ac3_format_tag_without_probe_is_unsupported() {
    let mut bytes = build_wav(48_000, 2, 16, 1024);
    let fmt_id_pos = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
    bytes[fmt_id_pos + 8..fmt_id_pos + 10].copy_from_slice(&0x2000u16.to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("bad-ac3.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn dts_format_tag_without_probe_is_unsupported() {
    let mut bytes = build_wav(48_000, 2, 16, 1024);
    let fmt_id_pos = bytes.windows(4).position(|w| w == b"fmt ").unwrap();
    bytes[fmt_id_pos + 8..fmt_id_pos + 10].copy_from_slice(&(WAVE_FORMAT_DTS as u16).to_le_bytes());
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("bad-dts.wav", 0);
    WavReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn codec_table_covers_common_tags() {
    assert_eq!(codec_id_and_name(0x0001).0, "A_PCM/INT/LIT");
    assert_eq!(codec_id_and_name(0x0003).1, "IEEE Float");
    assert_eq!(codec_id_and_name(0x0055).1, "MP3");
    assert_eq!(codec_id_and_name(0x2000).1, "AC-3");
    assert_eq!(codec_id_and_name(WAVE_FORMAT_DTS).0, "A_DTS");
    assert_eq!(codec_id_and_name(0xCAFE).1, "Unknown");
  }
}
