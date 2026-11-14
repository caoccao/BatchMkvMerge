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

//! Ogg page parser per RFC 3533.
//!
//! Each page begins with:
//!
//! ```text
//! "OggS"                       4 bytes magic
//! u8  stream_structure_version (== 0)
//! u8  header_type_flag         (continuation=1, BOS=2, EOS=4)
//! u64 granule_position         (LE)
//! u32 bitstream_serial_number  (LE)
//! u32 page_sequence_number     (LE)
//! u32 page_checksum            (LE, CRC32 — we don't verify)
//! u8  number_of_page_segments
//! [u8] segment_table           (segments × 1 byte; sum = payload size)
//! ```
//!
//! Payload follows the segment table; packets are concatenations of
//! consecutive 255-byte segments terminated by a segment < 255 bytes.

use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

pub const HEADER_FLAG_CONTINUATION: u8 = 0x01;
pub const HEADER_FLAG_BEGINNING_OF_STREAM: u8 = 0x02;
pub const HEADER_FLAG_END_OF_STREAM: u8 = 0x04;

#[derive(Debug, Clone)]
pub struct PageHeader {
  pub start: u64,
  pub version: u8,
  pub header_type_flag: u8,
  pub granule_position: u64,
  pub bitstream_serial: u32,
  pub page_sequence: u32,
  pub crc32: u32,
  pub segments: Vec<u8>,
}

impl PageHeader {
  pub fn is_continuation(&self) -> bool {
    self.header_type_flag & HEADER_FLAG_CONTINUATION != 0
  }
  pub fn is_beginning_of_stream(&self) -> bool {
    self.header_type_flag & HEADER_FLAG_BEGINNING_OF_STREAM != 0
  }
  pub fn is_end_of_stream(&self) -> bool {
    self.header_type_flag & HEADER_FLAG_END_OF_STREAM != 0
  }
  pub fn payload_size(&self) -> u64 {
    self.segments.iter().map(|s| *s as u64).sum()
  }
  pub fn header_len(&self) -> u64 {
    27 + self.segments.len() as u64
  }
  pub fn total_size(&self) -> u64 {
    self.header_len() + self.payload_size()
  }
  pub fn payload_start(&self) -> u64 {
    self.start + self.header_len()
  }
  pub fn end(&self) -> u64 {
    self.start + self.total_size()
  }

