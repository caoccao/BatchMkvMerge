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

//! Top-level `OggReader`.

use std::collections::{HashMap, HashSet};

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::codecs;
use super::comments;
use super::identify::{self, BitstreamState};
use super::page;

/// Cap the number of pages we walk per parse — protects against pathological
/// streams while still leaving plenty of room to collect VorbisComment blocks
/// from every bitstream.
const MAX_PAGES: usize = 2048;
const PAGE_PAYLOAD_CAP: u64 = 256 * 1024;
/// Cap on the running buffer used to reassemble multi-page packets
/// (PARSER-078).  16 MiB is more than enough for any sane comment block.
const PACKET_REASSEMBLY_CAP: usize = 16 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct OggReader;

impl Reader for OggReader {
  fn name(&self) -> &'static str {
    "ogg"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; 4];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    Ok(read == 4 && &head == b"OggS")
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    src.seek_to(0)?;
    let stream_end = src.length();

    // Map serial → state.  Preserve insertion order via a parallel Vec.
    let mut states: Vec<BitstreamState> = Vec::new();
    let mut serial_to_index: HashMap<u32, usize> = HashMap::new();
    // PARSER-078: per-serial running packet buffer used to reassemble
    // packets that span multiple pages.  `header_count` tracks how many
    // *complete* packets have been observed so we can stop once each
    // bitstream has its identification + comment headers.
    let mut reassembly: HashMap<u32, PacketReassembly> = HashMap::new();
    let mut pages_consumed = 0usize;
    // `true` once the cascade has consumed at least one non-BOS page.
    // mkvtoolnix waits until the BOS run is over before declaring the
    // stream table closed (`r_ogm.cpp:598-633`); without this guard we'd
    // terminate after the first stream's comment block lands but before
    // later BOS pages can introduce additional streams.
    let mut past_bos_run = false;

    loop {
      deadline.check("ogg::reader")?;
      if pages_consumed >= MAX_PAGES {
        break;
      }
      if let Some(end) = stream_end {
        if src.position() >= end {
          break;
        }
      }
      let pos = src.position();
      let header = match page::read_page_header(src) {
        Ok(h) => h,
        Err(ParseError::UnexpectedEof { .. }) => break,
        Err(ParseError::Malformed { .. }) => break,
        Err(e) => return Err(e),
      };
      pages_consumed += 1;

      let payload = page::read_page_payload(src, &header, PAGE_PAYLOAD_CAP)?;
      let is_bos = header.is_beginning_of_stream();
      handle_page(
        &header,
        &payload,
        &mut states,
        &mut serial_to_index,
        &mut reassembly,
        out,
      );
      if !is_bos {
        past_bos_run = true;
      }

      // PARSER-080: stop once every in-use bitstream has its
      // identification + comment headers parsed AND we've moved
      // far enough into the file that no more BOS pages can plausibly
      // arrive.  Mirrors mkvtoolnix's `r_ogm.cpp:598-633` which rewinds
      // once `headers_read` is true for every active stream.  The
      // `pages_consumed > 4` guard keeps non-conformant inputs that
      // sandwich a late BOS page from being truncated by an over-eager
      // early break.
      if past_bos_run && pages_consumed > 4 && all_streams_have_comments(&states) {
        break;
      }

      // Defensive: ensure progress.
      if src.position() <= pos {
        break;
      }
    }

    identify::finalise(states, out);
    Ok(())
  }
}

/// Per-bitstream packet reassembly state (PARSER-078).
#[derive(Default)]
struct PacketReassembly {
  /// Bytes accumulated from continuation segments so far.
  buffer: Vec<u8>,
  /// `true` while the running buffer is the tail of a packet that
  /// `continues_on_next_page`; reset to `false` once we emit a packet.
  pending: bool,
}

