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

//! `stsd` (sample description) box.
//!
//! Layout: 1B version + 3B flags + 4B entry_count + `entry_count` sample
//! entry boxes.  Each entry is a sub-box keyed by FOURCC (e.g. `avc1`,
//! `mp4a`, `hev1`, `vp09`).  Sample entries share a common 8-byte header
//! (`reserved[6] + data_reference_index`), after which the layout depends on
//! the handler type:
//!
//! - Video entries: 16B QuickTime-style preamble + width/height + 8B
//!   resolution + 4B reserved + 2B frame_count + 32B compressor_name
//!   + 2B depth + 2B color_table_id, then child boxes (`avcC`, `hvcC`,
//!   `colr`, `pasp`, `dvcC`, ...).
//! - Audio entries: 8B QuickTime version+revision+vendor + 2B channels
//!   + 2B sample_size + 4B reserved + 4B sample_rate, then v1/v2 extras and
//!   child boxes (`esds`, `dec1` ...).
//!
//! We extract:
//! - Sample entry FOURCC (mapped through `codec::fourcc::lookup`).
//! - Video width/height + depth (bit depth).
//! - Audio sample rate, channels, sample size.
//! - Dispatch into [`crate::media_metadata::mp4::codec_specific`] for every
//!   nested codec-config box.

use crate::media_metadata::codec::fourcc;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::codec_specific;

use crate::media_metadata::mp4::moov::trak::TrackBuilder;

const VIDEO_PREAMBLE_BYTES: u64 = 16; // version+revision+vendor+temporal+spatial
// hres(4) + vres(4) + reserved(4) + frame_count(2) + compressor_name(32)
// + depth(2) + color_table_id(2) = 50
const VIDEO_TAIL_BYTES: u64 = 4 + 4 + 4 + 2 + 32 + 2 + 2;
const AUDIO_PREAMBLE_BYTES: u64 = 8; // version+revision+vendor
const AUDIO_FIXED_BYTES: u64 = 12; // channels+sample_size+reserved+sample_rate

pub fn parse(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  let payload = parent.payload_size().unwrap_or(0);
  if payload < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: parent.start,
      reason: format!("stsd payload {payload} bytes is too small"),
    });
  }
  // 1B version + 3B flags + 4B entry_count = 8B header
  src.skip(4)?;
  let entry_count = src.read_u32_be()?;
  if entry_count == 0 {
    return Ok(());
  }
  // PARSER-198: mkvtoolnix (`handle_stsd_atom`, r_qtmp4.cpp:1370-1394) iterates
  // over EVERY sample-description entry, re-allocating `dmx.stsd` and re-running
  // the per-entry parse each time.  Because each iteration overwrites the
  // demuxer's `stsd` / `priv` / audio-video fields, the LAST entry's values win.
  // We mirror that: parse every entry into the builder so codec private data,
  // dimensions, audio properties and validation behaviour from later entries
  // are not silently dropped.
  let mut entry_idx = 0u32;
  let stop_at = parent.end();
  while entry_idx < entry_count {
    let pos = src.position();
    if let Some(end) = stop_at {
      if pos >= end {
        return Err(ParseError::Malformed {
          format: "mp4",
          offset: parent.start,
          reason: format!("stsd declares {entry_count} entries but only {entry_idx} fit in the payload"),
        });
      }
      if end - pos < 8 {
        return Err(ParseError::Malformed {
          format: "mp4",
          offset: pos,
          reason: format!("stsd entry {entry_idx} is missing its 8-byte sample-entry header"),
        });
      }
    }
    deadline.check("mp4::stsd")?;
    let entry = atom::read_box_header(src)?;
    parse_entry(src, &entry, builder, deadline)?;
    // Advance to next sibling regardless.
    atom::skip_payload(src, &entry)?;
    entry_idx += 1;
  }
  Ok(())
}

/// Reset the per-sample-entry builder fields before parsing one `stsd` entry so
/// that, matching mkvtoolnix's per-entry overwrite of `dmx.stsd` / `priv`, the
/// last entry's values win without state leaking across entries.
///
/// PARSER-229: the codec **identity** (`sample_entry_kind` / `codec_id_str` /
/// `codec_name`) is deliberately *not* reset here — mkvtoolnix keeps the FIRST
/// FourCC and only warns about later differing ones (`handle_audio_stsd_atom`
/// / `handle_video_stsd_atom`, `r_qtmp4.cpp:3007-3013/3091-3099`).  Only the
/// per-entry sample data (dimensions / audio props / codec private / configs),
/// which upstream genuinely overwrites each iteration, is cleared.
fn reset_sample_entry_state(builder: &mut TrackBuilder) {
  builder.video = None;
  builder.audio = None;
  builder.codec_private_hex = None;
  builder.video_codec_config = None;
  builder.audio_codec_config = None;
  builder.audio_format_flags = None;
  builder.esds_object_type = None;
  builder.esds_decoder_specific_len = None;
  builder.block_additions.clear();
}

