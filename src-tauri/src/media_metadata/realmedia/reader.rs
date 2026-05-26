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

//! RealMediaReader — walks the top-level chunk hierarchy and populates the
//! MediaMetadata model.

use std::collections::{HashMap, HashSet};

use crate::media_metadata::audio::{aac, ac3};
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use crate::media_metadata::reader::Reader;

use super::chunks::{
  COMMON_HEADER_LEN, ChunkHeader, ContChunk, ID_CONT, ID_DATA, ID_MDPR, ID_PROP, MdprChunk, PropChunk, RMF_MAGIC,
};
use super::stream_props::{AudioProps, VideoProps};

const PROBE_BYTES: usize = 4;
const DATA_PACKET_CAPTURE_CAP: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct RealMediaReader;

impl Reader for RealMediaReader {
  fn name(&self) -> &'static str {
    "realmedia"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut buf = [0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut buf)?;
    src.seek_to(0)?;
    Ok(read >= PROBE_BYTES && buf == RMF_MAGIC)
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    // Parse the file header (.RMF object header + format_version + num_headers).
    src.seek_to(0)?;
    let mut head = [0u8; COMMON_HEADER_LEN];
    src.read_exact(&mut head)?;
    let header = ChunkHeader::parse(&head).ok_or(ParseError::Unrecognised)?;
    if header.id != RMF_MAGIC {
      return Err(ParseError::Unrecognised);
    }
    let rmf_size = header.size as usize;
    if rmf_size < COMMON_HEADER_LEN + 8 {
      return Err(ParseError::Malformed {
        format: "realmedia",
        offset: 0,
        reason: format!(".RMF chunk size {rmf_size} is too small"),
      });
    }
    // format_version + num_headers (8 bytes); we don't need them but the
    // .RMF body is part of the chunk, so seek past the whole file header
    // object before walking top-level chunks.
    src.seek_to(header.size as u64)?;

    let mut prop: Option<PropChunk> = None;
    let mut tracks: Vec<MdprChunk> = Vec::new();

    // Walk top-level chunks until DATA (or EOF).  We only inspect the
    // first bounded DATA packet per stream for header-derived refinements;
    // payload scanning still stays out of the hot path.
    let first_packets = loop {
      deadline.check("realmedia-chunk")?;
      let mut hdr = [0u8; COMMON_HEADER_LEN];
      src.read_exact(&mut hdr)?;
      let chunk = ChunkHeader::parse(&hdr).ok_or(ParseError::Malformed {
        format: "realmedia",
        offset: src.position().saturating_sub(COMMON_HEADER_LEN as u64),
        reason: "chunk header is shorter than the common header".to_string(),
      })?;
      if (chunk.size as usize) < COMMON_HEADER_LEN {
        return Err(ParseError::Malformed {
          format: "realmedia",
          offset: src.position().saturating_sub(COMMON_HEADER_LEN as u64),
          reason: format!("chunk {:?} size {} is smaller than its header", chunk.id, chunk.size),
        });
      }
      let payload_len = chunk.size as usize - COMMON_HEADER_LEN;
      let next_pos = src.position() + payload_len as u64;
      if chunk.id == ID_PROP {
        let payload = read_payload(src, payload_len)?;
        prop = Some(PropChunk::parse(&payload).ok_or(ParseError::Malformed {
          format: "realmedia",
          offset: next_pos.saturating_sub(payload_len as u64),
          reason: "PROP chunk is truncated".to_string(),
        })?);
      } else if chunk.id == ID_CONT {
        let payload = read_payload(src, payload_len)?;
        let c = ContChunk::parse(&payload).ok_or(ParseError::Malformed {
          format: "realmedia",
          offset: next_pos.saturating_sub(payload_len as u64),
          reason: "CONT chunk is truncated".to_string(),
        })?;
        apply_content_metadata(out, &c);
      } else if chunk.id == ID_MDPR {
        let payload = read_payload(src, payload_len)?;
        let m = MdprChunk::parse(&payload).ok_or(ParseError::Malformed {
          format: "realmedia",
          offset: next_pos.saturating_sub(payload_len as u64),
          reason: "MDPR chunk is truncated".to_string(),
        })?;
        tracks.push(m);
      } else if chunk.id == ID_DATA {
        break read_first_data_packets(src, payload_len, &tracks, deadline)?;
      } else {
        return Err(ParseError::Malformed {
          format: "realmedia",
          offset: src.position().saturating_sub(COMMON_HEADER_LEN as u64),
          reason: format!("unknown RealMedia chunk {}", String::from_utf8_lossy(&chunk.id)),
        });
      }
      src.seek_to(next_pos)?;
    };

