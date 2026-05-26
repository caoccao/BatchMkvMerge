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

//! `MpegPsReader`. Pure-Rust port of `mkvtoolnix/src/input/r_mpeg_ps.cpp`.
//!
//! - Probe scans the first 32 KiB for a leading pack header or a
//!   system-header + packet start-code pair (PARSER-049).
//! - Header parsing walks a bounded probe range up to 10 MiB (PARSER-174),
//!   mirroring mkvtoolnix's `calculate_probe_range(file_size, 10 MiB)` floor.
//! - PES packets are depacketised per stream; Program Stream Map entries are
//!   parsed (PARSER-051) and private-stream-1 substream ids are recorded so
//!   the codec can be resolved later (PARSER-050); the accumulated elementary
//!   payload feeds codec-header decoding (PARSER-052).

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::identify::{self, StreamObservation};
use super::packet::{self, PACK_HEADER, SYSTEM_HEADER, StartCode};
use super::{pes, stream_map};

// PARSER-174: mkvtoolnix's `read_headers` probes packets up to
// `calculate_probe_range(file_size, 10 * 1024 * 1024)`, whose floor is the
// 10 MiB fixed minimum (`r_mpeg_ps.cpp:83-186`).  We mirror that bounded
// window — `read_at_most` only fills what is actually available, so small
// files are unaffected.  The percentage scaling above 10 MiB is intentionally
// NOT implemented: capping at the fixed 10 MiB minimum keeps the scan inside
// the ~1 second deadline contract.
const PROBE_BYTES: usize = 10 * 1024 * 1024;
const PROBE_SCAN: usize = 32 * 1024;
const STREAM_PAYLOAD_CAP: usize = 256 * 1024;
const MAX_STREAMS: usize = 64;
const PACKET_START_CODE: [u8; 4] = [0x00, 0x00, 0x01, PACK_HEADER];

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegPsReader;

/// A start-code byte is a PS packet-layer code (`0xB9`..`0xFF`) rather than an
/// elementary-stream code (`0x00`..`0xB8`, e.g. MPEG slice / sequence headers
/// embedded in a video payload).
fn is_packet_layer(sid: u8) -> bool {
  sid >= 0xB9
}

/// Scan forward for the next packet-layer start code, skipping elementary
/// start codes that appear inside a video payload.
fn next_packet_layer(bytes: &[u8], from: usize) -> usize {
  let mut i = from;
  loop {
    match packet::find_start_code(bytes, i) {
      Some((pos, sid)) if is_packet_layer(sid) => return pos,
      Some((pos, _)) => i = pos + 4,
      None => return bytes.len(),
    }
  }
}

fn bounded_packet_end(bytes: &[u8], pos: usize) -> usize {
  if pos + 6 > bytes.len() {
    return bytes.len();
  }
  let len = u16::from_be_bytes([bytes[pos + 4], bytes[pos + 5]]) as usize;
  (pos + 6 + len).min(bytes.len())
}

fn program_stream_map_end(bytes: &[u8], pos: usize) -> usize {
  if pos + 6 > bytes.len() {
    return bytes.len();
  }
  let len = u16::from_be_bytes([bytes[pos + 4], bytes[pos + 5]]) as usize;
  (pos + 6 + len).min(bytes.len())
}

fn pack_header_end(bytes: &[u8], pos: usize) -> usize {
  if pos + 5 > bytes.len() {
    return bytes.len();
  }
  if (bytes[pos + 4] & 0xC0) == 0x40 {
    if pos + 14 > bytes.len() {
      return bytes.len();
    }
    let stuffing = (bytes[pos + 13] & 0x07) as usize;
    (pos + 14 + stuffing).min(bytes.len())
  } else {
    (pos + 12).min(bytes.len())
  }
}

#[derive(Default)]
struct StreamAcc {
  payload: Vec<u8>,
}

