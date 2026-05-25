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

//! Blu-ray playlist (`.mpls`) parser.  Port of
//! `mkvtoolnix/src/common/bluray/mpls.cpp::parser_c`.
//!
//! The whole (small, ≤ 10 MiB) file is read into memory and walked with a bit
//! reader, exactly like mkvtoolnix.  We extract total duration, the ordered
//! list of segment clip ids, the chapter count (after the trailing-mark drop
//! rule), the per-play-item STN stream table (PARSER-157), and the sub-paths
//! (PARSER-156) — the latter so text-subtitle-presentation sub-path clips can
//! be added as external `.m2ts` inputs, matching mkvmerge.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;

/// Default Blu-ray mark-drop window: a final chapter within 5 s of the end is
/// dropped (mkvtoolnix's `m_drop_last_entry_if_at_end`).
const DROP_LAST_WINDOW_NS: i64 = 5_000_000_000;

/// `sub_path_type_e::text_subtitle_presentation` (mpls.h:69) — the sub-path
/// kind whose clips carry presentation-graphics / text subtitle streams that
/// mkvmerge adds as external inputs (`r_mpeg_ts.cpp:2956-3006`).
pub const SUB_PATH_TYPE_TEXT_SUBTITLE_PRESENTATION: u8 = 4;

/// One STN stream descriptor (mpls.h:94-101 / mpls.cpp:391-447).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StnStream {
  pub stream_type: u8,
  pub coding_type: u8,
  pub sub_path_id: u8,
  pub sub_clip_id: u8,
  pub pid: u16,
  pub format: u8,
  pub rate: u8,
  pub char_code: u8,
  /// ISO 639-2 alpha-3 language, when the coding type carries one.
  pub language: Option<String>,
}

/// The STN table of one play item (mpls.h:103-108).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stn {
  pub video: Vec<StnStream>,
  pub audio: Vec<StnStream>,
  pub pg: Vec<StnStream>,
}

/// One play item — a reference to a `.m2ts` clip plus its in/out times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayItem {
  pub clip_id: String,
  pub codec_id: String,
  pub in_ns: i64,
  pub out_ns: i64,
  /// Cumulative duration of all earlier play items (mkvtoolnix's
  /// `relative_in_time`), used to map chapter timestamps onto the timeline.
  pub relative_in_ns: i64,
  /// PARSER-157: the decoded STN stream table.
  pub stn: Stn,
}

/// One clip of a multi-clip sub-play-item (mpls.h:110-114).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubPlayItemClip {
  pub clip_id: String,
  pub codec_id: String,
}

/// One sub-play-item (mpls.h:117-125).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubPlayItem {
  pub clip_id: String,
  pub codec_id: String,
  pub in_ns: i64,
  pub out_ns: i64,
  pub clips: Vec<SubPlayItemClip>,
}

/// One sub-path (mpls.h:127-132).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubPath {
  pub sub_path_type: u8,
  pub is_repeat: bool,
  pub items: Vec<SubPlayItem>,
}

/// The parsed playlist — only the fields mkvmerge reports at identification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Playlist {
  pub duration_ns: i64,
  pub items: Vec<PlayItem>,
  pub chapter_count: u32,
  /// PARSER-156: sub-paths (sub-play-items + their clips).
  pub sub_paths: Vec<SubPath>,
}

/// 45 kHz Blu-ray ticks → nanoseconds (`value * 1_000_000 / 45`).
fn ticks_to_ns(value: u64) -> i64 {
  (value.saturating_mul(1_000_000) / 45) as i64
}

/// Parse an in-memory `.mpls` buffer.  Returns `Err` for anything that is not
/// a structurally valid MPLS playlist so the caller can fall through to the
/// normal probe cascade.
pub fn parse(buf: &[u8]) -> Result<Playlist, ParseError> {
  let mut r = BitReader::new(buf);
  let header = parse_header(&mut r)?;

  let mut playlist = Playlist::default();
  parse_playlist(&mut r, &header, &mut playlist)?;
  playlist.chapter_count = parse_chapters(&mut r, &header, &playlist)?;
  Ok(playlist)
}