    let Some(p) = &prop else {
      return Err(ParseError::Malformed {
        format: "realmedia",
        offset: src.position(),
        reason: "mandatory PROP chunk was not found before DATA".to_string(),
      });
    };

    out.container.format = ContainerFormat::RealMedia;
    out.container.recognized = true;
    out.container.supported = true;
    out.container.properties.duration = Some(DurationValue::from_ns(p.duration_ms as u64 * 1_000_000));
    out.container.properties.bitrate_bps = Some(p.avg_bit_rate as u64);

    for track in &tracks {
      push_track(
        out,
        track.stream_number as i64,
        track,
        first_packets.get(&track.stream_number).map(Vec::as_slice),
      );
    }
    Ok(())
  }
}

fn read_payload(src: &mut FileSource, len: usize) -> Result<Vec<u8>, ParseError> {
  let mut buf = vec![0u8; len];
  src.read_exact(&mut buf)?;
  Ok(buf)
}

fn read_first_data_packets(
  src: &mut FileSource,
  payload_len: usize,
  tracks: &[MdprChunk],
  deadline: &Deadline,
) -> Result<HashMap<u16, Vec<u8>>, ParseError> {
  if payload_len < 8 {
    return Err(ParseError::Malformed {
      format: "realmedia",
      offset: src.position(),
      reason: format!("DATA chunk payload {payload_len} is shorter than its packet header"),
    });
  }
  let dnet_streams = dnet_stream_numbers(tracks);
  let target_streams = tracks.len();
  let mut packets = HashMap::new();

  // DATA starts with num_packets + next_data_offset. The following packet walk
  // mirrors librmff's frame loop closely enough for identification, but stores
  // only a small prefix of each first packet instead of buffering the chunk.
  src.skip(8)?;
  let mut remaining = payload_len - 8;
  while remaining >= 12 {
    deadline.check("realmedia-data")?;
    let mut header = [0u8; 12];
    src.read_exact(&mut header)?;
    let length = u16::from_be_bytes([header[2], header[3]]) as usize;
    if length < 12 || length > remaining {
      break;
    }
    let stream_number = u16::from_be_bytes([header[4], header[5]]);
    let payload_size = length - 12;
    let capture_len = payload_size.min(DATA_PACKET_CAPTURE_CAP);
    let mut data = vec![0u8; capture_len];
    if capture_len > 0 {
      src.read_exact(&mut data)?;
    }
    if payload_size > capture_len {
      src.skip((payload_size - capture_len) as u64)?;
    }

    if dnet_streams.contains(&stream_number) {
      if dnet_packet_has_bsid(&data) {
        packets.insert(stream_number, data);
      } else {
        packets.entry(stream_number).or_insert(data);
      }
    } else {
      packets.entry(stream_number).or_insert(data);
    }

    remaining -= length;
    if packets.len() >= target_streams && dnet_streams.iter().all(|id| packets.get(id).is_some_and(|p| dnet_packet_has_bsid(p))) {
      break;
    }
  }
  Ok(packets)
}

fn dnet_stream_numbers(tracks: &[MdprChunk]) -> HashSet<u16> {
  tracks
    .iter()
    .filter_map(|track| {
      if track.mime_type == "audio/x-pn-realaudio" {
        let props = AudioProps::parse(&track.type_specific_data)?;
        if fourcc_string(&props.fourcc).eq_ignore_ascii_case("dnet") {
          return Some(track.stream_number);
        }
      }
      None
    })
    .collect()
}

fn dnet_packet_has_bsid(packet: &[u8]) -> bool {
  packet.get(4).is_some()
}

fn apply_content_metadata(out: &mut MediaMetadata, c: &ContChunk) {
  if !c.title.is_empty() {
    out.container.properties.title = Some(c.title.clone());
  }
  if !c.author.is_empty() {
    out.container.properties.writing_app = Some(c.author.clone());
  }
}

fn fourcc_string(fourcc: &[u8; 4]) -> String {
  String::from_utf8_lossy(fourcc).trim_end_matches('\0').to_string()
}

