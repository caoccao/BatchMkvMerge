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

use std::collections::HashMap;

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

      // PARSER-181: stop once every in-use bitstream has read its required
      // header packets (its `headers_read` is satisfied) AND we've moved far
      // enough into the file that no more BOS pages can plausibly arrive.
      // Mirrors mkvtoolnix's `r_ogm.cpp:598-633` which terminates the header
      // read once `headers_read` is true for every active stream — it does
      // NOT wait for decoded comments (FLAC/Speex/Kate/OGM never decode any,
      // so the old `all_streams_have_comments` gate ran to MAX_PAGES and
      // weakened the 1 s contract).  The `pages_consumed > 4` guard keeps
      // non-conformant inputs that sandwich a late BOS page from being
      // truncated by an over-eager early break.
      if past_bos_run && pages_consumed > 4 && all_streams_have_headers(&states) {
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
      headers_complete: false,
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
    // PARSER-180: mkvtoolnix's `handle_stream_comments` (r_ogm.cpp:826-836)
    // parses `packet_data[1]` — the SECOND header packet — as Vorbis
    // comments for every non-FLAC demuxer.  The BOS ident packet is
    // `header_packets[0]`; the first non-BOS complete packet becomes
    // `header_packets[1]`.  We therefore decide whether to decode comments
    // *before* appending, so the comment-decode attempt is restricted to
    // exactly the second header packet.  This matters once prefix detection
    // is loosened: the Vorbis SETUP packet (`0x05vorbis`) also matches the
    // `^.vorbis` prefix and would otherwise be mis-parsed.
    let is_comment_packet = states[idx].header_packets.len() == 1;
    remember_header_packet(idx, &packet, states);
    if is_comment_packet {
      // The second header packet carries tags and chapters.
      try_decode_comment_packet(&packet, idx, states, out);
    }
  }
}

/// Upper bound on header packets collected for variable-length-header codecs
/// (FLAC / Kate).  mkvtoolnix has no fixed cap because it follows the FLAC
/// metadata "last" flag / Kate high-bit run; this keeps the bounded,
/// header-only contract for pathological streams.
const MAX_HEADER_PACKETS: usize = 64;

fn remember_header_packet(idx: usize, packet: &[u8], states: &mut [BitstreamState]) {
  let state = &mut states[idx];
  let Some(metadata) = state.metadata.as_ref() else {
    return;
  };
  if state.headers_complete {
    return;
  }
  match metadata.codec_id.as_str() {
    // PARSER-205: Kate keeps reading header packets while the high bit of the
    // first byte is set (`r_ogm.cpp:1707-1710`).  A high-bit-clear packet ends
    // the header run.
    "S_KATE" => {
      let is_header = packet.first().map(|b| b & 0x80 != 0).unwrap_or(false);
      if is_header && state.header_packets.len() < MAX_HEADER_PACKETS {
        state.header_packets.push(packet.to_vec());
      } else {
        state.headers_complete = true;
      }
    }
    // PARSER-204: FLAC keeps reading header packets until the metadata block
    // with the "last" flag is seen (`r_ogm_flac.cpp:238-244`).
    "A_FLAC" => {
      let is_first = state.header_packets.is_empty();
      let post_1_1_1 = metadata.flac_post_1_1_1.unwrap_or(false);
      if state.header_packets.len() < MAX_HEADER_PACKETS {
        let last = codecs::flac::is_last_metadata_block(packet, is_first, post_1_1_1).unwrap_or(true);
        state.header_packets.push(packet.to_vec());
        if last {
          state.headers_complete = true;
        }
      } else {
        state.headers_complete = true;
      }
    }
    codec_id => {
      if state.header_packets.len() < identify::header_packet_target(codec_id) {
        state.header_packets.push(packet.to_vec());
      }
    }
  }
}

