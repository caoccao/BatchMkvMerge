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

use std::collections::HashMap;

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
const DATA_PACKET_SCAN_CAP: usize = 1024 * 1024;

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
    // format_version + num_headers (8 bytes); we don't need them but the
    // .RMF body is part of the chunk, so seek past them.
    src.skip(8)?;

    out.container.format = ContainerFormat::RealMedia;
    out.container.recognized = true;
    out.container.supported = true;

    let mut prop: Option<PropChunk> = None;
    let mut tracks: Vec<MdprChunk> = Vec::new();
    let mut first_packets: HashMap<u16, Vec<u8>> = HashMap::new();

    // Walk top-level chunks until DATA (or EOF).  We only inspect the
    // first bounded DATA packet per stream for header-derived refinements;
    // payload scanning still stays out of the hot path.
    loop {
      deadline.check("realmedia-chunk")?;
      let mut hdr = [0u8; COMMON_HEADER_LEN];
      if src.read_at_most(&mut hdr)? < COMMON_HEADER_LEN {
        break;
      }
      let chunk = match ChunkHeader::parse(&hdr) {
        Some(c) => c,
        None => break,
      };
      if (chunk.size as usize) < COMMON_HEADER_LEN {
        break;
      }
      let payload_len = chunk.size as usize - COMMON_HEADER_LEN;
      let next_pos = src.position() + payload_len as u64;
      if chunk.id == ID_PROP {
        let payload = read_payload(src, payload_len)?;
        prop = PropChunk::parse(&payload);
      } else if chunk.id == ID_CONT {
        let payload = read_payload(src, payload_len)?;
        if let Some(c) = ContChunk::parse(&payload) {
          apply_content_metadata(out, &c);
        }
      } else if chunk.id == ID_MDPR {
        let payload = read_payload(src, payload_len)?;
        if let Some(m) = MdprChunk::parse(&payload) {
          tracks.push(m);
        }
      } else if chunk.id == ID_DATA {
        first_packets = read_first_data_packets(src, payload_len, tracks.len().max(1))?;
        break;
      }
      src.seek_to(next_pos)?;
    }

    if let Some(p) = &prop {
      out.container.properties.duration = Some(DurationValue::from_ns(p.duration_ms as u64 * 1_000_000));
      out.container.properties.bitrate_bps = Some(p.avg_bit_rate as u64);
    }

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
  target_streams: usize,
) -> Result<HashMap<u16, Vec<u8>>, ParseError> {
  let to_read = payload_len.min(DATA_PACKET_SCAN_CAP);
  let mut buf = vec![0u8; to_read];
  src.read_exact(&mut buf)?;
  let mut packets = HashMap::new();
  if buf.len() < 8 {
    return Ok(packets);
  }

  let mut pos = 8usize; // num_packets + next_data_offset
  while pos + 12 <= buf.len() && packets.len() < target_streams {
    let length = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
    if length < 12 || pos + length > buf.len() {
      break;
    }
    let stream_number = u16::from_be_bytes([buf[pos + 4], buf[pos + 5]]);
    packets
      .entry(stream_number)
      .or_insert_with(|| buf[pos + 12..pos + length].to_vec());
    pos += length;
  }
  Ok(packets)
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
        let packet_dims = first_packet.and_then(real_video_dimensions_from_packet);
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
          if let Some(header) = aac::parse_audio_specific_config_bytes(&a.extra_data) {
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
            let cfg: AudioCodecConfig = aac::codec_config_from_header(&header, &a.extra_data);
            codec_name = aac::format_aac_profile(header.profile);
            audio.codec_config = Some(cfg);
          }
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
  match fourcc {
    "14_4" => ("A_REAL/14_4", "RealAudio 14.4"),
    "28_8" => ("A_REAL/28_8", "RealAudio 28.8"),
    "dnet" => ("A_AC3", "AC-3 (RealAudio dnet)"),
    "sipr" => ("A_REAL/SIPR", "Sipro Lab Telecom"),
    "cook" => ("A_REAL/COOK", "Cook (RealAudio G2)"),
    "atrc" => ("A_REAL/ATRC", "Sony ATRAC3"),
    "raac" | "racp" => ("A_AAC", "AAC (RealAudio)"),
    "ralf" => ("A_REAL/LF", "RealAudio Lossless"),
    _ => ("A_REAL/UNKNOWN", "RealAudio"),
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
  fn read_headers_extracts_audio_v5_track_and_promotes_raac_to_aac() {
    let mut blob = build_rmf_header();
    blob.extend(build_prop_chunk(0));
    let mut a_props = build_audio_v5(48_000, 6, 16, b"raac");
    a_props.extend_from_slice(&[0x12, 0x10]);
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
    assert_eq!(audio.sampling_frequency, Some(44_100.0));
    assert_eq!(audio.output_sampling_frequency, Some(44_100.0));
    assert_eq!(audio.channels, Some(2));
    let cfg = audio.codec_config.as_ref().unwrap();
    assert_eq!(cfg.aac_object_type, Some(2));
    assert_eq!(cfg.raw_hex.as_deref(), Some("1210"));
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