fn push_track(out: &mut MediaMetadata, id: i64, track: &MdprChunk, first_packet: Option<&[u8]>) {
  let mut common = CommonTrackProperties::default();
  common.number = Some(track.stream_number as u64);
  common.stream_id = Some(track.stream_number as u32);
  if !track.stream_name.is_empty() {
    common.track_name = Some(track.stream_name.clone());
  }
  let codec_private = Some(CodecPrivate::from_bytes(&track.type_specific_data));

  match track.mime_type.as_str() {
    "video/x-pn-realvideo" => {
      if let Some(v) = VideoProps::parse(&track.type_specific_data) {
        let fourcc = fourcc_string(&v.fourcc);
        let codec_id = format!("V_REAL/{}", fourcc);
        let header_dims = Dimensions2D {
          width: v.width as u32,
          height: v.height as u32,
        };
        // r_real.cpp:241-242 + :588-590 — only RV40 derives its dimensions
        // from the first packet; RV10/RV20/RV30 keep their header dimensions
        // (mkvtoolnix sets `rv_dimensions = true` for every non-RV40 fourcc
        // and only calls `set_dimensions` while that flag is false).
        let packet_dims = if fourcc == "RV40" {
          first_packet.and_then(real_video_dimensions_from_packet)
        } else {
          None
        };
        let pixel_dims = packet_dims.unwrap_or(header_dims);
        let display_dims =
          if packet_dims.is_some() && header_dims.width > 0 && header_dims.height > 0 && header_dims != pixel_dims {
            Some(header_dims)
          } else if pixel_dims.width > 0 && pixel_dims.height > 0 {
            Some(pixel_dims)
          } else {
            None
          };
        let dims = if pixel_dims.width > 0 && pixel_dims.height > 0 {
          Some(pixel_dims)
        } else {
          None
        };
        let fps = v.fps();
        let default_duration_ns = if fps > 0.0 {
          Some((1_000_000_000.0 / fps).round() as u64)
        } else {
          None
        };
        out.tracks.push(Track {
          id,
          track_type: TrackType::Video,
          codec: CodecInfo {
            id: codec_id,
            name: Some(real_video_display_name(&fourcc)),
            codec_private,
          },
          properties: TrackProperties {
            common,
            video: Some(VideoTrackProperties {
              pixel_dimensions: dims,
              display_dimensions: display_dims,
              default_duration_ns,
              ..VideoTrackProperties::default()
            }),
            ..TrackProperties::default()
          },
        });
      }
    }
    "audio/x-pn-realaudio" => {
      if let Some(a) = AudioProps::parse(&track.type_specific_data) {
        let fourcc = fourcc_string(&a.fourcc);
        let (codec_id, name) = real_audio_codec_id(&fourcc);
        let mut codec_id = codec_id.to_string();
        let mut audio = AudioTrackProperties {
          sampling_frequency: Some(a.sample_rate as f64),
          channels: Some(a.channels as u32),
          bit_depth: Some(a.sample_size as u32),
          ..AudioTrackProperties::default()
        };
        let mut codec_name = name.to_string();
        if fourcc.eq_ignore_ascii_case("dnet") {
          apply_dnet_packet_hints(first_packet, &mut codec_id, &mut codec_name, &mut audio);
        }
        if codec_id == "A_AAC" {
          apply_real_aac_config(&fourcc, &a, &mut audio, &mut codec_name);
        }
        out.tracks.push(Track {
          id,
          track_type: TrackType::Audio,
          codec: CodecInfo {
            id: codec_id,
            name: Some(codec_name),
            codec_private,
          },
          properties: TrackProperties {
            common,
            audio: Some(audio),
            ..TrackProperties::default()
          },
        });
      }
    }
    _ => {
      // Unknown MIME — surface as an Unknown track so the count stays
      // consistent with the container's `num_streams`.
    }
  }
}

fn apply_dnet_packet_hints(
  packet: Option<&[u8]>,
  codec_id: &mut String,
  codec_name: &mut String,
  audio: &mut AudioTrackProperties,
) {
  let Some(packet) = packet else {
    return;
  };
  let mut bsid = packet.get(4).map(|b| b >> 3);
  if let Some(frame) = ac3::decode_frame(packet) {
    bsid = Some(frame.bsid);
    audio.sampling_frequency = Some(frame.sample_rate as f64);
    audio.channels = Some(frame.channels);
    if frame.variant == ac3::Ac3Variant::Eac3 {
      *codec_id = "A_EAC3".to_string();
      *codec_name = "E-AC-3 (RealAudio dnet)".to_string();
    }
  }
  let mut cfg = audio.codec_config.take().unwrap_or_default();
  if let Some(bsid) = bsid {
    cfg.profile_name = Some(format!("BSID {bsid}"));
  }
  cfg.raw_hex = Some(hex_prefix(packet, 18));
  audio.codec_config = Some(cfg);
}