struct Header {
  playlist_pos: u64,
  chapter_pos: u64,
}

fn malformed(reason: &'static str) -> ParseError {
  ParseError::Malformed {
    format: "mpls",
    offset: 0,
    reason: reason.to_string(),
  }
}

fn parse_header(r: &mut BitReader) -> Result<Header, ParseError> {
  let type_indicator1 = r.read_bytes_aligned(4)?.to_vec();
  let type_indicator2 = r.read_bytes_aligned(4)?.to_vec();
  let playlist_pos = r.read_bits(32)?;
  let chapter_pos = r.read_bits(32)?;
  let _ext_pos = r.read_bits(32)?;

  if type_indicator1 != b"MPLS" {
    return Err(malformed("missing MPLS type indicator"));
  }
  if !matches!(type_indicator2.as_slice(), b"0100" | b"0200" | b"0300") {
    return Err(malformed("unsupported MPLS version"));
  }
  Ok(Header {
    playlist_pos,
    chapter_pos,
  })
}

fn parse_playlist(r: &mut BitReader, header: &Header, playlist: &mut Playlist) -> Result<(), ParseError> {
  r.set_bit_position(header.playlist_pos * 8);
  r.skip_bits(32 + 16)?; // playlist length, reserved
  let list_count = r.read_bits(16)? as usize;
  let sub_count = r.read_bits(16)? as usize;

  for _ in 0..list_count {
    let item = parse_play_item(r, playlist.duration_ns)?;
    playlist.duration_ns += item.out_ns - item.in_ns;
    playlist.items.push(item);
  }
  // PARSER-156: sub-paths follow the play items.  They do not contribute to
  // the playlist duration (the chapter parser only references play items), but
  // text-subtitle-presentation sub-paths reference clips mkvmerge adds as
  // external subtitle inputs.
  for _ in 0..sub_count {
    playlist.sub_paths.push(parse_sub_path(r)?);
  }
  Ok(())
}

fn parse_play_item(r: &mut BitReader, relative_in_ns: i64) -> Result<PlayItem, ParseError> {
  let length = r.read_bits(16)?;
  let position = r.position_bytes();

  let clip_id = read_string(r, 5)?;
  let codec_id = read_string(r, 4)?;
  r.skip_bits(11)?; // reserved
  let is_multi_angle = r.read_bit()?;
  let _connection_condition = r.read_bits(4)?;
  let _stc_id = r.read_bits(8)?;
  let in_ns = ticks_to_ns(r.read_bits(32)?);
  let out_ns = ticks_to_ns(r.read_bits(32)?);

  // PARSER-157: walk through the UO mask + multi-angle clips into the STN
  // table rather than seeking straight past it (mpls.cpp:290-304).
  r.skip_bits(12 * 8)?; // UO_mask_table, random_access_flag, reserved, still_mode

  if is_multi_angle {
    let num_angles = r.read_bits(8)?;
    r.skip_bits(8)?; // reserved, is_different_audio, is_seamless_angle_change
    if num_angles > 0 {
      r.skip_bits((num_angles - 1) * 10 * 8)?; // clip_id, clip_codec_id, stc_id
    }
  }

  r.skip_bits(16 + 16)?; // STN length, reserved
  let stn = parse_stn(r)?;

  // Re-align to the declared end of this play item.
  r.set_bit_position((position + length) * 8);

  Ok(PlayItem {
    clip_id,
    codec_id,
    in_ns,
    out_ns,
    relative_in_ns,
    stn,
  })
}

/// Parse the STN table (mpls.cpp:361-389).
fn parse_stn(r: &mut BitReader) -> Result<Stn, ParseError> {
  let num_video = r.read_bits(8)?;
  let num_audio = r.read_bits(8)?;
  let num_pg = r.read_bits(8)?;
  let _num_ig = r.read_bits(8)?;
  let _num_secondary_audio = r.read_bits(8)?;
  let _num_secondary_video = r.read_bits(8)?;
  let _num_pip_pg = r.read_bits(8)?;
  r.skip_bits(5 * 8)?; // reserved

  let mut stn = Stn::default();
  for _ in 0..num_video {
    stn.video.push(parse_stream(r)?);
  }
  for _ in 0..num_audio {
    stn.audio.push(parse_stream(r)?);
  }
  for _ in 0..num_pg {
    stn.pg.push(parse_stream(r)?);
  }
  Ok(stn)
}

