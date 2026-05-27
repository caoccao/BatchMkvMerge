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

//! RealMedia top-level chunk walker.  Every object on disk starts with a
//! 10-byte common header (4-byte FOURCC id, 4-byte BE size, 2-byte BE
//! version), followed by an id-specific payload.

pub const RMF_MAGIC: [u8; 4] = *b".RMF";
pub const ID_PROP: [u8; 4] = *b"PROP";
pub const ID_CONT: [u8; 4] = *b"CONT";
pub const ID_MDPR: [u8; 4] = *b"MDPR";
pub const ID_DATA: [u8; 4] = *b"DATA";
pub const COMMON_HEADER_LEN: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkHeader {
  pub id: [u8; 4],
  pub size: u32,
  pub version: u16,
}

impl ChunkHeader {
  pub fn parse(bytes: &[u8]) -> Option<Self> {
    if bytes.len() < COMMON_HEADER_LEN {
      return None;
    }
    Some(Self {
      id: [bytes[0], bytes[1], bytes[2], bytes[3]],
      size: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
      version: u16::from_be_bytes([bytes[8], bytes[9]]),
    })
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropChunk {
  pub max_bit_rate: u32,
  pub avg_bit_rate: u32,
  pub max_packet_size: u32,
  pub avg_packet_size: u32,
  pub num_packets: u32,
  pub duration_ms: u32,
  pub preroll: u32,
  pub index_offset: u32,
  pub data_offset: u32,
  pub num_streams: u16,
  pub flags: u16,
}

impl PropChunk {
  pub const PAYLOAD_LEN: usize = 4 * 9 + 2 * 2;

  pub fn parse(payload: &[u8]) -> Option<Self> {
    Self::parse_with_consumed(payload).map(|(chunk, _)| chunk)
  }

  pub fn parse_with_consumed(payload: &[u8]) -> Option<(Self, usize)> {
    if payload.len() < Self::PAYLOAD_LEN {
      return None;
    }
    let r = ChunkReader::new(payload);
    Some((Self {
      max_bit_rate: r.u32(0),
      avg_bit_rate: r.u32(4),
      max_packet_size: r.u32(8),
      avg_packet_size: r.u32(12),
      num_packets: r.u32(16),
      duration_ms: r.u32(20),
      preroll: r.u32(24),
      index_offset: r.u32(28),
      data_offset: r.u32(32),
      num_streams: r.u16(36),
      flags: r.u16(38),
    }, Self::PAYLOAD_LEN))
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContChunk {
  pub title: String,
  pub author: String,
  pub copyright: String,
  pub comment: String,
}

impl ContChunk {
  pub fn parse(payload: &[u8]) -> Option<Self> {
    Self::parse_with_consumed(payload).map(|(chunk, _)| chunk)
  }

  pub fn parse_with_consumed(payload: &[u8]) -> Option<(Self, usize)> {
    let mut r = ChunkReader::new(payload);
    let chunk = Self {
      title: r.length_prefixed_string_u16()?,
      author: r.length_prefixed_string_u16()?,
      copyright: r.length_prefixed_string_u16()?,
      comment: r.length_prefixed_string_u16()?,
    };
    Some((chunk, r.position()))
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MdprChunk {
  pub stream_number: u16,
  pub max_bit_rate: u32,
  pub avg_bit_rate: u32,
  pub max_packet_size: u32,
  pub avg_packet_size: u32,
  pub start_time_ms: u32,
  pub preroll: u32,
  pub duration_ms: u32,
  pub stream_name: String,
  pub mime_type: String,
  pub type_specific_data: Vec<u8>,
}

impl MdprChunk {
  pub fn parse(payload: &[u8]) -> Option<Self> {
    Self::parse_with_consumed(payload).map(|(chunk, _)| chunk)
  }

  pub fn parse_with_consumed(payload: &[u8]) -> Option<(Self, usize)> {
    let mut r = ChunkReader::new(payload);
    let stream_number = r.read_u16()?;
    let max_bit_rate = r.read_u32()?;
    let avg_bit_rate = r.read_u32()?;
    let max_packet_size = r.read_u32()?;
    let avg_packet_size = r.read_u32()?;
    let start_time_ms = r.read_u32()?;
    let preroll = r.read_u32()?;
    let duration_ms = r.read_u32()?;
    let stream_name = r.length_prefixed_string_u8()?;
    let mime_type = r.length_prefixed_string_u8()?;
    let ts_len = r.read_u32()? as usize;
    let type_specific_data = r.read_bytes(ts_len)?.to_vec();
    let chunk = Self {
      stream_number,
      max_bit_rate,
      avg_bit_rate,
      max_packet_size,
      avg_packet_size,
      start_time_ms,
      preroll,
      duration_ms,
      stream_name,
      mime_type,
      type_specific_data,
    };
    Some((chunk, r.position()))
  }
}

/// Lightweight forward-only byte slice reader used by the chunk decoders.
/// Returns `Option<...>` on short reads so the walk can bail cleanly when a
/// chunk is truncated.
pub(crate) struct ChunkReader<'a> {
  bytes: &'a [u8],
  pos: usize,
}

impl<'a> ChunkReader<'a> {
  pub(crate) fn new(bytes: &'a [u8]) -> Self {
    Self { bytes, pos: 0 }
  }

  fn u16(&self, off: usize) -> u16 {
    u16::from_be_bytes([self.bytes[off], self.bytes[off + 1]])
  }
  fn u32(&self, off: usize) -> u32 {
    u32::from_be_bytes([
      self.bytes[off],
      self.bytes[off + 1],
      self.bytes[off + 2],
      self.bytes[off + 3],
    ])
  }

  pub(crate) fn read_u8(&mut self) -> Option<u8> {
    if self.pos + 1 > self.bytes.len() {
      return None;
    }
    let v = self.bytes[self.pos];
    self.pos += 1;
    Some(v)
  }
  pub(crate) fn read_u16(&mut self) -> Option<u16> {
    if self.pos + 2 > self.bytes.len() {
      return None;
    }
    let v = self.u16(self.pos);
    self.pos += 2;
    Some(v)
  }
  pub(crate) fn read_u32(&mut self) -> Option<u32> {
    if self.pos + 4 > self.bytes.len() {
      return None;
    }
    let v = self.u32(self.pos);
    self.pos += 4;
    Some(v)
  }
  pub(crate) fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
    if self.pos + n > self.bytes.len() {
      return None;
    }
    let slice = &self.bytes[self.pos..self.pos + n];
    self.pos += n;
    Some(slice)
  }

  pub(crate) fn position(&self) -> usize {
    self.pos
  }

  fn length_prefixed_string_u8(&mut self) -> Option<String> {
    let len = self.read_u8()? as usize;
    let raw = self.read_bytes(len)?;
    Some(String::from_utf8_lossy(raw).into_owned())
  }
  fn length_prefixed_string_u16(&mut self) -> Option<String> {
    let len = self.read_u16()? as usize;
    let raw = self.read_bytes(len)?;
    Some(String::from_utf8_lossy(raw).into_owned())
  }
}

#[cfg(test)]
pub(crate) fn build_chunk(id: [u8; 4], version: u16, payload: &[u8]) -> Vec<u8> {
  let mut buf = Vec::with_capacity(COMMON_HEADER_LEN + payload.len());
  buf.extend_from_slice(&id);
  let size = (COMMON_HEADER_LEN + payload.len()) as u32;
  buf.extend_from_slice(&size.to_be_bytes());
  buf.extend_from_slice(&version.to_be_bytes());
  buf.extend_from_slice(payload);
  buf
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_chunk_header() {
    let bytes = [b'P', b'R', b'O', b'P', 0, 0, 0, 0x42, 0, 1];
    let h = ChunkHeader::parse(&bytes).unwrap();
    assert_eq!(h.id, *b"PROP");
    assert_eq!(h.size, 0x42);
    assert_eq!(h.version, 1);
  }

  #[test]
  fn chunk_header_rejects_short_input() {
    assert!(ChunkHeader::parse(&[0u8; 4]).is_none());
  }

  #[test]
  fn parses_prop_chunk_payload() {
    let mut payload = Vec::new();
    for n in 0u32..9 {
      payload.extend_from_slice(&n.to_be_bytes());
    }
    payload.extend_from_slice(&5u16.to_be_bytes()); // num_streams
    payload.extend_from_slice(&7u16.to_be_bytes()); // flags
    let p = PropChunk::parse(&payload).unwrap();
    assert_eq!(p.max_bit_rate, 0);
    assert_eq!(p.duration_ms, 5);
    assert_eq!(p.num_streams, 5);
    assert_eq!(p.flags, 7);
  }

  #[test]
  fn prop_chunk_rejects_short_payload() {
    assert!(PropChunk::parse(&[0u8; 16]).is_none());
  }

  #[test]
  fn parses_cont_chunk_payload() {
    let mut payload = Vec::new();
    for s in ["Title", "Author", "©2026", "A comment"] {
      payload.extend_from_slice(&(s.len() as u16).to_be_bytes());
      payload.extend_from_slice(s.as_bytes());
    }
    let c = ContChunk::parse(&payload).unwrap();
    assert_eq!(c.title, "Title");
    assert_eq!(c.author, "Author");
    assert_eq!(c.copyright, "©2026");
    assert_eq!(c.comment, "A comment");
  }

  #[test]
  fn parses_mdpr_chunk_payload() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&3u16.to_be_bytes()); // stream_number
    payload.extend_from_slice(&100_000u32.to_be_bytes());
    payload.extend_from_slice(&90_000u32.to_be_bytes());
    payload.extend_from_slice(&2048u32.to_be_bytes());
    payload.extend_from_slice(&1024u32.to_be_bytes());
    payload.extend_from_slice(&0u32.to_be_bytes()); // start_time
    payload.extend_from_slice(&0u32.to_be_bytes()); // preroll
    payload.extend_from_slice(&60_000u32.to_be_bytes()); // duration_ms
    // stream_name (5 bytes "audio")
    payload.push(5);
    payload.extend_from_slice(b"audio");
    // mime_type
    let mt = b"audio/x-pn-realaudio";
    payload.push(mt.len() as u8);
    payload.extend_from_slice(mt);
    // type_specific_size + data
    payload.extend_from_slice(&8u32.to_be_bytes());
    payload.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

    let m = MdprChunk::parse(&payload).unwrap();
    assert_eq!(m.stream_number, 3);
    assert_eq!(m.duration_ms, 60_000);
    assert_eq!(m.stream_name, "audio");
    assert_eq!(m.mime_type, "audio/x-pn-realaudio");
    assert_eq!(m.type_specific_data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
  }

  #[test]
  fn mdpr_chunk_rejects_truncated_payload() {
    // Truncated after stream_number — every read_* helper must short-circuit.
    let payload = 3u16.to_be_bytes();
    assert!(MdprChunk::parse(&payload).is_none());
  }

  #[test]
  fn build_chunk_round_trips_via_chunk_header_parse() {
    let payload = vec![0xDEu8; 16];
    let bytes = build_chunk(ID_PROP, 0, &payload);
    let h = ChunkHeader::parse(&bytes).unwrap();
    assert_eq!(h.id, ID_PROP);
    assert_eq!(h.size as usize, COMMON_HEADER_LEN + payload.len());
  }
}