fn parse_entry(
  src: &mut FileSource,
  entry: &BoxHeader,
  builder: &mut TrackBuilder,
  deadline: &Deadline,
) -> Result<(), ParseError> {
  let payload = entry.payload_size().unwrap_or(0);
  if payload < 8 {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: entry.start,
      reason: format!("sample entry payload {payload} too small"),
    });
  }
  // PARSER-198: clear the per-entry sample data so the last entry's values win
  // (mkvtoolnix re-allocates `dmx.stsd` per entry).
  reset_sample_entry_state(builder);
  // PARSER-229: the codec identity is taken from the FIRST entry only.  A later
  // entry with a differing FourCC is ignored for identity (mkvtoolnix keeps the
  // first FourCC and warns), while its sample data below still wins.
  if builder.codec_id_str.is_none() {
    builder.sample_entry_kind = Some(entry.kind.0);
    let codec_str: String = entry.kind.0.iter().map(|b| *b as char).collect();
    if let Some(catalogue) = fourcc::lookup(&codec_str) {
      builder.codec_name = Some(catalogue.name.to_string());
    }
    builder.codec_id_str = Some(codec_str);
  }
  let retained_sample_entry_kind = builder.sample_entry_kind.unwrap_or(entry.kind.0);

  // Common 8-byte sample entry header
  src.skip(6)?; // reserved
  let _data_ref_index = src.read_u16_be()?;
  let mut bytes_consumed: u64 = 8;

  let handler = builder.handler_type;
  let is_video;
  let is_subtitle;
  match handler {
    Some(h) if &h == b"vide" => {
      is_video = true;
      is_subtitle = false;
      bytes_consumed += parse_video_sample_entry(src, entry, builder, payload, bytes_consumed)?;
    }
    Some(h) if &h == b"soun" => {
      is_video = false;
      is_subtitle = false;
      bytes_consumed += parse_audio_sample_entry(src, entry, builder, payload, bytes_consumed)?;
    }
    Some(h) if matches!(&h, b"sbtl" | b"text" | b"subp") => {
      // Subtitle sample entries have no extra fixed struct beyond the 8-byte
      // header — the remaining bytes after it are the private data.
      is_video = false;
      is_subtitle = true;
    }
    _ => {
      // Unknown handler — leave the cursor where it is.
      is_video = false;
      is_subtitle = false;
    }
  }

  let remaining = payload.saturating_sub(bytes_consumed);

  // PARSER-178: mirror mkvtoolnix's `parse_video_header_priv_atoms`
  // (r_qtmp4.cpp:3329-3344) and `parse_subtitles_header_priv_atoms`
  // (r_qtmp4.cpp:3386-3396).  For a VIDEO sample entry whose codec is not one
  // we parse via dedicated child boxes (avc1/avc3, hvc1/hev1, av01, mp4v,
  // xvid) — or a SUBTITLE sample entry whose fourcc is not `mp4s` — the entire
  // remaining sample-entry payload is preserved verbatim as codec private
  // (`priv.clone(mem, size)`), rather than walked for sub-boxes.
  if is_video && !is_known_priv_video_fourcc(&retained_sample_entry_kind) {
    capture_remaining_as_private(src, entry, builder, bytes_consumed, remaining, deadline.max_element_size())?;
    return Ok(());
  }
  if is_subtitle && retained_sample_entry_kind != *b"mp4s" {
    capture_remaining_as_private(src, entry, builder, bytes_consumed, remaining, deadline.max_element_size())?;
    return Ok(());
  }

  // Walk any remaining bytes as child sample-entry sub-boxes (avcC, hvcC,
  // esds, colr, pasp, dvcC, ...).
  if remaining >= 8 {
    let synthetic = BoxHeader {
      start: entry.payload_start() + bytes_consumed,
      kind: entry.kind,
      header_len: 0,
      total_size: Some(remaining),
    };
    src.seek_to(entry.payload_start() + bytes_consumed)?;
    walk_sample_entry_children(src, &synthetic, deadline, builder)?;
  }
  Ok(())
}

/// Video FOURCCs whose codec-specific child boxes (`avcC`, `hvcC`, `av1C`, …)
/// we parse individually.  Mirrors the `!codec.is(...) && !fourcc.equiv(...)`
/// guard in `r_qtmp4.cpp:3335-3340`: AVC, HEVC, AV1, `mp4v`, `xvid` keep the
/// child-box walk; everything else preserves the raw remaining bytes.
fn is_known_priv_video_fourcc(fourcc: &[u8; 4]) -> bool {
  matches!(
    fourcc,
    b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"av01" | b"mp4v" | b"xvid" | b"XVID"
  )
}

/// PARSER-178: capture the whole remaining sample-entry payload (the bytes
/// after the fixed video/audio/subtitle struct) as codec private data, capped
/// for safety.  Mirrors `priv.clear(); priv.emplace_back(memory_c::clone(mem,
/// size))`.
fn capture_remaining_as_private(
  src: &mut FileSource,
  entry: &BoxHeader,
  builder: &mut TrackBuilder,
  bytes_consumed: u64,
  remaining: u64,
  payload_cap: u64,
) -> Result<(), ParseError> {
  if remaining == 0 {
    return Ok(());
  }
  src.seek_to(entry.payload_start() + bytes_consumed)?;
  let bytes = src.read_vec_capped(remaining, payload_cap)?;
  if !bytes.is_empty() {
    builder.codec_private_hex = Some(codec_specific::hex_encode(&bytes));
  }
  Ok(())
}

fn parse_video_sample_entry(
  src: &mut FileSource,
  entry: &BoxHeader,
  builder: &mut TrackBuilder,
  payload: u64,
  consumed_so_far: u64,
) -> Result<u64, ParseError> {
  let need = VIDEO_PREAMBLE_BYTES + 4 + VIDEO_TAIL_BYTES; // 16 + 4 dims + 48 tail = 68
  if payload < consumed_so_far + need {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: entry.start,
      reason: format!("video sample entry payload too short ({payload} bytes)"),
    });
  }
  src.skip(VIDEO_PREAMBLE_BYTES)?;
  let width = src.read_u16_be()? as u32;
  let height = src.read_u16_be()? as u32;
  src.skip(8 + 4 + 2 + 32)?; // hres+vres+reserved+frame_count+compressor
  let depth = src.read_u16_be()?;
  src.skip(2)?; // color_table_id
  let mut video = VideoTrackProperties::default();
  if width != 0 && height != 0 {
    video.pixel_dimensions = Some(Dimensions2D { width, height });
    video.display_dimensions = builder.display_dimensions().or(Some(Dimensions2D { width, height }));
  }
  if depth != 0 && depth != 24 {
    // Stash QT depth byte as bits-per-channel hint when not the default 24.
    if let Some(color) = video.color.as_mut() {
      color.bits_per_channel = Some(depth as u32);
    } else {
      video.color = Some(crate::media_metadata::model::track_properties_video::ColorMetadata {
        bits_per_channel: Some(depth as u32),
        ..Default::default()
      });
    }
  }
  builder.video = Some(video);
  Ok(VIDEO_PREAMBLE_BYTES + 4 + VIDEO_TAIL_BYTES)
}