// stream_coding_type groupings (mpls.h:34-51 / mpls.cpp:422-442).
const CODING_VIDEO: [u8; 3] = [0x02, 0x1b, 0xea]; // MPEG-2, AVC, VC-1
const CODING_AUDIO: [u8; 9] = [0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0xa1, 0xa2];
const CODING_PG_IG: [u8; 2] = [0x90, 0x91]; // presentation graphics / interactive menu
const CODING_TEXT_SUBTITLES: u8 = 0x92;

/// Parse one STN stream entry: a stream-entry block followed by a
/// stream-attributes block, each length-prefixed (mpls.cpp:391-447).
fn parse_stream(r: &mut BitReader) -> Result<StnStream, ParseError> {
  let mut s = StnStream::default();

  // --- stream_entry ---
  let length = r.read_bits(8)?;
  let position = r.position_bytes();
  s.stream_type = r.read_bits(8)? as u8;
  match s.stream_type {
    1 => {
      s.pid = r.read_bits(16)? as u16;
    }
    2 => {
      s.sub_path_id = r.read_bits(8)? as u8;
      s.sub_clip_id = r.read_bits(8)? as u8;
      s.pid = r.read_bits(16)? as u16;
    }
    3 => {
      s.sub_path_id = r.read_bits(8)? as u8;
      s.pid = r.read_bits(16)? as u16;
    }
    _ => {}
  }
  r.set_bit_position((length + position) * 8);

  // --- stream_attributes ---
  let length = r.read_bits(8)?;
  let position = r.position_bytes();
  s.coding_type = r.read_bits(8)? as u8;
  if CODING_VIDEO.contains(&s.coding_type) {
    s.format = r.read_bits(4)? as u8;
    s.rate = r.read_bits(4)? as u8;
  } else if CODING_AUDIO.contains(&s.coding_type) {
    s.format = r.read_bits(4)? as u8;
    s.rate = r.read_bits(4)? as u8;
    s.language = Some(read_string(r, 3)?);
  } else if CODING_PG_IG.contains(&s.coding_type) {
    s.language = Some(read_string(r, 3)?);
  } else if s.coding_type == CODING_TEXT_SUBTITLES {
    s.char_code = r.read_bits(8)? as u8;
    s.language = Some(read_string(r, 3)?);
  }
  r.set_bit_position((position + length) * 8);

  Ok(s)
}

/// Parse one sub-path (mpls.cpp:309-321).
fn parse_sub_path(r: &mut BitReader) -> Result<SubPath, ParseError> {
  r.skip_bits(32 + 8)?; // length, reserved
  let sub_path_type = r.read_bits(8)? as u8;
  r.skip_bits(15)?; // reserved
  let is_repeat = r.read_bit()?;
  r.skip_bits(8)?; // reserved
  let num_sub_play_items = r.read_bits(8)?;

  let mut items = Vec::new();
  for _ in 0..num_sub_play_items {
    items.push(parse_sub_play_item(r)?);
  }
  Ok(SubPath {
    sub_path_type,
    is_repeat,
    items,
  })
}

/// Parse one sub-play-item (mpls.cpp:323-348).
fn parse_sub_play_item(r: &mut BitReader) -> Result<SubPlayItem, ParseError> {
  r.skip_bits(16)?; // length
  let clip_id = read_string(r, 5)?;
  let codec_id = read_string(r, 4)?;
  r.skip_bits(27)?; // reserved
  let _connection_condition = r.read_bits(4)?;
  let is_multi_clip_entries = r.read_bit()?;
  let _ref_to_stc_id = r.read_bits(8)?;
  let in_ns = ticks_to_ns(r.read_bits(32)?);
  let out_ns = ticks_to_ns(r.read_bits(32)?);
  let _sync_playitem_id = r.read_bits(16)?;
  let _sync_start_pts = r.read_bits(32)?;

  let mut item = SubPlayItem {
    clip_id,
    codec_id,
    in_ns,
    out_ns,
    clips: Vec::new(),
  };
  if !is_multi_clip_entries {
    return Ok(item);
  }
  let num_clips = r.read_bits(8)?;
  r.skip_bits(8)?; // reserved
  // mkvtoolnix iterates `1..num_clips` — the first clip is the sub-play-item
  // itself, already captured above.
  for _ in 1..num_clips {
    item.clips.push(parse_sub_play_item_clip(r)?);
  }
  Ok(item)
}