/// Derive AAC parameters for a RealAudio RAAC/RACP track.
///
/// Mirrors `real_reader_c::create_aac_audio_packetizer`
/// (`r_real.cpp:253-292`).  The RealAudio AAC wrapper prefixes the
/// `AudioSpecificConfig` with a 4-byte big-endian length followed by one
/// extra byte, so the ASC begins at `extra_data[4 + 1]` and is
/// `extra_len - 1` bytes long (`r_real.cpp:260-267`).  When no profile can
/// be detected, mkvtoolnix applies an SBR / output-sample-rate fallback for
/// `racp` streams or when the detected sample rate is below 44100
/// (`r_real.cpp:281-287`).
fn apply_real_aac_config(fourcc: &str, a: &AudioProps, audio: &mut AudioTrackProperties, codec_name: &mut String) {
  let mut profile_detected = false;

  // r_real.cpp:260 — require at least the 4-byte length prefix + 1 byte.
  if a.extra_data.len() > 4 {
    let extra_len = u32::from_be_bytes([a.extra_data[0], a.extra_data[1], a.extra_data[2], a.extra_data[3]]) as usize;
    // r_real.cpp:265 — the wrapper must fit inside the extra data.
    if extra_len >= 1 && 4 + extra_len <= a.extra_data.len() {
      // r_real.cpp:266 — ASC at &extra_data[4 + 1], length extra_len - 1.
      let asc = &a.extra_data[5..4 + extra_len];
      if let Some(header) = aac::parse_audio_specific_config_bytes(asc) {
        if header.sample_rate > 0 {
          audio.sampling_frequency = Some(header.sample_rate as f64);
        }
        if header.output_sample_rate > 0 {
          audio.output_sampling_frequency = Some(header.output_sample_rate as f64);
        } else if header.sample_rate > 0 {
          audio.output_sampling_frequency = Some(header.sample_rate as f64);
        }
        if header.channels > 0 {
          audio.channels = Some(header.channels);
        }
        let cfg: AudioCodecConfig = aac::codec_config_from_header(&header, asc);
        *codec_name = aac::format_aac_profile(header.profile);
        audio.codec_config = Some(cfg);
        profile_detected = true;
      }
    }
  }

  if !profile_detected {
    // r_real.cpp:281-287 — fall back to the header parameters and assume
    // SBR for racp streams or when the sample rate is below 44100.
    let sample_rate = a.sample_rate;
    audio.channels = Some(a.channels as u32);
    audio.sampling_frequency = Some(sample_rate as f64);
    if fourcc.eq_ignore_ascii_case("racp") || sample_rate < 44_100 {
      let output_sample_rate = 2 * sample_rate;
      audio.output_sampling_frequency = Some(output_sample_rate as f64);
      // SBR implies AAC profile 4 ("AAC SBR" in our profile table).
      let cfg = AudioCodecConfig {
        profile_name: Some(aac::format_aac_profile(4)),
        aac_sbr_present: Some(true),
        ..AudioCodecConfig::default()
      };
      *codec_name = aac::format_aac_profile(4);
      audio.codec_config = Some(cfg);
    }
  }
}

fn real_video_dimensions_from_packet(packet: &[u8]) -> Option<Dimensions2D> {
  let skip = 1 + 2 * 4 * (packet.first().copied()? as usize + 1);
  if skip + 10 >= packet.len() {
    return None;
  }
  let (width, height) = parse_real_video_dimensions(&packet[skip..])?;
  Some(Dimensions2D { width, height })
}

fn parse_real_video_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
  const CW: [u32; 8] = [160, 176, 240, 320, 352, 640, 704, 0];
  const CH1: [u32; 8] = [120, 132, 144, 240, 288, 480, 0, 0];
  const CH2: [u32; 4] = [180, 360, 576, 0];

  let mut br = BitReader::new(bytes);
  br.skip_bits(13).ok()?;
  br.skip_bits(13).ok()?;
  let mut v = br.read_bits(3).ok()? as usize;
  let mut width = CW[v];
  if width == 0 {
    loop {
      let c = br.read_bits(8).ok()? as u32;
      width = width.saturating_add(c << 2);
      if c != 255 {
        break;
      }
    }
  }

  let mut c = br.read_bits(3).ok()? as usize;
  let mut height = CH1[c];
  if height == 0 {
    v = br.read_bits(1).ok()? as usize;
    c = ((c << 1) | v) & 3;
    height = CH2[c];
    if height == 0 {
      loop {
        let next = br.read_bits(8).ok()? as u32;
        height = height.saturating_add(next << 2);
        if next != 255 {
          break;
        }
      }
    }
  }

  if width > 0 && height > 0 {
    Some((width, height))
  } else {
    None
  }
}