fn parse_audio_sample_entry(
  src: &mut FileSource,
  entry: &BoxHeader,
  builder: &mut TrackBuilder,
  payload: u64,
  consumed_so_far: u64,
) -> Result<u64, ParseError> {
  let need = AUDIO_PREAMBLE_BYTES + AUDIO_FIXED_BYTES;
  if payload < consumed_so_far + need {
    return Err(ParseError::Malformed {
      format: "mp4",
      offset: entry.start,
      reason: format!("audio sample entry payload too short ({payload} bytes)"),
    });
  }
  let version = src.read_u16_be()?;
  let _revision = src.read_u16_be()?;
  let _vendor = src.read_u32_be()?;

  let channels;
  let sample_size;
  let sample_rate_hz;
  let bytes;

  if version == 2 {
    // QuickTime version-2 audio sample entry (PARSER-047): the v0
    // channel/sample-size fields are fixed placeholders (3 / 16); the real
    // values live in the explicit float64 + u32 fields that follow.
    let need = AUDIO_PREAMBLE_BYTES + 48;
    if payload < consumed_so_far + need {
      return Err(ParseError::Malformed {
        format: "mp4",
        offset: entry.start,
        reason: format!("v2 audio sample entry payload too short ({payload} bytes)"),
      });
    }
    // always3(2) always16(2) alwaysMinus2(2) always0(2) always65536(4)
    // sizeOfStructOnly(4) = 16 bytes
    src.skip(16)?;
    sample_rate_hz = f64::from_bits(src.read_u64_be()?); // audioSampleRate
    channels = src.read_u32_be()?; // numAudioChannels
    src.skip(4)?; // always7F000000
    sample_size = src.read_u32_be()?; // constBitsPerChannel
    // `lpcm` uses formatSpecificFlags to distinguish float / big-endian /
    // little-endian PCM, matching qtmp4_demuxer_c::determine_codec.
    builder.audio_format_flags = Some(src.read_u32_be()?);
    // constBytesPerAudioPacket(4) + constLPCMFramesPerAudioPacket(4)
    src.skip(8)?;
    bytes = AUDIO_PREAMBLE_BYTES + 48;
  } else {
    channels = src.read_u16_be()? as u32;
    sample_size = src.read_u16_be()? as u32;
    let _compression_id = src.read_u16_be()?;
    let _packet_size = src.read_u16_be()?;
    let sample_rate_fixed = src.read_u32_be()?; // 16.16 fixed-point in v0/v1
    // PARSER-075: mkvtoolnix decodes the 16.16 value as a float and
    // surfaces it as-is.  We preserve the fractional bits and let the
    // f64 carry the precise Hz value instead of discarding the low 16
    // bits with a logical shift.
    sample_rate_hz = sample_rate_fixed as f64 / 65536.0;

    let mut b = AUDIO_PREAMBLE_BYTES + AUDIO_FIXED_BYTES;
    // v1: 16 more bytes (samplesPerPacket, bytesPerPacket, ...).
    if version == 1 {
      let extra = 16u64;
      if payload >= consumed_so_far + b + extra {
        src.skip(extra)?;
        b += extra;
      }
    }
    bytes = b;
  }

  let mut audio = AudioTrackProperties::default();
  if channels != 0 {
    audio.channels = Some(channels);
  }
  if sample_rate_hz > 0.0 {
    audio.sampling_frequency = Some(sample_rate_hz);
  }
  if sample_size != 0 {
    audio.bit_depth = Some(sample_size);
  }
  builder.audio = Some(audio);
  Ok(bytes)
}

fn walk_sample_entry_children(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  // The synthetic header we built has start = parent.payload_start() + offset,
  // header_len = 0, and total_size = remaining bytes. The atom walker uses
  // payload_start() = start + header_len, so this iterates from `start`.
  let end = parent.end();
  let stream_end = src.length();
  while let Some(remaining) = remaining_in_parent(src, end, stream_end) {
    if remaining < 8 {
      break;
    }
    deadline.check("mp4::sample_entry_children")?;
    let child = match atom::read_box_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };
    if let (Some(end_pos), Some(child_end)) = (end, child.end()) {
      if child_end > end_pos {
        break; // malformed; stop quietly
      }
    }
    match &child.kind.0 {
      b"avcC" => codec_specific::avcc::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      b"hvcC" => codec_specific::hvcc::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      // PARSER-077: AV1 codec configuration box.
      b"av1C" => codec_specific::av1c::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      b"esds" => codec_specific::esds::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      b"colr" => codec_specific::colr::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      b"pasp" => codec_specific::pasp::parse(src, &child, builder)?,
      b"dvcC" | b"dvvC" => codec_specific::dvcc::parse_with_cap(src, &child, builder, deadline.max_element_size())?,
      // PARSER-149: the `hvcE` Dolby Vision enhancement-layer config sits on
      // the same parser path as its `dvcC` / `dvvC` siblings
      // (r_qtmp4.cpp:3374-3378), but mkvtoolnix keeps it as opaque
      // block-addition data — it is not a DV configuration record — so we
      // preserve its bytes without fabricating a profile string.
      b"hvcE" => parse_hvce(src, &child, builder, deadline.max_element_size())?,
      // QuickTime nests codec-config atoms (esds, dOps, ...) inside a
      // `wave` container (PARSER-044) — recurse into it.
      b"wave" => walk_sample_entry_children(src, &child, deadline, builder)?,
      // Opus / FLAC private boxes (PARSER-045).
      b"dOps" => parse_dops(src, &child, builder)?,
      b"dfLa" => parse_dfla(src, &child, builder, deadline.max_element_size())?,
      // PARSER-148: ALAC magic cookie (ALACSpecificConfig) — refines the
      // channel / bit-depth / sample-rate placeholders left by the sample
      // entry (r_qtmp4.cpp:3705-3716).
      b"alac" => parse_alac(src, &child, builder)?,
      _ => {}
    }
    atom::skip_payload(src, &child)?;
  }
  Ok(())
}

/// Parse a `dOps` (Opus) box. The box payload is the `OpusSpecificBox` body:
/// Version(1) OutputChannelCount(1) PreSkip(2) InputSampleRate(4)
/// OutputGain(2) ChannelMappingFamily(1) … all stored big-endian in MP4.
/// Opus always decodes at 48 kHz regardless of InputSampleRate.
///
/// PARSER-199: mkvtoolnix (`parse_dops_audio_header_priv_atom`,
/// r_qtmp4.cpp:3217-3243) does not store the raw `dOps` payload as codec
/// private data.  It builds the Matroska/Ogg Opus ID header by prepending the
/// 8-byte `"OpusHead"` magic and converting the pre-skip, input sample rate and
/// output gain fields from MP4 big-endian to the little-endian layout that
/// Opus-in-Ogg / Opus-in-Matroska expects.  It also clears `a_bitdepth`.
fn parse_dops(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, 4096)?;
  if payload.len() < 11 {
    return Ok(());
  }
  let channels = payload[1] as u32;
  let audio = builder.audio.get_or_insert_with(AudioTrackProperties::default);
  if channels != 0 {
    audio.channels = Some(channels);
  }
  audio.sampling_frequency = Some(48_000.0);
  // mkvtoolnix clears the bit depth for Opus.
  audio.bit_depth = None;

  // Build the OpusHead ID header: "OpusHead" + the dOps payload, then byte-swap
  // the multi-byte fields from MP4 big-endian to little-endian.  Offsets here
  // mirror r_qtmp4.cpp's offsets into the `"OpusHead" + payload` buffer: 10/12/16
  // there equal 2/4/8 within the payload.
  let mut opus_head = Vec::with_capacity(8 + payload.len());
  opus_head.extend_from_slice(b"OpusHead");
  opus_head.extend_from_slice(&payload);
  // pre-skip (u16) at payload offset 2 ⇒ buffer offset 10.
  let pre_skip = u16::from_be_bytes([opus_head[10], opus_head[11]]);
  opus_head[10..12].copy_from_slice(&pre_skip.to_le_bytes());
  // input sample rate (u32) at payload offset 4 ⇒ buffer offset 12.
  let input_rate = u32::from_be_bytes([opus_head[12], opus_head[13], opus_head[14], opus_head[15]]);
  opus_head[12..16].copy_from_slice(&input_rate.to_le_bytes());
  // output gain (u16) at payload offset 8 ⇒ buffer offset 16.
  let output_gain = u16::from_be_bytes([opus_head[16], opus_head[17]]);
  opus_head[16..18].copy_from_slice(&output_gain.to_le_bytes());

  builder.codec_private_hex = Some(codec_specific::hex_encode(&opus_head));
  Ok(())
}