/// Parse one sub-play-item clip (mpls.cpp:350-359).
fn parse_sub_play_item_clip(r: &mut BitReader) -> Result<SubPlayItemClip, ParseError> {
  let clip_id = read_string(r, 5)?;
  let codec_id = read_string(r, 4)?;
  let _ref_to_stc_id = r.read_bits(8)?;
  Ok(SubPlayItemClip { clip_id, codec_id })
}

/// Parse the PlayListMark table into a chapter count, applying mkvtoolnix's
/// trailing-mark drop rule and only counting entry marks bound to a valid
/// play item.
fn parse_chapters(r: &mut BitReader, header: &Header, playlist: &Playlist) -> Result<u32, ParseError> {
  r.set_bit_position(header.chapter_pos * 8);
  r.skip_bits(32)?; // unknown
  let num_chapters = r.read_bits(16)? as u64;

  let mut timestamps: Vec<i64> = Vec::new();
  for idx in 0..num_chapters {
    r.set_bit_position((header.chapter_pos + 4 + 2 + idx * 14) * 8);
    r.skip_bits(8)?; // unknown
    if r.read_bits(8)? != 1 {
      // chapter type must be "entry mark"
      continue;
    }
    let play_item_idx = r.read_bits(16)? as usize;
    let Some(item) = playlist.items.get(play_item_idx) else {
      continue;
    };
    let chapter_time = ticks_to_ns(r.read_bits(32)?);
    timestamps.push(chapter_time - item.in_ns + item.relative_in_ns);
  }

  // mkvtoolnix drops a final mark sitting within 5 s of the playlist end.
  if let Some(&last) = timestamps.last() {
    if (playlist.duration_ns - last) <= DROP_LAST_WINDOW_NS {
      timestamps.pop();
    }
  }
  Ok(timestamps.len() as u32)
}

fn read_string(r: &mut BitReader, len: usize) -> Result<String, ParseError> {
  let bytes = r.read_bytes_aligned(len)?;
  Ok(String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string())
}

// ---- Synthetic MPLS fixture builders (shared with the `mod.rs` tests) -----

/// Build one STN audio stream descriptor (stream_entry + stream_attributes).
#[cfg(test)]
pub(crate) fn build_audio_stn_stream(pid: u16, lang: &[u8; 3]) -> Vec<u8> {
  // stream_entry: length + [stream_type=1, pid].
  let mut entry = vec![1u8];
  entry.extend(pid.to_be_bytes());
  let mut out = vec![entry.len() as u8];
  out.extend(entry);
  // stream_attributes: length + [coding_type=0x81 (AC-3), format/rate, lang].
  let mut attr = vec![0x81u8, 0x00];
  attr.extend_from_slice(lang);
  out.push(attr.len() as u8);
  out.extend(attr);
  out
}

/// Build one STN video stream descriptor (stream_entry + stream_attributes).
/// `coding_type` defaults to AVC (0x1b) when set so the format/rate nibbles are
/// parsed (mpls.cpp:422-425).
#[cfg(test)]
pub(crate) fn build_video_stn_stream(pid: u16, coding_type: u8, format: u8, rate: u8) -> Vec<u8> {
  // stream_entry: length + [stream_type=1, pid].
  let mut entry = vec![1u8];
  entry.extend(pid.to_be_bytes());
  let mut out = vec![entry.len() as u8];
  out.extend(entry);
  // stream_attributes: length + [coding_type, format/rate nibbles].
  let attr = vec![coding_type, (format << 4) | (rate & 0x0f)];
  out.push(attr.len() as u8);
  out.extend(attr);
  out
}