fn handle_page(
  header: &page::PageHeader,
  payload: &[u8],
  states: &mut Vec<BitstreamState>,
  serial_to_index: &mut HashMap<u32, usize>,
  reassembly: &mut HashMap<u32, PacketReassembly>,
  out: &mut MediaMetadata,
) {
  if header.is_beginning_of_stream() {
    let idx = states.len();
    let mut state = BitstreamState {
      serial: header.bitstream_serial,
      first_packet: Vec::new(),
      header_packets: Vec::new(),
      metadata: None,
      vorbis_tags: Vec::new(),
      comment_language: None,
      vendor: None,
    };
    // The BOS packet must be wholly contained in this page (per RFC
    // 3533) — reassembly isn't needed for identification packets.
    if let Some(first_span) = header.packet_layout().first() {
      let end = (first_span.bytes as usize).min(payload.len());
      state.first_packet = payload[..end].to_vec();
      state.metadata = codecs::sniff_first_packet(&state.first_packet);
      if state.metadata.is_some() {
        state.header_packets.push(state.first_packet.clone());
      }
    }
    states.push(state);
    serial_to_index.insert(header.bitstream_serial, idx);
    return;
  }

  let Some(&idx) = serial_to_index.get(&header.bitstream_serial) else {
    return;
  };
  if states[idx].metadata.is_none() {
    return;
  }
  // PARSER-078 + PARSER-079: walk *every* packet on the page, threading
  // through the per-serial reassembly buffer so packets that cross page
  // boundaries are reconstructed.
  let layout = header.packet_layout();
  let mut offset = 0usize;
  let entry = reassembly.entry(header.bitstream_serial).or_default();
  for (i, span) in layout.iter().enumerate() {
    let end = (offset + span.bytes as usize).min(payload.len());
    let segment = &payload[offset..end];
    offset = end;
    let is_first_on_page = i == 0;
    if is_first_on_page && !header.is_continuation() && entry.pending {
      // The previous page advertised a continuation but the next page
      // does not flag it — drop the partial packet.
      entry.buffer.clear();
      entry.pending = false;
    }
    if (entry.buffer.len() + segment.len()) <= PACKET_REASSEMBLY_CAP {
      entry.buffer.extend_from_slice(segment);
    } else {
      entry.buffer.clear();
      entry.pending = false;
      return;
    }
    if span.continues_on_next_page {
      entry.pending = true;
      // Wait for the rest of the packet on the next page.
      continue;
    }
    // Complete packet now in `entry.buffer`.
    let packet = std::mem::take(&mut entry.buffer);
    entry.pending = false;
    remember_header_packet(idx, &packet, states);
    // The codec-defined comment header carries tags and chapters.
    try_decode_comment_packet(&packet, idx, states, out);
  }
}

fn remember_header_packet(idx: usize, packet: &[u8], states: &mut [BitstreamState]) {
  let state = &mut states[idx];
  let Some(metadata) = state.metadata.as_ref() else {
    return;
  };
  if state.header_packets.len() < identify::header_packet_target(metadata.codec_id) {
    state.header_packets.push(packet.to_vec());
  }
}

fn try_decode_comment_packet(packet: &[u8], idx: usize, states: &mut [BitstreamState], out: &mut MediaMetadata) {
  let state = &mut states[idx];
  let codec_id = state.metadata.as_ref().map(|m| m.codec_id).unwrap_or_default();
  let Some(decoded) = decode_comment_packet(packet, codec_id) else {
    return;
  };
  state.vendor = Some(decoded.vendor);
  state.comment_language = comments::extract_language(&decoded.entries);
  add_chapters_from_comments(&decoded.entries, out);
  // PARSER-083: convert METADATA_BLOCK_PICTURE comments into attachments
  // before we hand the rest of the tag list to the track.
  let (pictures, remaining): (Vec<_>, Vec<_>) = decoded
    .entries
    .into_iter()
    .partition(|t| t.name.eq_ignore_ascii_case("METADATA_BLOCK_PICTURE"));
  state.vorbis_tags = remaining;
  let mut next_id = (out.attachments.len() as u32) + 1;
  for tag in pictures {
    if let Some(att) = super::comments::metadata_block_picture_to_attachment(&tag.value, next_id) {
      out.attachments.push(att);
      next_id += 1;
    }
  }
}