/// Parse a `dfLa` (FLAC) box: 4-byte FullBox header + FLAC metadata block
/// chain. The first block is STREAMINFO (sample rate / channels / bit depth).
///
/// PARSER-200: mkvtoolnix (`parse_dfla_audio_header_priv_atom`,
/// r_qtmp4.cpp:3246-3266) skips the four-byte FullBox version/flags header and
/// stores only the FLAC metadata block chain as codec private data — it does
/// not keep the FullBox header.
fn parse_dfla(
  src: &mut FileSource,
  header: &BoxHeader,
  builder: &mut TrackBuilder,
  payload_cap: u64,
) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, payload_cap)?;
  if payload.len() <= 4 {
    return Ok(());
  }
  // After the 4-byte FullBox header: metadata block header (4) + STREAMINFO.
  let body = &payload[4..];
  builder.codec_private_hex = Some(codec_specific::hex_encode(body));
  if body.len() < 4 + 34 {
    return Ok(());
  }
  let info = &body[4..4 + 34];
  // STREAMINFO: bytes 10..18 pack sample_rate(20) channels(3) bits(5) ...
  let packed = u64::from_be_bytes([
    info[10], info[11], info[12], info[13], info[14], info[15], info[16], info[17],
  ]);
  let sample_rate = ((packed >> 44) & 0xF_FFFF) as f64;
  let channels = (((packed >> 41) & 0x07) + 1) as u32;
  let bits = (((packed >> 36) & 0x1F) + 1) as u32;
  let audio = builder.audio.get_or_insert_with(AudioTrackProperties::default);
  if sample_rate > 0.0 {
    audio.sampling_frequency = Some(sample_rate);
  }
  audio.channels = Some(channels);
  audio.bit_depth = Some(bits);
  Ok(())
}

/// Parse an `alac` magic cookie (ALACSpecificConfig).  PARSER-148.  The box is
/// a FullBox (4-byte version+flags) followed by the 24-byte ALACSpecificConfig:
/// `frameLength(4) compatibleVersion(1) bitDepth(1) pb(1) mb(1) kb(1)
/// numChannels(1) maxRun(2) maxFrameBytes(4) avgBitRate(4) sampleRate(4)`.
/// mkvtoolnix reads channel count, bit depth and sample rate from this cookie
/// (r_qtmp4.cpp:3705-3716) and carries the cookie as codec private data.
///
/// PARSER-201: mkvtoolnix passes only the ALAC magic cookie / config bytes to
/// the ALAC packetizer — `create_audio_packetizer_alac` (r_qtmp4.cpp:1833-1839)
/// clones the buffer starting at `stsd_non_priv_struct_size + 12`, i.e. after
/// the `alac` box header (8) and its FullBox version/flags header (4).  We must
/// store only the ALACSpecificConfig (the FullBox payload), not the whole `alac`
/// atom payload including the four-byte FullBox header.
fn parse_alac(src: &mut FileSource, header: &BoxHeader, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, 4096)?;
  // FullBox header (4) + ALACSpecificConfig (24) = 28 bytes minimum.
  if payload.len() < 28 {
    // Too short to carry a config; preserve whatever follows the FullBox header.
    if payload.len() > 4 {
      builder.codec_private_hex = Some(codec_specific::hex_encode(&payload[4..]));
    }
    return Ok(());
  }
  let cfg = &payload[4..];
  builder.codec_private_hex = Some(codec_specific::hex_encode(cfg));
  let bit_depth = cfg[5] as u32;
  let num_channels = cfg[9] as u32;
  let sample_rate = u32::from_be_bytes([cfg[20], cfg[21], cfg[22], cfg[23]]);
  let audio = builder.audio.get_or_insert_with(AudioTrackProperties::default);
  if num_channels != 0 {
    audio.channels = Some(num_channels);
  }
  if bit_depth != 0 {
    audio.bit_depth = Some(bit_depth);
  }
  if sample_rate != 0 {
    audio.sampling_frequency = Some(sample_rate as f64);
  }
  Ok(())
}

/// Preserve a Dolby Vision enhancement-layer `hvcE` box as a block addition
/// (PARSER-179).  Like `dvcC` / `dvvC`, mkvtoolnix records `hvcE` via
/// `add_data_as_block_addition` (`r_qtmp4.cpp:3377-3378`) — opaque per-frame
/// side data, not the codec-private decoder config.
fn parse_hvce(
  src: &mut FileSource,
  header: &BoxHeader,
  builder: &mut TrackBuilder,
  payload_cap: u64,
) -> Result<(), ParseError> {
  let payload = atom::read_payload(src, header, payload_cap)?;
  let fourcc: String = header.kind.0.iter().map(|b| *b as char).collect();
  builder.block_additions.push((fourcc, payload));
  Ok(())
}

fn remaining_in_parent(src: &FileSource, parent_end: Option<u64>, stream_end: Option<u64>) -> Option<u64> {
  let pos = src.position();
  let p = parent_end.map(|e| e.saturating_sub(pos));
  let s = stream_end.map(|e| e.saturating_sub(pos));
  match (p, s) {
    (Some(a), Some(b)) => Some(a.min(b)),
    (Some(a), None) => Some(a),
    (None, Some(b)) => Some(b),
    (None, None) => None,
  }
}

#[cfg(test)]
pub(crate) fn build_video_sample_entry(
  fourcc_kind: &[u8; 4],
  width: u16,
  height: u16,
  depth: u16,
  children: &[u8],
) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 6]); // reserved
  p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
  // 16-byte QuickTime preamble
  p.extend_from_slice(&[0u8; 16]);
  p.extend_from_slice(&width.to_be_bytes());
  p.extend_from_slice(&height.to_be_bytes());
  p.extend_from_slice(&[0u8; 8]); // hres + vres
  p.extend_from_slice(&[0u8; 4]); // reserved
  p.extend_from_slice(&[0u8; 2]); // frame_count
  p.extend_from_slice(&[0u8; 32]); // compressor name
  p.extend_from_slice(&depth.to_be_bytes());
  p.extend_from_slice(&0u16.to_be_bytes()); // color_table_id
  p.extend_from_slice(children);
  crate::media_metadata::mp4::atom::encode_box(fourcc_kind, &p)
}