/// Build one STN presentation-graphics stream descriptor (stream_entry +
/// stream_attributes); PG coding type carries only an ISO-639 language
/// (mpls.cpp:434-435).
#[cfg(test)]
pub(crate) fn build_pg_stn_stream(pid: u16, lang: &[u8; 3]) -> Vec<u8> {
  // stream_entry: length + [stream_type=1, pid].
  let mut entry = vec![1u8];
  entry.extend(pid.to_be_bytes());
  let mut out = vec![entry.len() as u8];
  out.extend(entry);
  // stream_attributes: length + [coding_type=0x90 (PGS), lang].
  let mut attr = vec![0x90u8];
  attr.extend_from_slice(lang);
  out.push(attr.len() as u8);
  out.extend(attr);
  out
}

/// Build a framed play item that carries the given STN audio streams.
#[cfg(test)]
pub(crate) fn build_item_with_stn(clip: &str, in_t: u32, out_t: u32, audio: &[Vec<u8>]) -> Vec<u8> {
  build_item_with_stn_groups(clip, in_t, out_t, &[], audio, &[])
}

/// Build a framed play item carrying the given STN video / audio / PG streams.
/// Mirrors `parse_stn`'s count-byte layout (mpls.cpp:361-389).
#[cfg(test)]
pub(crate) fn build_item_with_stn_groups(
  clip: &str,
  in_t: u32,
  out_t: u32,
  video: &[Vec<u8>],
  audio: &[Vec<u8>],
  pg: &[Vec<u8>],
) -> Vec<u8> {
  let mut body = Vec::new();
  body.extend(clip.as_bytes()); // 5
  body.extend(b"M2TS"); // 4
  body.extend([0u8; 3]); // reserved + multi_angle(0) + conn + stc
  body.extend(in_t.to_be_bytes());
  body.extend(out_t.to_be_bytes());
  body.extend([0u8; 12]); // UO mask + flags
  body.extend([0u8; 4]); // STN length + reserved
  body.push(video.len() as u8); // num_video
  body.push(audio.len() as u8); // num_audio
  body.push(pg.len() as u8); // num_pg
  body.extend([0u8; 4]); // num_ig / num_sec_audio / num_sec_video / num_pip_pg
  body.extend([0u8; 5]); // reserved
  for s in video {
    body.extend_from_slice(s);
  }
  for s in audio {
    body.extend_from_slice(s);
  }
  for s in pg {
    body.extend_from_slice(s);
  }
  let mut framed = (body.len() as u16).to_be_bytes().to_vec();
  framed.extend(body);
  framed
}

/// Build a framed sub-path with one (non-multi-clip) sub-play-item.
#[cfg(test)]
pub(crate) fn build_subpath(sub_path_type: u8, clip: &str) -> Vec<u8> {
  let mut spi_body = Vec::new();
  spi_body.extend(clip.as_bytes()); // 5
  spi_body.extend(b"M2TS"); // 4
  spi_body.extend([0u8; 4]); // 27 reserved + conn(4) + multi(1=0)
  spi_body.push(0); // ref_to_stc_id
  spi_body.extend(0u32.to_be_bytes()); // in
  spi_body.extend(0u32.to_be_bytes()); // out
  spi_body.extend(0u16.to_be_bytes()); // sync_playitem_id
  spi_body.extend(0u32.to_be_bytes()); // sync_start_pts
  let mut spi = (spi_body.len() as u16).to_be_bytes().to_vec();
  spi.extend(spi_body);

  let mut sp = Vec::new();
  sp.extend(0u32.to_be_bytes()); // length (unused by parser)
  sp.push(0); // reserved
  sp.push(sub_path_type); // type
  sp.push(0); // 8 reserved bits
  sp.push(0); // 7 reserved bits + is_repeat(0)
  sp.push(0); // reserved
  sp.push(1); // num_sub_play_items
  sp.extend(spi);
  sp
}