fn add_chapters_from_comments(entries: &[crate::media_metadata::model::tag::TagEntry], out: &mut MediaMetadata) {
  let mut chapter_ids = HashSet::new();
  for entry in entries {
    if let Some(id) = chapter_timestamp_id(&entry.name) {
      if parse_chapter_timestamp_ns(&entry.value).is_some() {
        chapter_ids.insert(id);
      }
    }
  }
  let count = chapter_ids.len() as u32;
  if count > out.chapters.num_entries {
    out.chapters.num_entries = count;
    out.chapters.num_editions = 1;
  }
}

fn chapter_timestamp_id(name: &str) -> Option<String> {
  let upper = name.to_ascii_uppercase();
  let rest = upper.strip_prefix("CHAPTER")?;
  if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
    return None;
  }
  Some(rest.to_string())
}

fn parse_chapter_timestamp_ns(value: &str) -> Option<u64> {
  let (h, rest) = value.split_once(':')?;
  let (m, rest) = rest.split_once(':')?;
  let (s, frac) = rest
    .split_once('.')
    .or_else(|| rest.split_once(','))
    .unwrap_or((rest, ""));
  let hours: u64 = h.parse().ok()?;
  let minutes: u64 = m.parse().ok()?;
  let seconds: u64 = s.parse().ok()?;
  if minutes >= 60 || seconds >= 60 {
    return None;
  }
  let mut fraction = 0u64;
  let mut scale = 100_000_000u64;
  for b in frac.bytes().take(9) {
    if !b.is_ascii_digit() {
      return None;
    }
    fraction += (b - b'0') as u64 * scale;
    scale /= 10;
  }
  Some(
    hours
      .saturating_mul(3_600_000_000_000)
      .saturating_add(minutes.saturating_mul(60_000_000_000))
      .saturating_add(seconds.saturating_mul(1_000_000_000))
      .saturating_add(fraction),
  )
}

fn decode_comment_packet(packet: &[u8], codec_id: &str) -> Option<comments::VorbisComments> {
  match codec_id {
    "A_VORBIS" => {
      if packet.len() > 7 && packet[0] == 0x03 && &packet[1..7] == b"vorbis" {
        comments::parse(&packet[7..])
      } else {
        None
      }
    }
    "A_OPUS" => {
      if packet.len() > 8 && &packet[..8] == b"OpusTags" {
        comments::parse(&packet[8..])
      } else {
        None
      }
    }
    "V_THEORA" => {
      if packet.len() > 7 && packet[0] == 0x81 && &packet[1..7] == b"theora" {
        comments::parse(&packet[7..])
      } else {
        None
      }
    }
    _ => None,
  }
}