fn try_decode_comment_packet(packet: &[u8], idx: usize, states: &mut [BitstreamState], out: &mut MediaMetadata) {
  let state = &mut states[idx];
  let codec_id = state.metadata.as_ref().map(|m| m.codec_id.as_str()).unwrap_or_default();
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

/// PARSER-249: count OGM-style simple chapters exactly as mkvmerge does.
///
/// `ogm_reader_c::handle_chapters` (`r_ogm.cpp:740-791`) collects every comment
/// whose key starts with `CHAPTER` (case-insensitive), in order, then feeds the
/// `KEY=VALUE` lines to the simple-chapter parser (`mtx::chapters::parse` →
/// `parse_simple`, `chapters.cpp:251`).  That parser alternates strictly
/// between a `CHAPTERxx=HH:MM:SS[.,]frac` timestamp line and a
/// `CHAPTERxxNAME=...` line; any deviation calls `chapter_error`, which throws
/// and aborts the whole parse so that **no** chapters are reported
/// (`r_ogm.cpp:788` swallows the exception, leaving `m_chapters` null).  A
/// trailing unmatched timestamp creates no chapter.  We therefore count only
/// completed (timestamp, name) pairs, and report nothing when the grammar is
/// violated.
fn add_chapters_from_comments(entries: &[crate::media_metadata::model::tag::TagEntry], out: &mut MediaMetadata) {
  let lines: Vec<&crate::media_metadata::model::tag::TagEntry> = entries
    .iter()
    .filter(|e| e.name.to_ascii_uppercase().starts_with("CHAPTER"))
    .collect();
  if lines.is_empty() {
    return;
  }
  let Some(count) = simple_chapter_pair_count(&lines) else {
    // Grammar violation — mkvmerge aborts the parse and reports no chapters.
    return;
  };
  if count > out.chapters.num_entries {
    out.chapters.num_entries = count;
    out.chapters.num_editions = 1;
  }
}

/// Walk the collected `CHAPTER*` comments under the strict simple-chapter
/// grammar.  Returns the number of completed `(timestamp, name)` pairs, or
/// `None` when the alternation is broken (mkvmerge's `chapter_error`).
fn simple_chapter_pair_count(lines: &[&crate::media_metadata::model::tag::TagEntry]) -> Option<u32> {
  let mut expect_name = false;
  let mut count = 0u32;
  for entry in lines {
    if expect_name {
      // mode 1: `^\s*CHAPTER\d+NAME\s*=(.*)`
      if !is_simple_chapter_name_key(&entry.name) {
        return None;
      }
      expect_name = false;
      count += 1;
    } else {
      // mode 0: `^\s*CHAPTER\d+\s*=\s*(\d+):(\d+):(\d+)[.,](\d{1,9})`
      if !is_simple_chapter_timestamp_key(&entry.name) || !simple_chapter_timestamp_value_ok(&entry.value) {
        return None;
      }
      expect_name = true;
    }
  }
  // A trailing unmatched timestamp (expect_name still true) created no chapter.
  Some(count)
}

/// `CHAPTER` followed by one or more digits and nothing else (ignoring trailing
/// whitespace) — the key of a simple-chapter timestamp line.
fn is_simple_chapter_timestamp_key(name: &str) -> bool {
  let upper = name.trim_end().to_ascii_uppercase();
  match upper.strip_prefix("CHAPTER") {
    Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
    None => false,
  }
}

/// `CHAPTER` + digits + `NAME` (ignoring trailing whitespace) — the key of a
/// simple-chapter name line.
fn is_simple_chapter_name_key(name: &str) -> bool {
  let upper = name.trim_end().to_ascii_uppercase();
  let Some(rest) = upper.strip_prefix("CHAPTER") else {
    return false;
  };
  let Some(digits) = rest.strip_suffix("NAME") else {
    return false;
  };
  !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Validate a timestamp value against `\s*(\d+)\s*:\s*(\d+)\s*:\s*(\d+)\s*[.,]\s*(\d{1,9})`.
/// The fractional part is mandatory; minute and second must be < 60; trailing
/// content after the fraction is allowed (the upstream regex is not anchored).
fn simple_chapter_timestamp_value_ok(value: &str) -> bool {
  let bytes = value.as_bytes();
  let mut i = 0usize;
  let skip_ws = |i: &mut usize| {
    while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
      *i += 1;
    }
  };
  // Read a run of digits; returns the parsed value (saturating on overflow) and
  // the number of digits consumed.
  let read_digits = |i: &mut usize| -> Option<u64> {
    let start = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
      *i += 1;
    }
    if *i == start {
      return None;
    }
    Some(value[start..*i].parse::<u64>().unwrap_or(u64::MAX))
  };
  let expect = |i: &mut usize, set: &[u8]| -> bool {
    if *i < bytes.len() && set.contains(&bytes[*i]) {
      *i += 1;
      true
    } else {
      false
    }
  };

  skip_ws(&mut i);
  if read_digits(&mut i).is_none() {
    return false; // hour
  }
  skip_ws(&mut i);
  if !expect(&mut i, b":") {
    return false;
  }
  skip_ws(&mut i);
  let Some(minute) = read_digits(&mut i) else {
    return false;
  };
  skip_ws(&mut i);
  if !expect(&mut i, b":") {
    return false;
  }
  skip_ws(&mut i);
  let Some(second) = read_digits(&mut i) else {
    return false;
  };
  skip_ws(&mut i);
  if !expect(&mut i, b".,") {
    return false;
  }
  skip_ws(&mut i);
  // At least one fraction digit is required.
  if i >= bytes.len() || !bytes[i].is_ascii_digit() {
    return false;
  }
  minute < 60 && second < 60
}