/// Assemble an MPLS from pre-framed play items + sub-paths.
#[cfg(test)]
pub(crate) fn build_mpls_with(items: &[Vec<u8>], sub_paths: &[Vec<u8>]) -> Vec<u8> {
  let mut playlist = Vec::new();
  playlist.extend(0u32.to_be_bytes());
  playlist.extend(0u16.to_be_bytes());
  playlist.extend((items.len() as u16).to_be_bytes());
  playlist.extend((sub_paths.len() as u16).to_be_bytes());
  for it in items {
    playlist.extend_from_slice(it);
  }
  for sp in sub_paths {
    playlist.extend_from_slice(sp);
  }
  let mut chapters = Vec::new();
  chapters.extend(0u32.to_be_bytes());
  chapters.extend(0u16.to_be_bytes());

  let playlist_pos = 40u32;
  let chapter_pos = playlist_pos + playlist.len() as u32;
  let mut buf = Vec::new();
  buf.extend(b"MPLS");
  buf.extend(b"0200");
  buf.extend(playlist_pos.to_be_bytes());
  buf.extend(chapter_pos.to_be_bytes());
  buf.extend(0u32.to_be_bytes());
  while (buf.len() as u32) < playlist_pos {
    buf.push(0);
  }
  buf.extend(playlist);
  buf.extend(chapters);
  buf
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Build a minimal but structurally complete MPLS buffer with the given
  /// play items (clip_id, in_ticks, out_ticks) and chapter marks
  /// (play_item_idx, chapter_ticks).
  fn build_mpls(items: &[(&str, u32, u32)], marks: &[(u16, u32)]) -> Vec<u8> {
    // Layout: 40-byte header region, then playlist block, then chapter block.
    let mut playlist = Vec::new();
    // playlist length (4) + reserved (2)
    playlist.extend(0u32.to_be_bytes());
    playlist.extend(0u16.to_be_bytes());
    playlist.extend((items.len() as u16).to_be_bytes()); // list_count
    playlist.extend(0u16.to_be_bytes()); // sub_count
    for (clip_id, in_t, out_t) in items {
      let mut item = Vec::new();
      item.extend(clip_id.as_bytes()); // 5
      item.extend(b"M2TS"); // codec_id, 4
      item.push(0x00); // 8 reserved bits (part of 11)
      item.push(0x00); // 3 reserved + multi_angle(0) + conn(4) = byte
      item.push(0x00); // stc_id
      item.extend(in_t.to_be_bytes());
      item.extend(out_t.to_be_bytes());
      item.extend([0u8; 12]); // UO_mask + flags + still_mode
      item.extend([0u8; 4]); // STN length + reserved
      item.extend([0u8; 12]); // STN: 7 count bytes (all 0) + 5 reserved → no streams
      // body length excludes the 2-byte length field itself.
      let mut framed = ((item.len()) as u16).to_be_bytes().to_vec();
      framed.extend(item);
      playlist.extend(framed);
    }

    // Chapter block: skip(4) + num_chapters(2) + 14 bytes per mark.
    let mut chapters = Vec::new();
    chapters.extend(0u32.to_be_bytes()); // unknown
    chapters.extend((marks.len() as u16).to_be_bytes());
    for (play_item_idx, ticks) in marks {
      let mut mark = Vec::new();
      mark.push(0x00); // unknown
      mark.push(0x01); // entry mark
      mark.extend(play_item_idx.to_be_bytes());
      mark.extend(ticks.to_be_bytes());
      mark.extend([0u8; 6]); // remaining bytes of the 14-byte record
      assert_eq!(mark.len(), 14);
      chapters.extend(mark);
    }

    // Assemble: header (20 bytes) + padding to playlist_pos.
    let header_len = 40u32; // leave room past the 20-byte fixed header
    let playlist_pos = header_len;
    let chapter_pos = playlist_pos + playlist.len() as u32;

    let mut buf = Vec::new();
    buf.extend(b"MPLS");
    buf.extend(b"0200");
    buf.extend(playlist_pos.to_be_bytes());
    buf.extend(chapter_pos.to_be_bytes());
    buf.extend(0u32.to_be_bytes()); // ext_pos
    // pad from byte 20 to playlist_pos
    while (buf.len() as u32) < playlist_pos {
      buf.push(0);
    }
    buf.extend(playlist);
    buf.extend(chapters);
    buf
  }

  #[test]
  fn parses_header_and_duration() {
    // Two items: 0..45000 ticks (1s) and 0..90000 ticks (2s) → 3s total.
    let buf = build_mpls(&[("00001", 0, 45_000), ("00002", 0, 90_000)], &[]);
    let p = parse(&buf).unwrap();
    assert_eq!(p.items.len(), 2);
    assert_eq!(p.items[0].clip_id, "00001");
    assert_eq!(p.items[1].clip_id, "00002");
    assert_eq!(p.duration_ns, 3_000_000_000);
    assert_eq!(p.items[1].relative_in_ns, 1_000_000_000);
  }

  #[test]
  fn rejects_non_mpls_magic() {
    let mut buf = build_mpls(&[("00001", 0, 45_000)], &[]);
    buf[0] = b'X';
    assert!(parse(&buf).is_err());
  }

  #[test]
  fn rejects_unknown_version() {
    let mut buf = build_mpls(&[("00001", 0, 45_000)], &[]);
    buf[4..8].copy_from_slice(b"9999");
    assert!(parse(&buf).is_err());
  }

  #[test]
  fn counts_chapter_marks() {
    // Duration 10s; marks at 0s, 2s, 4s — none within 5s of the end → kept.
    let buf = build_mpls(
      &[("00001", 0, 450_000)],
      &[(0, 0), (0, 90_000), (0, 180_000)],
    );
    let p = parse(&buf).unwrap();
    assert_eq!(p.duration_ns, 10_000_000_000);
    assert_eq!(p.chapter_count, 3);
  }

  #[test]
  fn drops_trailing_chapter_within_five_seconds() {
    // Duration 10s; last mark at 9s is within the 5s window → dropped.
    let buf = build_mpls(&[("00001", 0, 450_000)], &[(0, 0), (0, 405_000)]);
    let p = parse(&buf).unwrap();
    assert_eq!(p.chapter_count, 1);
  }

  #[test]
  fn parses_stn_audio_stream_descriptor() {
    // PARSER-157
    let item = build_item_with_stn("00001", 0, 90_000, &[build_audio_stn_stream(0x1100, b"eng")]);
    let buf = build_mpls_with(&[item], &[]);
    let p = parse(&buf).unwrap();
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].stn.audio.len(), 1);
    let s = &p.items[0].stn.audio[0];
    assert_eq!(s.pid, 0x1100);
    assert_eq!(s.coding_type, 0x81);
    assert_eq!(s.language.as_deref(), Some("eng"));
  }

  #[test]
  fn parses_text_subtitle_sub_path() {
    // PARSER-156
    let item = build_item_with_stn("00001", 0, 90_000, &[]);
    let sp = build_subpath(SUB_PATH_TYPE_TEXT_SUBTITLE_PRESENTATION, "00100");
    let buf = build_mpls_with(&[item], &[sp]);
    let p = parse(&buf).unwrap();
    assert_eq!(p.sub_paths.len(), 1);
    assert_eq!(p.sub_paths[0].sub_path_type, SUB_PATH_TYPE_TEXT_SUBTITLE_PRESENTATION);
    assert_eq!(p.sub_paths[0].items.len(), 1);
    assert_eq!(p.sub_paths[0].items[0].clip_id, "00100");
  }

  #[test]
  fn ignores_non_entry_mark_and_bad_play_item_index() {
    let mut buf = build_mpls(&[("00001", 0, 450_000)], &[(0, 0), (9, 90_000)]);
    // Flip the first mark's type byte (offset within chapter block) to a
    // non-entry value to confirm it is skipped.
    // chapter_pos = 40 + playlist length; recompute by re-parsing positions.
    let chapter_pos = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;
    // first mark starts at chapter_pos + 4 + 2; type byte is at +1.
    let first_mark = chapter_pos + 6;
    buf[first_mark + 1] = 0x02; // not an entry mark
    let p = parse(&buf).unwrap();
    // First mark skipped (not entry), second mark references item 9 (missing).
    assert_eq!(p.chapter_count, 0);
  }
}
