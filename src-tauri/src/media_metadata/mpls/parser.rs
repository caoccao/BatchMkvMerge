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
//! reader, exactly like mkvtoolnix.  We extract the facts mkvmerge surfaces in
//! its playlist identification: total duration, the ordered list of segment
//! clip ids, and the chapter count (after the trailing-mark drop rule).
//! Sub-paths and per-stream STN tables are skipped — the play-item `length`
//! field lets us seek straight past them, and the segment streams themselves
//! are identified by reading the referenced `.m2ts` file as MPEG-TS.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::bit_reader::BitReader;

/// Default Blu-ray mark-drop window: a final chapter within 5 s of the end is
/// dropped (mkvtoolnix's `m_drop_last_entry_if_at_end`).
const DROP_LAST_WINDOW_NS: i64 = 5_000_000_000;

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
}

/// The parsed playlist — only the fields mkvmerge reports at identification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Playlist {
  pub duration_ns: i64,
  pub items: Vec<PlayItem>,
  pub chapter_count: u32,
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
  let _sub_count = r.read_bits(16)?;

  for _ in 0..list_count {
    let item = parse_play_item(r, playlist.duration_ns)?;
    playlist.duration_ns += item.out_ns - item.in_ns;
    playlist.items.push(item);
  }
  // Sub-paths are skipped: they do not contribute to the playlist duration
  // and the chapter parser only references play items.
  Ok(())
}

fn parse_play_item(r: &mut BitReader, relative_in_ns: i64) -> Result<PlayItem, ParseError> {
  let length = r.read_bits(16)?;
  let position = r.position_bytes();

  let clip_id = read_string(r, 5)?;
  let codec_id = read_string(r, 4)?;
  r.skip_bits(11)?; // reserved
  let _is_multi_angle = r.read_bit()?;
  let _connection_condition = r.read_bits(4)?;
  let _stc_id = r.read_bits(8)?;
  let in_ns = ticks_to_ns(r.read_bits(32)?);
  let out_ns = ticks_to_ns(r.read_bits(32)?);

  // Seek straight to the end of this play item (skips UO mask, multi-angle
  // clips, and the STN table that mkvtoolnix decodes for stream details).
  r.set_bit_position((position + length) * 8);

  Ok(PlayItem {
    clip_id,
    codec_id,
    in_ns,
    out_ns,
    relative_in_ns,
  })
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
      item.push(0x00); // 3 reserved + multi_angle + conn(4) = byte
      item.push(0x00); // stc_id
      item.extend(in_t.to_be_bytes());
      item.extend(out_t.to_be_bytes());
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