#[cfg(test)]
pub(crate) fn build_audio_sample_entry_v0(
  fourcc_kind: &[u8; 4],
  channels: u16,
  sample_size: u16,
  sample_rate_hz: u32,
  children: &[u8],
) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 6]); // reserved
  p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
  p.extend_from_slice(&0u16.to_be_bytes()); // version 0
  p.extend_from_slice(&[0u8; 2 + 4]); // revision + vendor
  p.extend_from_slice(&channels.to_be_bytes());
  p.extend_from_slice(&sample_size.to_be_bytes());
  p.extend_from_slice(&[0u8; 2 + 2]); // compression_id + packet_size
  p.extend_from_slice(&(sample_rate_hz << 16).to_be_bytes());
  p.extend_from_slice(children);
  crate::media_metadata::mp4::atom::encode_box(fourcc_kind, &p)
}

#[cfg(test)]
pub(crate) fn build_audio_sample_entry_v2(
  fourcc_kind: &[u8; 4],
  channels: u32,
  bits: u32,
  sample_rate_hz: f64,
  children: &[u8],
) -> Vec<u8> {
  build_audio_sample_entry_v2_with_flags(fourcc_kind, channels, bits, sample_rate_hz, 0, children)
}

#[cfg(test)]
pub(crate) fn build_audio_sample_entry_v2_with_flags(
  fourcc_kind: &[u8; 4],
  channels: u32,
  bits: u32,
  sample_rate_hz: f64,
  format_specific_flags: u32,
  children: &[u8],
) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 6]); // reserved
  p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
  p.extend_from_slice(&2u16.to_be_bytes()); // version 2
  p.extend_from_slice(&[0u8; 2 + 4]); // revision + vendor
  // v2 fixed placeholders + struct size.
  p.extend_from_slice(&3u16.to_be_bytes());
  p.extend_from_slice(&16u16.to_be_bytes());
  p.extend_from_slice(&(-2i16).to_be_bytes());
  p.extend_from_slice(&0u16.to_be_bytes());
  p.extend_from_slice(&65536u32.to_be_bytes());
  p.extend_from_slice(&72u32.to_be_bytes()); // sizeOfStructOnly
  p.extend_from_slice(&sample_rate_hz.to_bits().to_be_bytes());
  p.extend_from_slice(&channels.to_be_bytes());
  p.extend_from_slice(&0x7F00_0000u32.to_be_bytes());
  p.extend_from_slice(&bits.to_be_bytes());
  p.extend_from_slice(&format_specific_flags.to_be_bytes()); // formatSpecificFlags
  p.extend_from_slice(&0u32.to_be_bytes()); // constBytesPerAudioPacket
  p.extend_from_slice(&0u32.to_be_bytes()); // constLPCMFramesPerAudioPacket
  p.extend_from_slice(children);
  crate::media_metadata::mp4::atom::encode_box(fourcc_kind, &p)
}