impl Reader for MpegPsReader {
  fn name(&self) -> &'static str {
    "mpeg_ps"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = vec![0u8; PROBE_SCAN];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 4 {
      return Ok(false);
    }
    // Fast path: file begins with a pack header.
    if head[..4] == PACKET_START_CODE {
      return Ok(true);
    }
    // Otherwise require both a system-header and a packet start code within
    // the scan window (mkvtoolnix's fallback).
    let bytes = &head[..read];
    let mut system_header = false;
    let mut packet_start = false;
    let mut i = 0usize;
    while i + 4 <= bytes.len() && (!system_header || !packet_start) {
      if let Some((pos, sid)) = packet::find_start_code(bytes, i) {
        match sid {
          SYSTEM_HEADER => system_header = true,
          PACK_HEADER => packet_start = true,
          _ => {}
        }
        i = pos + 4;
      } else {
        break;
      }
    }
    Ok(system_header && packet_start)
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let read = src.read_at_most(&mut probe)?;
    if read < 4 {
      return Err(ParseError::Unrecognised);
    }
    let bytes = &probe[..read];

    // Insertion-ordered stream table keyed by (stream_id, sub_id).
    let mut order: Vec<(u8, Option<u8>)> = Vec::new();
    let mut streams: HashMap<(u8, Option<u8>), StreamAcc> = HashMap::new();
    let mut psm_types: HashMap<u8, u8> = HashMap::new();

    let mut offset = 0usize;
    let mut iterations = 0usize;
    while let Some((pos, sid)) = packet::find_start_code(bytes, offset) {
      deadline.check("mpeg_ps::reader")?;
      iterations += 1;
      if iterations > 200_000 {
        break;
      }
      if !is_packet_layer(sid) {
        offset = pos + 4;
        continue;
      }
      match StartCode::from_byte(sid) {
        StartCode::ProgramStreamMap => {
          let psm_end = program_stream_map_end(bytes, pos);
          if let Ok(psm) = stream_map::parse(&bytes[pos + 4..psm_end]) {
            for e in psm.entries {
              psm_types.entry(e.elementary_stream_id).or_insert(e.stream_type);
            }
          }
          offset = psm_end.max(pos + 4);
        }
        StartCode::Audio(_) | StartCode::Video(_) | StartCode::PrivateStream1 => {
          let pkt = &bytes[pos..];
          let pkt_len = pes::parse(pkt).map(|h| h.packet_length as usize).unwrap_or(0);
          let payoff = pes::pes_payload_offset(pkt);
          let payload_abs = (pos + payoff).min(bytes.len());
          let pkt_end = if pkt_len > 0 {
            (pos + 6 + pkt_len).min(bytes.len())
          } else {
            next_packet_layer(bytes, payload_abs.max(pos + 4))
          };

          let sub_id = if sid == 0xBD && payload_abs < bytes.len() {
            Some(bytes[payload_abs])
          } else {
            None
          };
          // PARSER-176: for 0xBD private-stream-1 audio substreams, mkvtoolnix
          // reads the 1-byte sub_id and then skips an extra framing header
          // before the elementary payload (`r_mpeg_ps.cpp:443-462`).  The
          // header is 4 bytes for TrueHD/MLP (sub_id 0xB0..=0xBF), else 3
          // bytes, and applies for sub_id in 0x80..=0x8F or 0x98..=0xCF.  The
          // VobSub range (0x20..=0x3F) gets no extra skip.  AC-3/DTS/TrueHD
          // probing scans for a sync word so they tolerate either offset, but
          // LPCM reads a fixed-offset header and needs the correct start.
          let extra_skip = match sub_id {
            Some(s) if (0x80..=0x8F).contains(&s) || (0x98..=0xCF).contains(&s) => {
              if (0xB0..=0xBF).contains(&s) { 4usize } else { 3usize }
            }
            _ => 0usize,
          };
          let data_start = if sid == 0xBD {
            (payload_abs + 1 + extra_skip).min(bytes.len())
          } else {
            payload_abs
          };
          let data_end = pkt_end.min(bytes.len()).max(data_start);

          let key = (sid, sub_id);
          let acc = streams.entry(key).or_insert_with(|| {
            if order.len() < MAX_STREAMS {
              order.push(key);
            }
            StreamAcc::default()
          });
          if acc.payload.len() < STREAM_PAYLOAD_CAP {
            let take = (STREAM_PAYLOAD_CAP - acc.payload.len()).min(data_end.saturating_sub(data_start));
            acc.payload.extend_from_slice(&bytes[data_start..data_start + take]);
          }
          offset = pkt_end.max(pos + 4);
        }
        _ => {
          // PackHeader / SystemHeader / Padding / PrivateStream2 / ...
          offset = match StartCode::from_byte(sid) {
            StartCode::PackHeader => pack_header_end(bytes, pos),
            StartCode::ProgramEnd => pos + 4,
            _ => bounded_packet_end(bytes, pos),
          }
          .max(pos + 4);
        }
      }
    }