  /// Split the segment table into individual packet byte counts.  A packet
  /// is the concatenation of one or more 255-byte segments terminated by a
  /// segment shorter than 255 bytes.  A page may end mid-packet, in which
  /// case the last entry's `continues_on_next_page` is `true`.
  pub fn packet_layout(&self) -> Vec<PacketSpan> {
    let mut packets = Vec::new();
    let mut current = 0u64;
    let mut started = false;
    for &len in &self.segments {
      current += len as u64;
      started = true;
      if len < 255 {
        packets.push(PacketSpan {
          bytes: current,
          continues_on_next_page: false,
        });
        current = 0;
        started = false;
      }
    }
    if started {
      packets.push(PacketSpan {
        bytes: current,
        continues_on_next_page: true,
      });
    }
    packets
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketSpan {
  pub bytes: u64,
  pub continues_on_next_page: bool,
}

/// Read one page header at the current cursor.  Advances past the header
/// (segment table inclusive) so the cursor lands on the first payload byte.
pub fn read_page_header(src: &mut FileSource) -> Result<PageHeader, ParseError> {
  let start = src.position();
  let magic = src.read_array::<4>()?;
  if &magic != b"OggS" {
    return Err(ParseError::Malformed {
      format: "ogg",
      offset: start,
      reason: format!("expected OggS, got {:?}", magic),
    });
  }
  let version = src.read_u8()?;
  let header_type_flag = src.read_u8()?;
  let granule_position = src.read_u64_le()?;
  let bitstream_serial = src.read_u32_le()?;
  let page_sequence = src.read_u32_le()?;
  let crc32 = src.read_u32_le()?;
  let n_segments = src.read_u8()?;
  let mut segments = vec![0u8; n_segments as usize];
  if n_segments > 0 {
    src.read_exact(&mut segments)?;
  }
  Ok(PageHeader {
    start,
    version,
    header_type_flag,
    granule_position,
    bitstream_serial,
    page_sequence,
    crc32,
    segments,
  })
}

/// Read the payload of one Ogg page into memory, capped against runaway
/// segment tables.
pub fn read_page_payload(src: &mut FileSource, header: &PageHeader, cap: u64) -> Result<Vec<u8>, ParseError> {
  let size = header.payload_size();
  if size > cap {
    return Err(ParseError::OversizedElement {
      format: "ogg",
      id: 0,
      size,
      cap,
      offset: header.start,
    });
  }
  let mut buf = vec![0u8; size as usize];
  src.read_exact(&mut buf)?;
  Ok(buf)
}

#[cfg(test)]
pub(crate) fn build_page(
  flags: u8,
  granule_position: u64,
  bitstream_serial: u32,
  page_sequence: u32,
  packets: &[&[u8]],
) -> Vec<u8> {
  let mut segments: Vec<u8> = Vec::new();
  let mut payload: Vec<u8> = Vec::new();
  for packet in packets {
    let mut remaining = packet.len();
    let mut offset = 0;
    while remaining >= 255 {
      segments.push(255);
      payload.extend_from_slice(&packet[offset..offset + 255]);
      offset += 255;
      remaining -= 255;
    }
    segments.push(remaining as u8);
    payload.extend_from_slice(&packet[offset..]);
  }

  let mut bytes = Vec::with_capacity(27 + segments.len() + payload.len());
  bytes.extend_from_slice(b"OggS");
  bytes.push(0); // version
  bytes.push(flags);
  bytes.extend_from_slice(&granule_position.to_le_bytes());
  bytes.extend_from_slice(&bitstream_serial.to_le_bytes());
  bytes.extend_from_slice(&page_sequence.to_le_bytes());
  bytes.extend_from_slice(&0u32.to_le_bytes()); // crc placeholder
  bytes.push(segments.len() as u8);
  bytes.extend_from_slice(&segments);
  bytes.extend_from_slice(&payload);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  #[test]
  fn reads_minimal_page_header() {
    let bytes = build_page(HEADER_FLAG_BEGINNING_OF_STREAM, 0, 0xCAFE, 0, &[b"hello"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    assert_eq!(h.bitstream_serial, 0xCAFE);
    assert!(h.is_beginning_of_stream());
    assert!(!h.is_continuation());
    assert!(!h.is_end_of_stream());
    assert_eq!(h.payload_size(), 5);
  }

  #[test]
  fn rejects_invalid_magic() {
    let bytes = b"FAKE\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec();
    let mut s = src(bytes);
    let err = read_page_header(&mut s).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn packet_layout_collapses_consecutive_255_segments() {
    // One packet of 600 bytes → 255 + 255 + 90
    let big = vec![0xAA; 600];
    let bytes = build_page(0, 0, 1, 0, &[&big]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    let layout = h.packet_layout();
    assert_eq!(layout.len(), 1);
    assert_eq!(layout[0].bytes, 600);
    assert!(!layout[0].continues_on_next_page);
  }

  #[test]
  fn packet_layout_separates_two_packets() {
    let bytes = build_page(0, 0, 1, 0, &[b"abc", b"defgh"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    let layout = h.packet_layout();
    assert_eq!(layout.len(), 2);
    assert_eq!(layout[0].bytes, 3);
    assert_eq!(layout[1].bytes, 5);
  }

  #[test]
  fn packet_layout_marks_continuation_when_page_ends_at_255() {
    // Hand-craft a page with segments = [255] (no terminator) — Ogg
    // convention for a packet that continues onto the next page.
    let mut bytes = b"OggS".to_vec();
    bytes.push(0); // version
    bytes.push(0); // flags
    bytes.extend_from_slice(&0u64.to_le_bytes()); // granule
    bytes.extend_from_slice(&1u32.to_le_bytes()); // serial
    bytes.extend_from_slice(&0u32.to_le_bytes()); // sequence
    bytes.extend_from_slice(&0u32.to_le_bytes()); // crc
    bytes.push(1); // 1 segment
    bytes.push(255); // segment table: just [255]
    bytes.extend_from_slice(&[0u8; 255]); // payload
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    let layout = h.packet_layout();
    assert_eq!(layout.len(), 1);
    assert_eq!(layout[0].bytes, 255);
    assert!(layout[0].continues_on_next_page);
  }

  #[test]
  fn read_page_payload_returns_concatenated_bytes() {
    let bytes = build_page(0, 0, 1, 0, &[b"abc", b"defgh"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    let payload = read_page_payload(&mut s, &h, 1024).unwrap();
    assert_eq!(payload, b"abcdefgh");
  }

  #[test]
  fn read_page_payload_caps_oversize() {
    let bytes = build_page(0, 0, 1, 0, &[&vec![0u8; 600]]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    let err = read_page_payload(&mut s, &h, 16).unwrap_err();
    assert!(matches!(err, ParseError::OversizedElement { .. }));
  }

  #[test]
  fn end_of_stream_flag_decoded() {
    let bytes = build_page(HEADER_FLAG_END_OF_STREAM, 12345, 1, 0, &[b"end"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    assert!(h.is_end_of_stream());
    assert_eq!(h.granule_position, 12345);
  }

  #[test]
  fn continuation_flag_decoded() {
    let bytes = build_page(HEADER_FLAG_CONTINUATION, 0, 1, 0, &[b"cont"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    assert!(h.is_continuation());
  }

  #[test]
  fn zero_segment_count_is_legal() {
    let bytes = build_page(0, 0, 1, 0, &[]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    assert_eq!(h.payload_size(), 0);
    assert!(h.packet_layout().is_empty());
  }

  #[test]
  fn header_len_matches_27_plus_segment_count() {
    let bytes = build_page(0, 0, 1, 0, &[b"abc", b"d"]);
    let mut s = src(bytes);
    let h = read_page_header(&mut s).unwrap();
    assert_eq!(h.header_len(), 27 + 2);
  }
}