/// PARSER-180: decode a VorbisComment block out of a comment packet using the
/// same prefix auto-detection as mkvtoolnix's
/// `parse_vorbis_comments_from_packet` (common/tags/vorbis.cpp:221-279).  This
/// runs for every non-FLAC stream (r_ogm.cpp:827) regardless of codec id —
/// `handle_stream_comments` always parses `packet_data[1]` through the generic
/// VorbisComment routine.
///
/// Recognised prefixes (auto-detected from the first 8 bytes):
///   * `OpusTags`              → comment block at offset 8
///   * first byte + `vorbis`   → offset 7 (Vorbis ident-style AND OGM streams
///                               whose comment packet is `0x03vorbis`)
///   * `OVP80`                 → offset 7 (VP8-in-Ogg)
///   * `0x81theora`            → offset 7 (intentional Theora superset that the
///                               native parser already supported — kept so we
///                               don't regress Theora comment parsing)
///
/// FLAC-in-Ogg (`A_FLAC`) is excluded, mirroring r_ogm.cpp:827.
fn decode_comment_packet(packet: &[u8], codec_id: &str) -> Option<comments::VorbisComments> {
  if codec_id == "A_FLAC" {
    return None;
  }
  let offset = comment_block_offset(packet)?;
  comments::parse(&packet[offset..])
}

/// Auto-detect the VorbisComment block offset from the comment packet's prefix.
/// Mirrors common/tags/vorbis.cpp:227-247.
fn comment_block_offset(packet: &[u8]) -> Option<usize> {
  if packet.len() > 8 && &packet[..8] == b"OpusTags" {
    Some(8)
  } else if packet.len() > 7 && &packet[1..7] == b"vorbis" {
    // `^.vorbis` — any first byte followed by "vorbis" (covers Vorbis
    // ident-style 0x03 and OGM comment packets alike).
    Some(7)
  } else if packet.len() > 7 && &packet[..5] == b"OVP80" {
    // VP8-in-Ogg.
    Some(7)
  } else if packet.len() > 7 && packet[0] == 0x81 && &packet[1..7] == b"theora" {
    // Theora comment header (`0x81theora`).  Intentional superset of
    // mkvtoolnix — keep so Theora comment parsing does not regress.
    Some(7)
  } else {
    None
  }
}