fn hex_prefix(bytes: &[u8], max_len: usize) -> String {
  bytes.iter().take(max_len).map(|b| format!("{:02x}", b)).collect()
}

fn real_video_display_name(fourcc: &str) -> String {
  match fourcc {
    "RV10" => "RealVideo 1".to_string(),
    "RV20" => "RealVideo G2 / 2.0".to_string(),
    "RV30" => "RealVideo 8".to_string(),
    "RV40" => "RealVideo 9 / 10".to_string(),
    "RV60" | "RVHD" => "RealVideo HD".to_string(),
    other => format!("RealVideo ({})", other),
  }
}

fn real_audio_codec_id(fourcc: &str) -> (&'static str, &'static str) {
  if fourcc == "14_4" {
    ("A_REAL/14_4", "RealAudio 14.4")
  } else if fourcc == "28_8" {
    ("A_REAL/28_8", "RealAudio 28.8")
  } else if fourcc == "dnet" {
    ("A_AC3", "AC-3 (RealAudio dnet)")
  } else if fourcc == "sipr" {
    ("A_REAL/SIPR", "Sipro Lab Telecom")
  } else if fourcc.eq_ignore_ascii_case("cook") {
    ("A_REAL/COOK", "Cook (RealAudio G2)")
  } else if fourcc == "atrc" {
    ("A_REAL/ATRC", "Sony ATRAC3")
  } else if fourcc.eq_ignore_ascii_case("raac") || fourcc.eq_ignore_ascii_case("racp") {
    ("A_AAC", "AAC (RealAudio)")
  } else if fourcc == "ralf" {
    ("A_REAL/LF", "RealAudio Lossless")
  } else {
    ("A_REAL/UNKNOWN", "RealAudio")
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::realmedia::chunks::{ID_DATA, build_chunk};
  use crate::media_metadata::realmedia::stream_props::{
    build_audio_v3, build_audio_v4, build_audio_v5, build_video_props,
  };
  use std::io::Cursor;

  fn build_rmf_header() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u32.to_be_bytes()); // format_version
    payload.extend_from_slice(&5u32.to_be_bytes()); // num_headers
    build_chunk(RMF_MAGIC, 0, &payload)
  }

  fn build_prop_chunk(duration_ms: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    for _ in 0..5 {
      payload.extend_from_slice(&0u32.to_be_bytes());
    }
    payload.extend_from_slice(&duration_ms.to_be_bytes());
    for _ in 0..3 {
      payload.extend_from_slice(&0u32.to_be_bytes());
    }
    payload.extend_from_slice(&1u16.to_be_bytes()); // num_streams
    payload.extend_from_slice(&0u16.to_be_bytes()); // flags
    build_chunk(ID_PROP, 0, &payload)
  }

  fn build_mdpr(stream_id: u16, mime: &str, type_specific: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&stream_id.to_be_bytes());
    for _ in 0..7 {
      payload.extend_from_slice(&0u32.to_be_bytes());
    }
    payload.push(0); // stream_name_len
    payload.push(mime.len() as u8);
    payload.extend_from_slice(mime.as_bytes());
    payload.extend_from_slice(&(type_specific.len() as u32).to_be_bytes());
    payload.extend_from_slice(type_specific);
    build_chunk(ID_MDPR, 0, &payload)
  }

  fn build_data_chunk() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u32.to_be_bytes()); // num_packets
    payload.extend_from_slice(&0u32.to_be_bytes()); // next_data_offset
    build_chunk(ID_DATA, 0, &payload)
  }

  fn build_data_chunk_with_packets(packets: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(packets.len() as u32).to_be_bytes());
    payload.extend_from_slice(&0u32.to_be_bytes()); // next_data_offset
    for (stream, data) in packets {
      let length = (12 + data.len()) as u16;
      payload.extend_from_slice(&0u16.to_be_bytes()); // object_version
      payload.extend_from_slice(&length.to_be_bytes());
      payload.extend_from_slice(&stream.to_be_bytes());
      payload.extend_from_slice(&0u32.to_be_bytes()); // timestamp
      payload.push(0); // reserved
      payload.push(0x02); // keyframe
      payload.extend_from_slice(data);
    }
    build_chunk(ID_DATA, 0, &payload)
  }

  /// AAC LC, 48 kHz, stereo AudioSpecificConfig (2 bytes).
  /// Bits: object_type=00010 (LC), sr_index=0011 (48 kHz),
  /// channel_config=0010 (stereo), GA flags=000 -> 0x11 0x90.
  fn build_aac_lc_asc() -> Vec<u8> {
    vec![0x11, 0x90]
  }

  /// Wrap an ASC the way RealAudio does: 4-byte BE length (ASC len + 1),
  /// then 1 unused byte, then the ASC. The ASC begins at byte 5
  /// (mkvtoolnix `r_real.cpp:266` reads `&extra_data[4 + 1]`).
  fn build_real_aac_wrapper(asc: &[u8]) -> Vec<u8> {
    let extra_len = (asc.len() + 1) as u32;
    let mut buf = Vec::new();
    buf.extend_from_slice(&extra_len.to_be_bytes());
    buf.push(0x00); // the 1 byte skipped before the ASC
    buf.extend_from_slice(asc);
    buf
  }

  fn build_real_video_packet_dims(width_code: u8, height_code: u8) -> Vec<u8> {
    let mut packet = vec![0u8; 9];
    let bits = ((width_code as u32) << 3) | height_code as u32;
    packet.extend_from_slice(&bits.to_be_bytes());
    packet.extend_from_slice(&[0u8; 12]);
    packet
  }

  #[test]
  fn probe_accepts_rmf_signature() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(build_rmf_header()));
    assert!(RealMediaReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_random_bytes() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0xAAu8; 32]));
    assert!(!RealMediaReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_video_track_metadata() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(120_000));
    let v_props = build_video_props(b"RV40", 1280, 720, 25.0);
    blob.extend(build_mdpr(7, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::RealMedia);
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert_eq!(out.tracks[0].id, 7);
    assert_eq!(out.tracks[0].properties.common.number, Some(7));
    assert_eq!(out.tracks[0].properties.common.stream_id, Some(7));
    assert_eq!(out.tracks[0].codec.id, "V_REAL/RV40");
    assert_eq!(
      out.tracks[0].codec.codec_private.as_ref().unwrap().length,
      v_props.len() as u64
    );
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1280,
        height: 720
      })
    );
    assert_eq!(v.default_duration_ns, Some(40_000_000));
    let dur = out.container.properties.duration.as_ref().unwrap();
    assert_eq!(dur.ns, 120_000 * 1_000_000);
  }

  #[test]
  fn read_headers_extracts_audio_v4_track() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let a_props = build_audio_v4(44_100, 2, 16, b"cook");
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_REAL/COOK");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(44_100.0));
    assert_eq!(a.channels, Some(2));
  }

  #[test]
  fn read_headers_classifies_uppercase_cook_as_real_cook() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let a_props = build_audio_v4(44_100, 2, 16, b"COOK");
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_REAL/COOK");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("Cook (RealAudio G2)"));
  }

  #[test]
  fn read_headers_extracts_audio_v5_track_and_promotes_raac_to_aac() {
    // The RealAudio AAC wrapper prefixes the ASC with a 4-byte BE length +
    // one byte (mkvtoolnix `r_real.cpp:260-267`); the ASC begins at byte 5.
    let asc = build_aac_lc_asc(); // 48 kHz stereo LC, 2 bytes
    let wrapper = build_real_aac_wrapper(&asc);
    let mut a_props = build_audio_v5(48_000, 6, 16, b"raac");
    // PARSER-269: mkvtoolnix skips four bytes past the v5 props struct before
    // the AAC wrapper (`r_real.cpp:216-217`).
    a_props.extend_from_slice(&[0u8; 4]);
    a_props.extend_from_slice(&wrapper);

    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("AAC LC"));
    assert_eq!(
      out.tracks[0].codec.codec_private.as_ref().unwrap().length,
      a_props.len() as u64
    );
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.sampling_frequency, Some(48_000.0));
    assert_eq!(audio.output_sampling_frequency, Some(48_000.0));
    assert_eq!(audio.channels, Some(2));
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_object_type, Some(2));
    // raw_hex carries only the ASC bytes, not the 4+1-byte wrapper.
    assert_eq!(
      cfg.raw_hex.as_deref(),
      Some(asc.iter().map(|b| format!("{:02x}", b)).collect::<String>().as_str())
    );
  }

  #[test]
  fn read_headers_classifies_uppercase_raac_and_racp_as_aac() {
    for (fourcc, expected_output_rate) in [(b"RAAC", None), (b"RACP", Some(96_000.0))] {
      let mut a_props = build_audio_v5(48_000, 2, 16, fourcc);
      a_props.extend_from_slice(&[0x00]); // no ASC wrapper; RACP still triggers SBR fallback.

      let mut blob = build_rmf_header();
      blob.extend(build_prop_chunk(0));
      blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
      blob.extend(build_data_chunk());

      let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
      let mut out = MediaMetadata::new("clip.ra", 0);
      RealMediaReader
        .read_headers(&mut s, &Deadline::new(60_000), &mut out)
        .unwrap();
      assert_eq!(out.tracks[0].codec.id, "A_AAC");
      let audio = out.tracks[0].properties.audio.as_ref().unwrap();
      assert_eq!(audio.output_sampling_frequency, expected_output_rate);
    }
  }

  #[test]
  fn read_headers_v5_aac_ignores_four_skipped_bytes_before_wrapper() {
    // PARSER-269 regression: the four bytes following the v5 props struct are
    // *not* the AAC wrapper. Seed them with a decoy big-endian length that, if
    // misread as the wrapper, would point ASC parsing at garbage. The real
    // wrapper sits after the skip and must still drive ASC detection.
    let asc = build_aac_lc_asc();
    let wrapper = build_real_aac_wrapper(&asc);
    let mut a_props = build_audio_v5(48_000, 6, 16, b"raac");
    a_props.extend_from_slice(&[0x00, 0x00, 0x00, 0x40]); // decoy length (64)
    a_props.extend_from_slice(&wrapper);

    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    // ASC parsed from the shifted position -> LC profile, not the SBR fallback.
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("AAC LC"));
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.channels, Some(2));
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_object_type, Some(2));
  }

  #[test]
  fn read_headers_aac_falls_back_to_sbr_for_racp() {
    // No valid ASC wrapper (extra_data too short) -> profile undetected.
    // racp fourcc forces the SBR / doubled-output-rate fallback regardless
    // of the header sample rate (mkvtoolnix `r_real.cpp:284-286`).
    let mut a_props = build_audio_v5(48_000, 2, 16, b"racp");
    a_props.extend_from_slice(&[0x00, 0x01]); // < 5 bytes -> no ASC

    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("AAC SBR"));
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.sampling_frequency, Some(48_000.0));
    assert_eq!(audio.output_sampling_frequency, Some(96_000.0));
    assert_eq!(audio.channels, Some(2));
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_sbr_present, Some(true));
  }

  #[test]
  fn read_headers_aac_falls_back_to_sbr_for_low_sample_rate() {
    // raac with a header sample rate below 44100 and no ASC -> SBR fallback.
    let mut a_props = build_audio_v5(22_050, 1, 16, b"raac");
    a_props.extend_from_slice(&[0x00]); // < 5 bytes -> no ASC

    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("AAC SBR"));
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.sampling_frequency, Some(22_050.0));
    assert_eq!(audio.output_sampling_frequency, Some(44_100.0));
    assert_eq!(audio.channels, Some(1));
  }

  #[test]
  fn read_headers_aac_no_fallback_for_raac_at_44100() {
    // raac at exactly 44100 with no ASC -> no SBR fallback (sample rate not
    // below 44100 and fourcc is not racp).
    let mut a_props = build_audio_v5(44_100, 2, 16, b"raac");
    a_props.extend_from_slice(&[0x00]); // < 5 bytes -> no ASC

    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    let audio = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(audio.sampling_frequency, Some(44_100.0));
    assert_eq!(audio.output_sampling_frequency, None);
    assert!(audio.codec_config.is_none());
  }

  #[test]
  fn read_headers_handles_v3_audio_with_hardcoded_codec() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let a_props = build_audio_v3();
    blob.extend(build_mdpr(0, "audio/x-pn-realaudio", &a_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_REAL/14_4");
  }

  #[test]
  fn read_headers_records_content_title() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let mut cont_payload = Vec::new();
    for s in ["My Movie", "Some Author", "©2026", ""] {
      cont_payload.extend_from_slice(&(s.len() as u16).to_be_bytes());
      cont_payload.extend_from_slice(s.as_bytes());
    }
    blob.extend(build_chunk(ID_CONT, 0, &cont_payload));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.properties.title.as_deref(), Some("My Movie"));
    assert_eq!(out.container.properties.writing_app.as_deref(), Some("Some Author"));
  }

  #[test]
  fn read_headers_stops_at_data_chunk() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let v_props = build_video_props(b"RV40", 320, 240, 25.0);
    blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk());
    // Any extra bytes after DATA must not influence identification.
    blob.extend_from_slice(&[0xFFu8; 64]);

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks.len(), 1);
  }

  #[test]
  fn read_headers_rejects_unknown_top_level_chunk() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_chunk(*b"JUNK", 0, &[0u8; 4]));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    let err = RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
    assert!(!out.container.recognized);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_requires_prop_before_data() {
    let mut blob = build_rmf_header();
    let v_props = build_video_props(b"RV40", 320, 240, 25.0);
    blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    let err = RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
    assert!(!out.container.recognized);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_requires_data_chunk() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let v_props = build_video_props(b"RV40", 320, 240, 25.0);
    blob.extend(build_mdpr(0, "video/x-pn-realvideo", &v_props));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    let err = RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
    assert!(!out.container.recognized);
    assert!(out.tracks.is_empty());
  }

  #[test]
  fn read_headers_refines_dnet_from_first_data_packet() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let a_props = build_audio_v4(48_000, 2, 16, b"dnet");
    blob.extend(build_mdpr(3, "audio/x-pn-realaudio", &a_props));
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame_full(0, 8, 11, 2, false);
    blob.extend(build_data_chunk_with_packets(&[(3, frame)]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_EAC3");
    let cfg = out.tracks[0]
      .properties
      .audio
      .as_ref()
      .unwrap()
      .codec_config
      .as_ref()
      .unwrap();
    assert_eq!(cfg.profile_name.as_deref(), Some("BSID 11"));
    assert!(cfg.raw_hex.as_deref().unwrap().starts_with("0b77"));
  }

  #[test]
  fn read_headers_keeps_scanning_until_dnet_bsid_is_found() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let dnet_props = build_audio_v4(48_000, 2, 16, b"dnet");
    let video_props = build_video_props(b"RV40", 320, 240, 25.0);
    blob.extend(build_mdpr(3, "audio/x-pn-realaudio", &dnet_props));
    blob.extend(build_mdpr(4, "video/x-pn-realvideo", &video_props));
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame_full(0, 8, 11, 2, false);
    blob.extend(build_data_chunk_with_packets(&[
      (3, vec![0, 1, 2, 3]),
      (4, build_real_video_packet_dims(5, 5)),
      (3, frame),
    ]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.ra", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let audio = out.tracks.iter().find(|t| t.id == 3).unwrap();
    assert_eq!(audio.codec.id, "A_EAC3");
    let cfg = audio
      .properties
      .audio
      .as_ref()
      .unwrap()
      .codec_config
      .as_ref()
      .unwrap();
    assert_eq!(cfg.profile_name.as_deref(), Some("BSID 11"));
  }

  #[test]
  fn read_headers_refines_realvideo_dimensions_from_first_data_packet() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let v_props = build_video_props(b"RV40", 320, 240, 25.0);
    blob.extend(build_mdpr(4, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk_with_packets(&[(
      4,
      build_real_video_packet_dims(5, 5),
    )]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 640,
        height: 480
      })
    );
    assert_eq!(
      video.display_dimensions,
      Some(Dimensions2D {
        width: 320,
        height: 240
      })
    );
  }

  #[test]
  fn read_headers_keeps_header_dims_for_rv20_despite_packet() {
    // RV20 must NOT derive dimensions from the first packet (mkvtoolnix only
    // enables packet dimensions for RV40 — `r_real.cpp:241-242,588-590`).
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let v_props = build_video_props(b"RV20", 320, 240, 25.0);
    blob.extend(build_mdpr(4, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk_with_packets(&[(
      4,
      build_real_video_packet_dims(5, 5), // would decode to 640x480 for RV40
    )]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 320,
        height: 240
      })
    );
    // Display dims fall back to the (unchanged) header dimensions.
    assert_eq!(
      video.display_dimensions,
      Some(Dimensions2D {
        width: 320,
        height: 240
      })
    );
  }

  #[test]
  fn read_headers_keeps_header_dims_for_rv30_despite_packet() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let v_props = build_video_props(b"RV30", 176, 144, 25.0);
    blob.extend(build_mdpr(4, "video/x-pn-realvideo", &v_props));
    blob.extend(build_data_chunk_with_packets(&[(4, build_real_video_packet_dims(5, 5))]));

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let video = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      video.pixel_dimensions,
      Some(Dimensions2D {
        width: 176,
        height: 144
      })
    );
  }

  #[test]
  fn read_headers_ignores_unknown_mime_types() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    blob.extend(build_mdpr(0, "application/octet-stream", &[]));
    blob.extend(build_data_chunk());

    let mut s = FileSource::from_reader_for_test(Cursor::new(blob));
    let mut out = MediaMetadata::new("clip.rm", 0);
    RealMediaReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.tracks.is_empty());
  }
}