#[cfg(test)]
pub(crate) fn build_stsd_payload(entries: &[Vec<u8>]) -> Vec<u8> {
  let mut p = Vec::new();
  p.extend_from_slice(&[0u8; 4]); // version + flags
  p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
  for e in entries {
    p.extend_from_slice(e);
  }
  p
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mp4::atom::encode_box;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
      .step_by(2)
      .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
      .collect()
  }

  fn run(payload: Vec<u8>, handler: [u8; 4]) -> TrackBuilder {
    let stsd = encode_box(b"stsd", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stsd));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(handler);
    parse(&mut s, &parent, &dl(), &mut b).unwrap();
    b
  }

  #[test]
  fn video_avc1_entry_extracts_dims() {
    let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_id_str.as_deref(), Some("avc1"));
    let v = b.video.unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn video_depth_stored_when_not_24() {
    let entry = build_video_sample_entry(b"avc1", 1920, 1080, 32, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    let v = b.video.unwrap();
    assert_eq!(v.color.unwrap().bits_per_channel, Some(32));
  }

  #[test]
  fn audio_mp4a_entry_extracts_channels_and_rate() {
    let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48000, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(16));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(b.codec_id_str.as_deref(), Some("mp4a"));
  }

  #[test]
  fn empty_stsd_does_not_populate() {
    let payload = build_stsd_payload(&[]);
    let b = run(payload, *b"vide");
    assert!(b.video.is_none());
    assert!(b.codec_id_str.is_none());
  }

  #[test]
  fn truncated_payload_rejected() {
    let payload = vec![0u8; 4]; // missing entry_count
    let stsd = encode_box(b"stsd", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stsd));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"vide");
    let err = parse(&mut s, &parent, &dl(), &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn entry_count_over_payload_is_rejected() {
    let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &[]);
    let mut payload = build_stsd_payload(&[entry]);
    payload[4..8].copy_from_slice(&2u32.to_be_bytes());
    let stsd = encode_box(b"stsd", &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(stsd));
    let parent = atom::read_box_header(&mut s).unwrap();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"vide");
    let err = parse(&mut s, &parent, &dl(), &mut b).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn unknown_handler_keeps_codec_str_but_no_typed_subtree() {
    let entry = build_video_sample_entry(b"junk", 100, 100, 24, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"meta");
    assert_eq!(b.codec_id_str.as_deref(), Some("junk"));
    assert!(b.video.is_none());
    assert!(b.audio.is_none());
  }

  #[test]
  fn audio_zero_channels_skipped() {
    let entry = build_audio_sample_entry_v0(b"mp4a", 0, 0, 0, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    assert!(a.channels.is_none());
    assert!(a.sampling_frequency.is_none());
    assert!(a.bit_depth.is_none());
  }

  #[test]
  fn unknown_fourcc_does_not_set_codec_name() {
    let entry = build_video_sample_entry(b"ZZZZ", 100, 100, 24, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_id_str.as_deref(), Some("ZZZZ"));
    assert!(b.codec_name.is_none());
  }

  #[test]
  fn video_entry_with_avcc_child_decodes_codec_config() {
    // Build an avc1 sample entry that carries an embedded avcC child.
    let avcc_payload = crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
      100,
      40,
      3,
      &[&[0u8; 4]],
      &[&[0u8; 2]],
      Some((1, 2, 2)),
    );
    let avcc = encode_box(b"avcC", &avcc_payload);
    let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &avcc);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(100));
    assert_eq!(
      cfg.chroma_format,
      Some(crate::media_metadata::model::track_properties_video::ChromaFormat::Yuv420)
    );
  }

  #[test]
  fn avcc_child_preserves_payload_larger_than_four_kib() {
    let mut avcc_payload = crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
      100,
      40,
      3,
      &[&[0u8; 4]],
      &[&[0u8; 2]],
      Some((1, 2, 2)),
    );
    avcc_payload.extend_from_slice(&vec![0x42u8; 5 * 1024]);
    let avcc = encode_box(b"avcC", &avcc_payload);
    let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &avcc);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_private_hex.as_ref().unwrap().len(), avcc_payload.len() * 2);
    assert_eq!(
      b.video_codec_config.as_ref().unwrap().raw_hex.as_ref().unwrap().len(),
      avcc_payload.len() * 2
    );
  }

  #[test]
  fn hvcc_child_preserves_payload_larger_than_four_kib() {
    let mut hvcc_payload = crate::media_metadata::mp4::codec_specific::hvcc::build_hvcc_payload(
      2, true, 153, 1, 2, 2,
    );
    hvcc_payload.extend_from_slice(&vec![0x24u8; 5 * 1024]);
    let hvcc = encode_box(b"hvcC", &hvcc_payload);
    let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &hvcc);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_private_hex.as_ref().unwrap().len(), hvcc_payload.len() * 2);
    assert_eq!(
      b.video_codec_config.as_ref().unwrap().raw_hex.as_ref().unwrap().len(),
      hvcc_payload.len() * 2
    );
  }

  // ---- PARSER-047: version-2 audio sample entry ------------------------

  #[test]
  fn v2_audio_entry_reads_explicit_channels_and_rate() {
    let entry = build_audio_sample_entry_v2(b"lpcm", 8, 24, 96_000.0, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    // Real values from the v2 fields, not the always-3 / always-16 stubs.
    assert_eq!(a.channels, Some(8));
    assert_eq!(a.sampling_frequency, Some(96_000.0));
    assert_eq!(a.bit_depth, Some(24));
  }

  #[test]
  fn v2_audio_entry_keeps_lpcm_format_flags() {
    let entry = build_audio_sample_entry_v2_with_flags(b"lpcm", 2, 32, 48_000.0, 0x01, &[]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    assert_eq!(b.audio_format_flags, Some(0x01));
  }

  // ---- PARSER-044: wave container recursion ----------------------------

  #[test]
  fn wave_container_nests_esds() {
    let esds_payload = crate::media_metadata::mp4::codec_specific::esds::build_esds_payload(0x40, &[0x12, 0x10]);
    let esds = encode_box(b"esds", &esds_payload);
    let wave = encode_box(b"wave", &esds);
    let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 44_100, &wave);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    // esds nested inside wave is still decoded.
    assert_eq!(b.audio_codec_config.unwrap().aac_object_type, Some(2));
  }

  // ---- PARSER-045: Opus / FLAC private boxes ---------------------------

  #[test]
  fn dops_box_sets_opus_channels_and_48k() {
    // dOps: version(1) channels(1) preskip(2) inputrate(4) gain(2) family(1)
    let mut dops_payload = vec![0u8, 6]; // version 0, 6 channels
    dops_payload.extend_from_slice(&0u16.to_be_bytes()); // preskip
    dops_payload.extend_from_slice(&44_100u32.to_be_bytes()); // input sample rate
    dops_payload.extend_from_slice(&0u16.to_be_bytes()); // gain
    dops_payload.push(0); // mapping family
    let dops = encode_box(b"dOps", &dops_payload);
    let entry = build_audio_sample_entry_v0(b"Opus", 0, 0, 0, &dops);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48_000.0));
  }

  // PARSER-199: the dOps codec private data is rewritten as an OpusHead ID
  // header — "OpusHead" magic prepended, and the pre-skip / input-sample-rate /
  // output-gain fields byte-swapped from MP4 big-endian to little-endian.
  #[test]
  fn dops_box_builds_opushead_with_le_fields() {
    let mut dops_payload = vec![0u8, 2]; // version 0, 2 channels
    dops_payload.extend_from_slice(&312u16.to_be_bytes()); // pre-skip (BE)
    dops_payload.extend_from_slice(&48_000u32.to_be_bytes()); // input sample rate (BE)
    dops_payload.extend_from_slice(&7u16.to_be_bytes()); // output gain (BE)
    dops_payload.push(0); // mapping family
    let dops = encode_box(b"dOps", &dops_payload);
    let entry = build_audio_sample_entry_v0(b"Opus", 0, 16, 0, &dops);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");

    let hex = b.codec_private_hex.clone().unwrap();
    let bytes = hex_to_bytes(&hex);
    // Magic + version + channel count.
    assert_eq!(&bytes[0..8], b"OpusHead");
    assert_eq!(bytes[8], 0); // version
    assert_eq!(bytes[9], 2); // channels
    // Pre-skip / input rate / output gain stored little-endian.
    assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 312);
    assert_eq!(u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]), 48_000);
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 7);
    // mkvtoolnix clears the bit depth for Opus.
    assert!(b.audio.unwrap().bit_depth.is_none());
  }

  #[test]
  fn dfla_box_decodes_flac_streaminfo() {
    // dfLa: 4-byte FullBox header + metadata block header(4) + STREAMINFO(34).
    let mut info = vec![0u8; 34];
    let packed = (44_100u64 << 44) | ((1u64) << 41) | ((15u64) << 36); // 44100, 2ch, 16-bit
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    let mut dfla_payload = vec![0u8; 4]; // FullBox
    dfla_payload.extend_from_slice(&[0x80, 0x00, 0x00, 34]); // last STREAMINFO block header
    dfla_payload.extend_from_slice(&info);
    let dfla = encode_box(b"dfLa", &dfla_payload);
    let entry = build_audio_sample_entry_v0(b"fLaC", 0, 0, 0, &dfla);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    assert_eq!(a.sampling_frequency, Some(44_100.0));
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(16));
    // PARSER-200: codec private excludes the 4-byte FullBox header — it is the
    // FLAC metadata block chain (block header + STREAMINFO).
    let hex = b.codec_private_hex.unwrap();
    let bytes = hex_to_bytes(&hex);
    assert_eq!(bytes.len(), 4 + 34); // block header + STREAMINFO, no FullBox
    assert_eq!(&bytes[0..4], &[0x80, 0x00, 0x00, 34]); // STREAMINFO block header
  }

  #[test]
  fn dfla_box_preserves_payload_larger_than_sixty_four_kib() {
    let mut info = vec![0u8; 34];
    let packed = (48_000u64 << 44) | ((1u64) << 41) | ((15u64) << 36);
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    let mut dfla_payload = vec![0u8; 4]; // FullBox
    dfla_payload.extend_from_slice(&[0x00, 0x00, 0x00, 34]);
    dfla_payload.extend_from_slice(&info);
    dfla_payload.extend_from_slice(&vec![0x5au8; 70 * 1024]);
    let dfla = encode_box(b"dfLa", &dfla_payload);
    let entry = build_audio_sample_entry_v0(b"fLaC", 0, 0, 0, &dfla);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    assert_eq!(b.codec_private_hex.as_ref().unwrap().len(), (dfla_payload.len() - 4) * 2);
    assert_eq!(b.audio.unwrap().sampling_frequency, Some(48_000.0));
  }

  #[test]
  fn audio_entry_with_esds_child_decodes_codec_config() {
    let esds_payload = crate::media_metadata::mp4::codec_specific::esds::build_esds_payload(0x40, &[0x12u8, 0x10]);
    let esds = encode_box(b"esds", &esds_payload);
    let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48_000, &esds);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let cfg = b.audio_codec_config.unwrap();
    assert_eq!(cfg.aac_object_type, Some(2));
  }

  // PARSER-198 / PARSER-229: every stsd entry is parsed; the per-entry sample
  // data (dimensions / audio props / codec private) follows the LAST entry, but
  // the codec **identity** (FourCC) is taken from the FIRST entry — mirroring
  // mkvtoolnix re-allocating `dmx.stsd` each iteration while keeping the first
  // FourCC and only warning about later differing ones
  // (r_qtmp4.cpp:1370-1394 + 3091-3099).
  #[test]
  fn first_fourcc_wins_but_sample_data_is_last() {
    let entry_a = build_video_sample_entry(b"avc1", 1920, 1080, 24, &[]);
    let entry_b = build_video_sample_entry(b"hev1", 3840, 2160, 24, &[]);
    let payload = build_stsd_payload(&[entry_a, entry_b]);
    let b = run(payload, *b"vide");
    // Codec identity is the first FourCC.
    assert_eq!(b.codec_id_str.as_deref(), Some("avc1"));
    assert_eq!(b.sample_entry_kind, Some(*b"avc1"));
    // But the per-entry sample data is the last entry's.
    let v = b.video.unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 3840);
    assert_eq!(v.pixel_dimensions.unwrap().height, 2160);
  }

  #[test]
  fn private_data_decision_uses_retained_first_video_fourcc_for_known_codec() {
    let pasp = encode_box(
      b"pasp",
      &crate::media_metadata::mp4::codec_specific::pasp::build_pasp_payload(4, 3),
    );
    let entry_a = build_video_sample_entry(b"avc1", 640, 480, 24, &[]);
    let entry_b = build_video_sample_entry(b"rle ", 320, 240, 24, &pasp);
    let payload = build_stsd_payload(&[entry_a, entry_b]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_id_str.as_deref(), Some("avc1"));
    assert!(b.codec_private_hex.is_none());
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.sample_aspect_ratio.unwrap().num, 4);
    assert_eq!(b.video.unwrap().pixel_dimensions.unwrap().width, 320);
  }

  #[test]
  fn private_data_decision_uses_retained_first_video_fourcc_for_unknown_codec() {
    let pasp = encode_box(
      b"pasp",
      &crate::media_metadata::mp4::codec_specific::pasp::build_pasp_payload(4, 3),
    );
    let entry_a = build_video_sample_entry(b"rle ", 640, 480, 24, &[]);
    let entry_b = build_video_sample_entry(b"avc1", 320, 240, 24, &pasp);
    let payload = build_stsd_payload(&[entry_a, entry_b]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_id_str.as_deref(), Some("rle "));
    assert!(b.video_codec_config.is_none());
    let expected_hex = codec_specific::hex_encode(&pasp);
    assert_eq!(b.codec_private_hex.as_deref(), Some(expected_hex.as_str()));
    assert_eq!(b.video.unwrap().pixel_dimensions.unwrap().width, 320);
  }

  // A later entry that carries codec-specific child boxes (avcC) must not lose
  // its codec config to an earlier entry that lacked one — PARSER-198 ensures
  // every entry is parsed and the last one's private data survives.
  #[test]
  fn later_entry_codec_private_not_dropped() {
    let avcc_payload = crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
      100,
      40,
      3,
      &[&[0u8; 4]],
      &[&[0u8; 2]],
      Some((1, 2, 2)),
    );
    let avcc = encode_box(b"avcC", &avcc_payload);
    let entry_a = build_video_sample_entry(b"avc1", 640, 480, 24, &[]);
    let entry_b = build_video_sample_entry(b"avc1", 1280, 720, 24, &avcc);
    let payload = build_stsd_payload(&[entry_a, entry_b]);
    let b = run(payload, *b"vide");
    // The second entry's avcC config is present, not dropped.
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(100));
    assert_eq!(b.video.unwrap().pixel_dimensions.unwrap().width, 1280);
  }

  #[test]
  fn video_entry_with_pasp_child_records_aspect() {
    let pasp = encode_box(
      b"pasp",
      &crate::media_metadata::mp4::codec_specific::pasp::build_pasp_payload(40, 33),
    );
    let entry = build_video_sample_entry(b"avc1", 720, 480, 24, &pasp);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    let cfg = b.video_codec_config.unwrap();
    let par = cfg.sample_aspect_ratio.unwrap();
    assert_eq!(par.num, 40);
    assert_eq!(par.den, 33);
  }

  #[test]
  fn video_entry_with_colr_child_decodes_colour() {
    let colr_payload = crate::media_metadata::mp4::codec_specific::colr::build_nclx_payload(9, 16, 9, true);
    let colr = encode_box(b"colr", &colr_payload);
    let entry = build_video_sample_entry(b"avc1", 3840, 2160, 24, &colr);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    let v = b.video.unwrap();
    let c = v.color.unwrap();
    assert_eq!(c.primaries, Some(9));
    assert_eq!(c.matrix_coefficients, Some(9));
  }

  // ---- PARSER-148: ALAC magic cookie -----------------------------------

  fn build_alac_config(channels: u8, bit_depth: u8, sample_rate: u32) -> Vec<u8> {
    let mut cfg = vec![0u8; 4]; // FullBox version+flags
    let mut spec = vec![0u8; 24];
    spec[5] = bit_depth; // bitDepth
    spec[9] = channels; // numChannels
    spec[20..24].copy_from_slice(&sample_rate.to_be_bytes());
    cfg.extend(spec);
    encode_box(b"alac", &cfg)
  }

  #[test]
  fn alac_cookie_refines_channels_bitdepth_and_rate() {
    // Sample entry leaves placeholder channels/bits; the ALAC cookie carries
    // the real values (6 channels, 24-bit, 96 kHz).
    let alac = build_alac_config(6, 24, 96_000);
    let entry = build_audio_sample_entry_v0(b"alac", 2, 16, 44_100, &alac);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"soun");
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.bit_depth, Some(24));
    assert_eq!(a.sampling_frequency, Some(96_000.0));
    // PARSER-201: codec private is the ALAC magic cookie / ALACSpecificConfig
    // ONLY — the 4-byte FullBox version/flags header is stripped.
    let hex = b.codec_private_hex.unwrap();
    let bytes = hex_to_bytes(&hex);
    assert_eq!(bytes.len(), 24); // 24-byte ALACSpecificConfig, no FullBox header
    assert_eq!(bytes[5], 24); // bitDepth
    assert_eq!(bytes[9], 6); // numChannels
    assert_eq!(u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]), 96_000);
  }

  // ---- PARSER-178: unknown video / subtitle private data preserved -----

  // r_qtmp4.cpp:3335-3344: a video sample entry whose codec is not
  // AVC/HEVC/AV1 and whose fourcc is not mp4v/xvid keeps the WHOLE remaining
  // sample-entry payload as codec private.
  #[test]
  fn unknown_video_fourcc_preserves_trailing_private_bytes() {
    // A QuickTime codec (`rle `) with trailing private bytes after the
    // 68-byte video struct.
    let entry = build_video_sample_entry(b"rle ", 320, 240, 24, &[0xDE, 0xAD, 0xBE, 0xEF, 0x01]);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_id_str.as_deref(), Some("rle "));
    assert_eq!(b.codec_private_hex.as_deref(), Some("deadbeef01"));
    // The video struct dims still parse.
    assert_eq!(b.video.unwrap().pixel_dimensions.unwrap().width, 320);
  }

  #[test]
  fn unknown_video_fourcc_preserves_large_trailing_private_bytes() {
    let private = vec![0xabu8; 70 * 1024];
    let entry = build_video_sample_entry(b"rle ", 320, 240, 24, &private);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.codec_private_hex.as_ref().unwrap().len(), private.len() * 2);
    assert_eq!(b.video.unwrap().pixel_dimensions.unwrap().height, 240);
  }

  // Recognised video codecs still walk their dedicated child boxes rather
  // than swallowing them as raw private bytes.
  #[test]
  fn known_video_fourcc_still_walks_child_boxes() {
    let pasp = encode_box(
      b"pasp",
      &crate::media_metadata::mp4::codec_specific::pasp::build_pasp_payload(40, 33),
    );
    let entry = build_video_sample_entry(b"avc1", 720, 480, 24, &pasp);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.sample_aspect_ratio.unwrap().num, 40);
  }

  // r_qtmp4.cpp:3392-3396: a non-`mp4s` subtitle sample entry preserves all
  // remaining bytes as priv.
  #[test]
  fn non_mp4s_subtitle_preserves_private_bytes() {
    // Build a non-mp4s subtitle sample entry: 8-byte header + private bytes.
    let mut e = Vec::new();
    e.extend_from_slice(&[0u8; 6]); // reserved
    e.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    e.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // private bytes
    let entry = encode_box(b"tx3g", &e);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"sbtl");
    assert_eq!(b.codec_id_str.as_deref(), Some("tx3g"));
    assert_eq!(b.codec_private_hex.as_deref(), Some("11223344"));
  }

  // An `mp4s` subtitle entry keeps the child-box walk (its esds is decoded).
  #[test]
  fn mp4s_subtitle_walks_children() {
    let esds_payload = crate::media_metadata::mp4::codec_specific::esds::build_esds_payload(0x40, &[0x12, 0x10]);
    let esds = encode_box(b"esds", &esds_payload);
    let mut e = Vec::new();
    e.extend_from_slice(&[0u8; 6]); // reserved
    e.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    e.extend_from_slice(&esds);
    let entry = encode_box(b"mp4s", &e);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"subt");
    // The mp4s esds object type was decoded, not swallowed as raw bytes.
    assert!(b.esds_object_type.is_some());
  }

  // ---- PARSER-179: hvcE / dvcC preserved as block additions ------------

  // r_qtmp4.cpp:3377-3378 records hvcE via add_data_as_block_addition.
  #[test]
  fn hvce_box_recorded_as_block_addition() {
    let hvce = encode_box(b"hvcE", &[0x01, 0x02, 0x03, 0x04, 0x05]);
    let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &hvce);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    // hvcE bytes preserved as a block addition; no codec private, no DV config.
    assert!(b.codec_private_hex.is_none());
    assert!(b.video_codec_config.is_none());
    assert_eq!(b.block_additions.len(), 1);
    assert_eq!(b.block_additions[0].0, "hvcE");
    assert_eq!(b.block_additions[0].1, vec![0x01, 0x02, 0x03, 0x04, 0x05]);
  }

  #[test]
  fn hvce_box_preserves_payload_larger_than_sixty_four_kib() {
    let hvce_payload = vec![0x61u8; 70 * 1024];
    let hvce = encode_box(b"hvcE", &hvce_payload);
    let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &hvce);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.block_additions.len(), 1);
    assert_eq!(b.block_additions[0].0, "hvcE");
    assert_eq!(b.block_additions[0].1.len(), hvce_payload.len());
  }

  // r_qtmp4.cpp:3377-3378 records dvcC via add_data_as_block_addition, not as
  // the decoder configuration record.
  #[test]
  fn video_entry_with_dvcc_child_recorded_as_block_addition() {
    let dvcc_payload = crate::media_metadata::mp4::codec_specific::dvcc::build_dvcc_payload(8, 6, true, true, true);
    let dvcc = encode_box(b"dvcC", &dvcc_payload);
    let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &dvcc);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert!(b.video_codec_config.is_none());
    assert_eq!(b.block_additions.len(), 1);
    assert_eq!(b.block_additions[0].0, "dvcC");
    assert_eq!(b.block_additions[0].1, dvcc_payload);
  }

  #[test]
  fn dvcc_child_preserves_payload_larger_than_legacy_small_cap() {
    let mut dvcc_payload = crate::media_metadata::mp4::codec_specific::dvcc::build_dvcc_payload(8, 6, true, true, true);
    dvcc_payload.extend_from_slice(&vec![0x99u8; 1024]);
    let dvcc = encode_box(b"dvcC", &dvcc_payload);
    let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &dvcc);
    let payload = build_stsd_payload(&[entry]);
    let b = run(payload, *b"vide");
    assert_eq!(b.block_additions.len(), 1);
    assert_eq!(b.block_additions[0].0, "dvcC");
    assert_eq!(b.block_additions[0].1, dvcc_payload);
  }
}