fn all_streams_have_comments(states: &[BitstreamState]) -> bool {
  !states.is_empty()
    && states
      .iter()
      .filter(|s| s.metadata.is_some())
      .all(|s| !s.vorbis_tags.is_empty() || s.vendor.is_some())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::model::container::ContainerFormat;
  use crate::media_metadata::model::track::TrackType;
  use crate::media_metadata::ogg::codecs::{opus, theora, vorbis};
  use crate::media_metadata::ogg::comments::build_block;
  use crate::media_metadata::ogg::page::{HEADER_FLAG_BEGINNING_OF_STREAM, build_page};
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_vorbis_stream(serial: u32, language: Option<&str>) -> Vec<u8> {
    let bos = vorbis::build_identification_packet(2, 44100);
    let page_bos = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, serial, 0, &[&bos]);

    // VorbisComment packet: 0x03 + "vorbis" + comment block + framing bit (0x01).
    let mut comments_pkt = vec![0x03];
    comments_pkt.extend_from_slice(b"vorbis");
    let tags: Vec<(&str, &str)> = match language {
      Some(l) => vec![("TITLE", "Track"), ("LANGUAGE", l)],
      None => vec![("TITLE", "Track")],
    };
    comments_pkt.extend(build_block("libvorbis 1.3.7", &tags));
    comments_pkt.push(0x01); // framing bit
    let page_comments = build_page(0, 0, serial, 1, &[&comments_pkt]);

    let mut bytes = page_bos;
    bytes.extend(page_comments);
    bytes
  }

  #[test]
  fn probe_accepts_ogg_signature() {
    let bytes = build_vorbis_stream(0xCAFE, None);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(OggReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_other_magic() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
    assert!(!OggReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_vorbis_track_with_comments() {
    let bytes = build_vorbis_stream(0xCAFE, Some("fra"));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Ogg);
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.track_type, TrackType::Audio);
    assert_eq!(t.codec.id, "A_VORBIS");
    assert!(t.codec.codec_private.is_none());
    // TITLE + LANGUAGE + VENDOR
    assert_eq!(t.properties.tags.len(), 3);
    let lang = t.properties.common.language.as_ref().unwrap();
    assert_eq!(lang.iso639_2, "fra");
  }

  #[test]
  fn read_headers_handles_opus_stream() {
    let bos = opus::build_identification_packet(2, 48000);
    let mut comments_pkt = b"OpusTags".to_vec();
    comments_pkt.extend(build_block("libopus 1.4", &[("ARTIST", "X")]));
    let page_bos = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    let page_comments = build_page(0, 0, 1, 1, &[&comments_pkt]);
    let mut bytes = page_bos;
    bytes.extend(page_comments);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.opus", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_OPUS");
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    assert_eq!(private.length, bos.len() as u64);
  }

  #[test]
  fn read_headers_preserves_vorbis_header_packet_set() {
    let bos = vorbis::build_identification_packet(2, 44100);
    let mut comments_pkt = vec![0x03];
    comments_pkt.extend_from_slice(b"vorbis");
    comments_pkt.extend(build_block("libvorbis 1.3.7", &[("TITLE", "Track")]));
    comments_pkt.push(0x01);
    let mut setup_pkt = vec![0x05];
    setup_pkt.extend_from_slice(b"vorbis");
    setup_pkt.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
    let page_bos = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    let page_headers = build_page(0, 0, 1, 1, &[&comments_pkt, &setup_pkt]);
    let mut bytes = page_bos;
    bytes.extend(page_headers);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let private = out.tracks[0].codec.codec_private.as_ref().unwrap();
    assert!(private.length > (bos.len() + comments_pkt.len() + setup_pkt.len()) as u64);
    assert!(private.hex.starts_with("02"));
  }

  #[test]
  fn read_headers_converts_chapter_comments_to_summary() {
    let bos = vorbis::build_identification_packet(2, 44100);
    let mut comments_pkt = vec![0x03];
    comments_pkt.extend_from_slice(b"vorbis");
    comments_pkt.extend(build_block(
      "libvorbis 1.3.7",
      &[
        ("CHAPTER01", "00:00:00.000"),
        ("CHAPTER01NAME", "Intro"),
        ("CHAPTER02", "00:01:02.345"),
        ("CHAPTERBAD", "ignored"),
      ],
    ));
    comments_pkt.push(0x01);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comments_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.chapters.num_entries, 2);
    assert_eq!(out.chapters.num_editions, 1);
  }

  #[test]
  fn read_headers_handles_two_independent_streams() {
    let v = build_vorbis_stream(1, None);
    let mut t_bos = vec![0x80];
    t_bos.extend_from_slice(b"theora");
    t_bos.extend(
      theora::build_identification_packet(640, 480, 24, 1)[7..]
        .iter()
        .copied(),
    );
    let theora_full = theora::build_identification_packet(640, 480, 24, 1);
    let theora_page = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 2, 0, &[&theora_full]);
    let mut bytes = v;
    bytes.extend(theora_page);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogv", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.iter().any(|t| t.codec.id == "A_VORBIS"));
    assert!(out.tracks.iter().any(|t| t.codec.id == "V_THEORA"));
  }

  #[test]
  fn malformed_first_page_returns_no_tracks() {
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[b"junk"]);
    // Corrupt the magic.
    bytes[0..4].copy_from_slice(b"FAKE");
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
    // Reader still claims the container as recognised since we got past
    // probe; identify::finalise stamps recognized=true.
    assert_eq!(out.container.format, ContainerFormat::Ogg);
  }

  #[test]
  fn non_bos_page_without_known_serial_is_ignored() {
    // Just two non-BOS pages.
    let p1 = build_page(0, 0, 999, 0, &[b"data"]);
    let p2 = build_page(0, 0, 998, 0, &[b"more"]);
    let mut bytes = p1;
    bytes.extend(p2);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
  }
}