    let observations: Vec<StreamObservation> = order
      .into_iter()
      .filter_map(|key| {
        streams.remove(&key).map(|acc| StreamObservation {
          stream_id: key.0,
          sub_id: key.1,
          psm_stream_type: psm_types.get(&key.0).copied(),
          payload: acc.payload,
        })
      })
      .collect();

    identify::finalise(observations, out);
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::model::container::ContainerFormat;
  use crate::media_metadata::model::track::TrackType;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn start_code(stream_id: u8) -> [u8; 4] {
    [0x00, 0x00, 0x01, stream_id]
  }

  fn build_ps(stream_ids: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]); // pack body
    for id in stream_ids {
      bytes.extend_from_slice(&start_code(*id));
      bytes.extend_from_slice(&8u16.to_be_bytes()); // packet length
      bytes.extend_from_slice(&[0u8; 8]);
    }
    bytes
  }

  #[test]
  fn probe_accepts_files_starting_with_pack_header() {
    let bytes = build_ps(&[0xE0]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegPsReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-049: scan-based probe ------------------------------------

  #[test]
  fn probe_accepts_leading_garbage_then_system_and_pack() {
    let mut bytes = vec![0xAA, 0xBB, 0xCC]; // leading junk
    bytes.extend_from_slice(&start_code(SYSTEM_HEADER));
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegPsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_files_without_pack_header() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
    assert!(!MpegPsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_collects_unique_stream_ids() {
    // 0xBD packets with all-zero payload have sub-id 0x00 which is *not*
    // a documented private-stream-1 substream — mkvtoolnix drops them, so
    // we expect just the two unique audio/video stream ids (PARSER-095).
    let bytes = build_ps(&[0xE0, 0xC0, 0xE0, 0xC0, 0xBD]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::MpegPs);
    assert_eq!(out.tracks.len(), 2);
    let kinds: Vec<TrackType> = out.tracks.iter().map(|t| t.track_type).collect();
    assert!(kinds.contains(&TrackType::Video));
    assert!(kinds.contains(&TrackType::Audio));
  }

  #[test]
  fn read_headers_returns_unrecognised_on_empty_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(Vec::<u8>::new()));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    let err = MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn padding_stream_is_ignored() {
    let bytes = build_ps(&[0xBE, 0xE0, 0xBE]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
  }

  // ---- PARSER-050: private-stream-1 substreams -------------------------

  #[test]
  fn private_stream_1_dts_substream_classified() {
    // 0xBD packet whose first payload byte (sub_id) is 0x88 → DTS.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&start_code(0xBD));
    bytes.extend_from_slice(&16u16.to_be_bytes()); // packet length
    // PES header: 2 flag bytes + header_data_length=0, then payload.
    bytes.extend_from_slice(&[0x80, 0x80, 0x00]);
    bytes.push(0x88); // sub_id → DTS
    bytes.extend_from_slice(&[0u8; 12]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.vob", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_DTS");
  }

  // ---- PARSER-094: VC-1 stream id 0xFD --------------------------------

  #[test]
  fn vc1_stream_id_fd_is_collected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&start_code(0xFD));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "V_VC1");
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
  }

  // ---- PARSER-174: bounded 10 MiB probe window -------------------------

  #[test]
  fn read_headers_finds_stream_beyond_64kib() {
    // A video PES whose start code lives well past the old 64 KiB window is
    // now reached because the probe range was widened to 10 MiB.  The filler
    // is 0xFF bytes so it carries no spurious `00 00 01` start codes.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&vec![0xFFu8; 200 * 1024]); // > 64 KiB filler
    bytes.extend_from_slice(&start_code(0xE0));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
  }

  // ---- PARSER-176: LPCM private-stream-1 framing-header skip ------------

  #[test]
  fn lpcm_private_stream_payload_offset_skips_framing_header() {
    // 0xBD packet, sub_id 0xA0 (LPCM).  After the 1-byte sub_id mkvtoolnix
    // skips a 3-byte audio framing header before the elementary payload, so
    // the LPCM header bytes must follow that skip to decode correctly.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&start_code(0xBD));
    bytes.extend_from_slice(&20u16.to_be_bytes()); // packet length
    bytes.extend_from_slice(&[0x80, 0x80, 0x00]); // PES flags + header_data_length=0
    bytes.push(0xA0); // sub_id → LPCM
    bytes.extend_from_slice(&[0x00, 0x00, 0x00]); // 3-byte audio framing header
    // LPCM header: skip 8 bits, then bps=2 (16+2*4=24), freq idx=0 (48000),
    // skip 1 bit, channels = 5+1 = 6.  Byte layout after the 8-bit skip:
    //   bits: bps(2)=10, freq(2)=00, reserved(1)=0, channels(3)=101 -> 0b10000101
    bytes.push(0x00); // emphasis/muse/reserved/frame-number (skipped)
    bytes.push(0b10_00_0_101); // bps=24, freq=48000, channels=6
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.vob", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_PCM/INT/BIG");
    let a = t.properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.bit_depth, Some(24));
  }

  // ---- PARSER-272: MPEG-1 PES optional-header layout -------------------

  #[test]
  fn mpeg1_pts_only_pes_reaches_codec_probe() {
    // An MPEG-1 video PES uses the `0x2x` PTS-only marker, *not* the MPEG-2
    // `header_data_length` byte. The elementary payload (an MPEG sequence
    // header) must therefore be located at the MPEG-1 offset so dimensions
    // decode; the old MPEG-2-only offset would have skipped into the wrong
    // bytes.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    // Sequence header: 0x000001B3 + 720x480 + aspect/frame-rate + padding.
    let seq = [
      0x00, 0x00, 0x01, 0xB3, 0x2D, 0x01, 0xE0, 0x13, 0x00, 0x00, 0x00, 0x00,
    ];
    let pts = [0x21u8, 0x11, 0x11, 0x11, 0x11]; // MPEG-1 PTS-only marker + value
    let pkt_len = (pts.len() + seq.len()) as u16;
    bytes.extend_from_slice(&start_code(0xE0));
    bytes.extend_from_slice(&pkt_len.to_be_bytes());
    bytes.extend_from_slice(&pts);
    bytes.extend_from_slice(&seq);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 720);
    assert_eq!(v.pixel_dimensions.unwrap().height, 480);
  }

  // ---- PARSER-051: Program Stream Map ----------------------------------

  fn psm_packet(entries: &[(u8, u8)]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&start_code(super::super::packet::PROGRAM_STREAM_MAP));
    let mut body = Vec::new();
    body.extend_from_slice(&0u16.to_be_bytes());
    body.push(0x80); // current_next + version
    body.push(0x01); // marker
    body.extend_from_slice(&0u16.to_be_bytes()); // program_stream_info_length
    body.extend_from_slice(&((entries.len() * 4) as u16).to_be_bytes());
    for (stream_type, stream_id) in entries {
      body.push(*stream_type);
      body.push(*stream_id);
      body.extend_from_slice(&0u16.to_be_bytes());
    }
    body.extend_from_slice(&0u32.to_be_bytes()); // CRC
    let len = (body.len() - 2) as u16;
    body[..2].copy_from_slice(&len.to_be_bytes());
    payload.extend_from_slice(&body);
    payload
  }

  #[test]
  fn program_stream_map_overrides_classification() {
    // PSM mapping stream id 0xE0 → stream_type 0x1B (AVC).
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&psm_packet(&[(0x1B, 0xE0)]));
    // A video PES on 0xE0.
    bytes.extend_from_slice(&start_code(0xE0));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "V_MPEG4/ISO/AVC");
  }

  // ---- PARSER-276: packet-body skipping and PSM declared length --------

  #[test]
  fn padding_payload_start_code_is_not_collected() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&start_code(super::super::packet::PADDING));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&start_code(0xE0)); // fake packet start inside padding
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&start_code(0xC0)); // real audio packet after padding
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("padding.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Audio);
  }

  #[test]
  fn psm_ignores_entries_after_declared_length() {
    let mut psm = psm_packet(&[]);
    psm.extend_from_slice(&[0x1B, 0xE0, 0x00, 0x00]); // fake trailing entry

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&start_code(PACK_HEADER));
    bytes.extend_from_slice(&[0u8; 10]);
    bytes.extend_from_slice(&psm);
    bytes.extend_from_slice(&start_code(0xE0));
    bytes.extend_from_slice(&8u16.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 8]);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("psm-trailing.mpg", 0);
    MpegPsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_ne!(out.tracks[0].codec.id, "V_MPEG4/ISO/AVC");
  }
}