/// PARSER-181: every in-use stream has read its required header packets.
/// Mirrors the `headers_read` loop in r_ogm.cpp:619-625 — the reader stops once
/// every active demuxer has parsed the number of header packets its codec
/// requires (see `identify::header_packet_target`), not once comments decode.
fn all_streams_have_headers(states: &[BitstreamState]) -> bool {
  !states.is_empty()
    && states.iter().filter(|s| s.metadata.is_some()).all(|s| {
      let metadata = s.metadata.as_ref().expect("filtered to Some");
      identify::headers_satisfied(&s.header_packets, s.headers_complete, metadata)
    })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::model::container::ContainerFormat;
  use crate::media_metadata::model::track::TrackType;
  use crate::media_metadata::ogg::codecs::{flac, kate, ogm, opus, speex, theora, vorbis, vp8};
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

    // PARSER-181: Vorbis needs three header packets (ident + comments + setup)
    // before `headers_read` is satisfied (header_packet_target == 3).  Append a
    // setup packet (0x05 + "vorbis") on the comments page so the stream
    // survives finalise's `erase_if(!headers_read)` (r_ogm.cpp:633).
    let mut setup_pkt = vec![0x05];
    setup_pkt.extend_from_slice(b"vorbis");
    setup_pkt.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
    let page_comments = build_page(0, 0, serial, 1, &[&comments_pkt, &setup_pkt]);

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
    // PARSER-181: the fixture now supplies the full 3-packet Vorbis header set
    // (ident + comments + setup), so the track carries xiph-laced codec private
    // data — a complete Vorbis stream always does.
    assert!(t.codec.codec_private.is_some());
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
        ("CHAPTER02NAME", "Part Two"),
      ],
    ));
    comments_pkt.push(0x01);
    // PARSER-181: Vorbis needs a third (setup) header packet to survive.
    let mut setup_pkt = vec![0x05];
    setup_pkt.extend_from_slice(b"vorbis");
    setup_pkt.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comments_pkt, &setup_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.chapters.num_entries, 2);
    assert_eq!(out.chapters.num_editions, 1);
  }

  // ---- PARSER-249: strict simple-chapter grammar ------------------------

  fn entry(name: &str, value: &str) -> crate::media_metadata::model::tag::TagEntry {
    crate::media_metadata::model::tag::TagEntry {
      name: name.to_string(),
      value: value.to_string(),
      language: None,
    }
  }

  #[test]
  fn simple_chapter_counts_only_completed_pairs() {
    let lines = [
      entry("CHAPTER01", "00:00:00.000"),
      entry("CHAPTER01NAME", "Intro"),
      entry("CHAPTER02", "00:01:02.345"),
      entry("CHAPTER02NAME", "Part Two"),
    ];
    let refs: Vec<_> = lines.iter().collect();
    assert_eq!(simple_chapter_pair_count(&refs), Some(2));
  }

  #[test]
  fn simple_chapter_trailing_timestamp_creates_no_chapter() {
    // A timestamp with no following NAME line is not counted (mkvmerge ends in
    // mode 1 without creating the atom).
    let lines = [
      entry("CHAPTER01", "00:00:00.000"),
      entry("CHAPTER01NAME", "Intro"),
      entry("CHAPTER02", "00:01:02.345"),
    ];
    let refs: Vec<_> = lines.iter().collect();
    assert_eq!(simple_chapter_pair_count(&refs), Some(1));
  }

  #[test]
  fn simple_chapter_broken_alternation_reports_none() {
    // A non-NAME line where a NAME is required is a chapter_error → mkvmerge
    // aborts the parse and reports zero chapters.
    let lines = [
      entry("CHAPTER01", "00:00:00.000"),
      entry("CHAPTERBAD", "ignored"),
    ];
    let refs: Vec<_> = lines.iter().collect();
    assert_eq!(simple_chapter_pair_count(&refs), None);
  }

  #[test]
  fn simple_chapter_missing_fraction_reports_none() {
    // The timestamp regex requires a `[.,]frac` part; a fractionless timestamp
    // fails in mode 0 → chapter_error.
    let lines = [
      entry("CHAPTER01", "00:00:00"),
      entry("CHAPTER01NAME", "Intro"),
    ];
    let refs: Vec<_> = lines.iter().collect();
    assert_eq!(simple_chapter_pair_count(&refs), None);
  }

  #[test]
  fn simple_chapter_rejects_invalid_minute_second() {
    assert!(!simple_chapter_timestamp_value_ok("00:60:00.000"));
    assert!(!simple_chapter_timestamp_value_ok("00:00:60.000"));
    assert!(simple_chapter_timestamp_value_ok("99:59:59.999999999"));
    // Comma fraction separator + surrounding whitespace tolerated.
    assert!(simple_chapter_timestamp_value_ok("01 : 02 : 03 , 5"));
    // Trailing content after the fraction is allowed.
    assert!(simple_chapter_timestamp_value_ok("00:00:01.000 leftover"));
  }

  #[test]
  fn simple_chapter_name_key_recognition() {
    assert!(is_simple_chapter_name_key("CHAPTER01NAME"));
    assert!(is_simple_chapter_name_key("chapter12name"));
    assert!(!is_simple_chapter_name_key("CHAPTER01"));
    assert!(!is_simple_chapter_name_key("CHAPTERNAME"));
    assert!(is_simple_chapter_timestamp_key("CHAPTER01"));
    assert!(!is_simple_chapter_timestamp_key("CHAPTER01NAME"));
    assert!(!is_simple_chapter_timestamp_key("CHAPTERS"));
  }

  #[test]
  fn read_headers_handles_two_independent_streams() {
    let v = build_vorbis_stream(1, None);
    let theora_full = theora::build_identification_packet(640, 480, 24, 1);
    let theora_page = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 2, 0, &[&theora_full]);
    // PARSER-181: Theora's header_packet_target is 3 (ident + comment + setup).
    // A BOS-only stream would be erased by finalise (r_ogm.cpp:633), so supply
    // the comment (0x81"theora") and setup (0x82"theora") header packets too.
    let mut theora_comment = vec![0x81];
    theora_comment.extend_from_slice(b"theora");
    theora_comment.extend(build_block("libtheora 1.1", &[("LANGUAGE", "deu")]));
    let mut theora_setup = vec![0x82];
    theora_setup.extend_from_slice(b"theora");
    theora_setup.extend_from_slice(&[0x11, 0x22, 0x33]);
    let theora_headers_page = build_page(0, 0, 2, 1, &[&theora_comment, &theora_setup]);
    let mut bytes = v;
    bytes.extend(theora_page);
    bytes.extend(theora_headers_page);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogv", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.iter().any(|t| t.codec.id == "A_VORBIS"));
    let theora = out.tracks.iter().find(|t| t.codec.id == "V_THEORA").unwrap();
    // PARSER-180: the generic comment-packet path now decodes Theora's
    // LANGUAGE comment from the second header packet.
    assert_eq!(theora.properties.common.language.as_ref().unwrap().iso639_2, "deu");
  }

  #[test]
  fn read_headers_decodes_ogm_comment_packet_generically() {
    // PARSER-180: an OGM stream's comment packet (`0x03vorbis` + block) is now
    // parsed through the generic VorbisComment path (r_ogm.cpp:826-836 always
    // parses packet_data[1] for non-FLAC demuxers), yielding language + tags.
    // OGM video's TITLE is promoted to the container title (ms_compat), so we
    // assert language/tags rather than track name.
    let bos = ogm::build_audio_header(48000, 2, 16); // format tag 0x00ff → AAC
    let mut comment_pkt = vec![0x03];
    comment_pkt.extend_from_slice(b"vorbis");
    comment_pkt.extend(build_block(
      "libVorbis OGM",
      &[("TITLE", "OGM Audio"), ("LANGUAGE", "jpn"), ("ARTIST", "X")],
    ));
    comment_pkt.push(0x01);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    // OGM audio header_packet_target is 1, so the BOS alone keeps it; the
    // second packet is the comment packet we want decoded.
    bytes.extend(build_page(0, 0, 1, 1, &[&comment_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogm", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_AAC");
    assert_eq!(t.properties.common.language.as_ref().unwrap().iso639_2, "jpn");
    // TITLE + LANGUAGE + ARTIST + VENDOR
    assert_eq!(t.properties.tags.len(), 4);
  }

  #[test]
  fn read_headers_decodes_speex_comment_packet() {
    // PARSER-180: a Speex stream's comment packet (`0x03vorbis` + block, the
    // Speex VorbisComment convention) is decoded generically.  Speex's
    // header_packet_target is 2 (ident + comments).
    let bos = speex::build_identification_packet(16000, 1);
    let mut comment_pkt = vec![0x03];
    comment_pkt.extend_from_slice(b"vorbis");
    comment_pkt.extend(build_block("libspeex 1.2", &[("TITLE", "Voice"), ("LANGUAGE", "spa")]));
    comment_pkt.push(0x01);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comment_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.spx", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_SPEEX");
    assert_eq!(t.properties.common.language.as_ref().unwrap().iso639_2, "spa");
    assert_eq!(t.properties.common.track_name.as_deref(), Some("Voice"));
  }

  #[test]
  fn read_headers_does_not_parse_flac_comment_packet() {
    // PARSER-180: FLAC-in-Ogg is excluded from generic comment parsing
    // (r_ogm.cpp:827 skips A_FLAC).  Even though its second packet looks like a
    // VorbisComment block, no tags/language must be harvested.  FLAC's
    // header_packet_target is 1, so the BOS alone keeps the track.
    let bos = flac::build_identification_packet(48000, 2, 24, 1_000_000);
    let mut comment_pkt = vec![0x04]; // native FLAC VORBIS_COMMENT block header
    comment_pkt.extend(build_block("reference libFLAC", &[("TITLE", "Song"), ("LANGUAGE", "eng")]));
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comment_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.oga", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_FLAC");
    assert!(t.properties.common.language.is_none());
    assert!(t.properties.common.track_name.is_none());
    assert!(t.properties.tags.is_empty());
  }

  #[test]
  fn read_headers_drops_stream_whose_headers_never_complete() {
    // PARSER-181: a Vorbis BOS with no comment/setup packets never reaches its
    // header_packet_target of 3, so finalise erases it (r_ogm.cpp:633's
    // `erase_if(!headers_read)`).  The result is zero tracks even though the
    // BOS was identified.
    let bos = vorbis::build_identification_packet(2, 44100);
    let bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
    // Container is still recognised — only the incomplete stream is dropped.
    assert_eq!(out.container.format, ContainerFormat::Ogg);
  }

  #[test]
  fn comment_block_offset_detects_recognised_prefixes() {
    // PARSER-180: prefix auto-detection (common/tags/vorbis.cpp:227-247).
    let mut opus = b"OpusTags".to_vec();
    opus.push(0);
    assert_eq!(comment_block_offset(&opus), Some(8));

    let mut vorbis = vec![0x03];
    vorbis.extend_from_slice(b"vorbis_");
    assert_eq!(comment_block_offset(&vorbis), Some(7));

    let mut vp8 = b"OVP80".to_vec();
    vp8.extend_from_slice(&[0u8; 4]);
    assert_eq!(comment_block_offset(&vp8), Some(7));

    let mut theora = vec![0x81];
    theora.extend_from_slice(b"theora_");
    assert_eq!(comment_block_offset(&theora), Some(7));

    // Unrecognised prefix and too-short buffers yield None.
    assert_eq!(comment_block_offset(b"junkjunk0"), None);
    assert_eq!(comment_block_offset(b"\x03vorbi"), None);
  }

  #[test]
  fn decode_comment_packet_excludes_flac_even_with_vorbis_prefix() {
    // PARSER-180: A_FLAC is skipped (r_ogm.cpp:827) regardless of prefix.
    let mut pkt = vec![0x03];
    pkt.extend_from_slice(b"vorbis");
    pkt.extend(build_block("v", &[("TITLE", "X")]));
    assert!(decode_comment_packet(&pkt, "A_FLAC").is_none());
    // The same bytes decode for any non-FLAC codec id.
    assert!(decode_comment_packet(&pkt, "A_SPEEX").is_some());
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
  fn read_headers_recognizes_ogg_vp8_with_dimensions_and_duration() {
    // PARSER-202: VP8-in-Ogg is recognised, reports V_VP8, pixel + display
    // dimensions, and a default duration derived from the frame rate.  The
    // optional comment packet (`0x03vorbis`) sits on the second page; VP8's
    // header target is 1 so the BOS alone keeps the track.
    let bos = vp8::build_identification_packet(1280, 720, 0, 0, 30000, 1001);
    let mut comment_pkt = vec![0x03];
    comment_pkt.extend_from_slice(b"vorbis");
    comment_pkt.extend(build_block("libVP8 OGM", &[("TITLE", "Clip"), ("LANGUAGE", "eng")]));
    comment_pkt.push(0x01);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comment_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.track_type, TrackType::Video);
    assert_eq!(t.codec.id, "V_VP8");
    let v = t.properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(crate::media_metadata::model::track_properties_video::Dimensions2D { width: 1280, height: 720 })
    );
    assert_eq!(
      v.display_dimensions,
      Some(crate::media_metadata::model::track_properties_video::Dimensions2D { width: 1280, height: 720 })
    );
    // 1001/30000 s ≈ 33_366_666 ns.
    assert_eq!(v.default_duration_ns, Some(33_366_666));
    // PARSER-202: the `0x03vorbis` comment packet decodes generically.
    assert_eq!(t.properties.common.language.as_ref().unwrap().iso639_2, "eng");
  }

  #[test]
  fn read_headers_accepts_pre_1_1_1_bare_flac() {
    // PARSER-203: a stream whose first packet starts directly with `fLaC`
    // (pre-1.1.1 mapping) is recognised as A_FLAC.
    let bos = flac::build_identification_packet_ex(44100, 2, 16, 1000, false, 0, true);
    let bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.oga", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_FLAC");
    assert_eq!(t.properties.audio.as_ref().unwrap().sampling_frequency, Some(44100.0));
  }

  #[test]
  fn read_headers_assembles_multi_packet_flac_codec_private() {
    // PARSER-204: a post-1.1.1 FLAC stream that advertises 2 "other" header
    // packets is read until the last metadata block; the codec private strips
    // the 9-byte wrapper from the first packet and concatenates all blocks.
    let bos = flac::build_identification_packet_ex(48000, 2, 24, 0, true, 2, false);
    // Two more metadata blocks; the last carries the last-block flag.
    let vc_block = flac::build_metadata_block_packet(4, false, b"VORBIS-COMMENT-BODY");
    let pad_block = flac::build_metadata_block_packet(1, true, &[0u8; 8]);
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&bos]);
    bytes.extend(build_page(0, 0, 1, 1, &[&vc_block, &pad_block]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.oga", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "A_FLAC");
    let private = t.codec.codec_private.as_ref().unwrap();
    // Expected = (bos[9..]) ++ vc_block ++ pad_block.
    let mut expected = bos[9..].to_vec();
    expected.extend_from_slice(&vc_block);
    expected.extend_from_slice(&pad_block);
    assert_eq!(private.length, expected.len() as u64);
    assert!(private.hex.starts_with("664c6143")); // "fLaC"
  }

  #[test]
  fn read_headers_xiph_laces_multi_packet_kate_codec_private() {
    // PARSER-205: Kate keeps reading header packets while the high bit is set
    // and Xiph-laces all of them into codec private.  A high-bit-clear packet
    // ends the header run.
    let ident = kate::build_identification_packet("en");
    let mut comment = vec![0x81]; // second Kate header (high bit set)
    comment.extend_from_slice(b"kate\0\0\0");
    comment.extend_from_slice(&[0xAA; 20]);
    let mut regions = vec![0x82]; // third Kate header (high bit set)
    regions.extend_from_slice(b"kate\0\0\0");
    regions.extend_from_slice(&[0xBB; 12]);
    let data_pkt = vec![0x00, 0x01, 0x02]; // high bit clear → ends header run
    let mut bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 1, 0, &[&ident]);
    bytes.extend(build_page(0, 0, 1, 1, &[&comment, &regions, &data_pkt]));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ogg", 0);
    OggReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    let t = &out.tracks[0];
    assert_eq!(t.codec.id, "S_KATE");
    let private = t.codec.codec_private.as_ref().unwrap();
    // Xiph lacing of three header packets: count-1 (=2), lace sizes for the
    // first two, then all three payloads.  The data packet is excluded.
    let total_payload = ident.len() + comment.len() + regions.len();
    assert!(private.length as usize > total_payload); // includes lace header
    assert!(private.hex.starts_with("02")); // 3 packets → count-1 = 2
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
