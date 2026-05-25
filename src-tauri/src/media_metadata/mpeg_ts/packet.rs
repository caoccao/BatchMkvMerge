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

//! MPEG-TS packet parser.
//!
//! Layout (188-byte packet, ISO/IEC 13818-1 §2.4.3.2):
//!
//! ```text
//! 0x47                                            (sync byte)
//! 3 bits: transport_error_indicator | payload_unit_start_indicator | transport_priority
//! 13 bits: PID
//! 2 bits: transport_scrambling_control
//! 2 bits: adaptation_field_control     (1 = payload only, 2 = adaptation only,
//!                                       3 = adaptation + payload)
//! 4 bits: continuity_counter
//! [adaptation_field if any]
//! [payload to end of packet]
//! ```
//!
//! Three packet sizes seen in the wild:
//! - 188 bytes — standard MPEG-TS.
//! - 192 bytes — BD M2TS (4-byte timecode prefix per packet).
//! - 204 bytes — FEC-extended.
//!
//! [`detect_packet_size`] sniffs the first 1024 bytes to figure out which.

use crate::media_metadata::error::ParseError;

pub const TS_SYNC_BYTE: u8 = 0x47;
pub const PACKET_SIZE_STANDARD: usize = 188;
pub const PACKET_SIZE_BD_M2TS: usize = 192;
pub const PACKET_SIZE_FEC: usize = 204;

/// One MPEG-TS packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
  /// 13-bit PID (Packet IDentifier).
  pub pid: u16,
  pub transport_error: bool,
  pub payload_unit_start: bool,
  pub transport_priority: bool,
  pub scrambling: u8,
  /// 2-bit adaptation_field_control.
  pub adaptation_field_control: u8,
  pub continuity_counter: u8,
}

impl PacketHeader {
  pub fn has_adaptation_field(&self) -> bool {
    matches!(self.adaptation_field_control, 2 | 3)
  }
  pub fn has_payload(&self) -> bool {
    matches!(self.adaptation_field_control, 1 | 3)
  }
}

/// Decode the 4-byte packet header starting at index 0 of `packet`.
pub fn decode_header(packet: &[u8]) -> Result<PacketHeader, ParseError> {
  if packet.len() < 4 {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("packet {} bytes too small for header", packet.len()),
    });
  }
  if packet[0] != TS_SYNC_BYTE {
    return Err(ParseError::Malformed {
      format: "mpeg_ts",
      offset: 0,
      reason: format!("missing sync byte (got 0x{:02X})", packet[0]),
    });
  }
  let pid = ((packet[1] as u16 & 0x1F) << 8) | packet[2] as u16;
  Ok(PacketHeader {
    pid,
    transport_error: packet[1] & 0x80 != 0,
    payload_unit_start: packet[1] & 0x40 != 0,
    transport_priority: packet[1] & 0x20 != 0,
    scrambling: (packet[3] >> 6) & 0x03,
    adaptation_field_control: (packet[3] >> 4) & 0x03,
    continuity_counter: packet[3] & 0x0F,
  })
}

/// Return a slice over the packet's payload (after any adaptation field).
/// Returns an empty slice if the packet carries no payload.
pub fn payload_slice<'a>(packet: &'a [u8], header: &PacketHeader) -> &'a [u8] {
  if !header.has_payload() {
    return &[];
  }
  let mut offset = 4usize;
  if header.has_adaptation_field() {
    if offset >= packet.len() {
      return &[];
    }
    let adaptation_length = packet[offset] as usize;
    offset += 1 + adaptation_length;
    if offset >= packet.len() {
      return &[];
    }
  }
  &packet[offset..]
}

/// Number of consecutive in-stride sync bytes required to lock onto a packet
/// size — mirrors mkvtoolnix's many-matching-sync-bytes requirement.
const SYNC_CONFIRMATIONS: usize = 5;
/// How far into the probe we look for the first aligned packet (tolerates
/// leading garbage / partial packets).
const MAX_ALIGN_SCAN: usize = 64 * 1024;

/// Detect the packet size *and* the byte offset of the first whole packet,
/// scanning for an alignment where [`SYNC_CONFIRMATIONS`] consecutive packets
/// all carry a sync byte (PARSER-053). For BD M2TS the sync sits 4 bytes into
/// each 192-byte unit; the returned offset is the unit start.
pub fn detect_packet_size_aligned(probe: &[u8]) -> Option<(usize, usize)> {
  for &size in &[PACKET_SIZE_STANDARD, PACKET_SIZE_BD_M2TS, PACKET_SIZE_FEC] {
    let sync_off = if size == PACKET_SIZE_BD_M2TS { 4 } else { 0 };
    let need = size * (SYNC_CONFIRMATIONS - 1) + sync_off + 1;
    if probe.len() < need {
      continue;
    }
    let max_start = probe.len().saturating_sub(need).min(MAX_ALIGN_SCAN);
    for start in 0..=max_start {
      let aligned = (0..SYNC_CONFIRMATIONS).all(|k| probe.get(start + k * size + sync_off) == Some(&TS_SYNC_BYTE));
      if aligned {
        return Some((size, start));
      }
    }
  }
  None
}

/// Sniff just the packet size (used by the probe). See
/// [`detect_packet_size_aligned`].
pub fn detect_packet_size(probe: &[u8]) -> Option<usize> {
  detect_packet_size_aligned(probe).map(|(size, _)| size)
}

/// `true` for the conventional system-PID values mkvtoolnix recognises.
pub fn is_pat_pid(pid: u16) -> bool {
  pid == 0
}

#[cfg(test)]
pub(crate) fn build_packet(pid: u16, payload_unit_start: bool, payload: &[u8]) -> Vec<u8> {
  let mut p = Vec::with_capacity(PACKET_SIZE_STANDARD);
  p.push(TS_SYNC_BYTE);
  let b1 = ((payload_unit_start as u8) << 6) | ((pid >> 8) as u8 & 0x1F);
  p.push(b1);
  p.push((pid & 0xFF) as u8);
  p.push(0x10); // adaptation_field_control = 1 (payload only), CC = 0
  p.extend_from_slice(payload);
  while p.len() < PACKET_SIZE_STANDARD {
    p.push(0xFF); // stuffing
  }
  p
}

#[cfg(test)]
pub(crate) fn build_packet_with_pointer(pid: u16, payload: &[u8]) -> Vec<u8> {
  // pointer_field = 0 (PSI section starts immediately).
  let mut full = vec![0u8];
  full.extend_from_slice(payload);
  build_packet(pid, true, &full)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn decodes_standard_packet_header() {
    let pkt = build_packet(0x1FFF, true, &[]);
    let h = decode_header(&pkt).unwrap();
    assert_eq!(h.pid, 0x1FFF);
    assert!(h.payload_unit_start);
    assert!(h.has_payload());
    assert!(!h.has_adaptation_field());
  }

  #[test]
  fn rejects_packet_without_sync_byte() {
    let pkt = vec![0x00; 188];
    let err = decode_header(&pkt).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_truncated_packet() {
    let err = decode_header(&[0x47, 0x00]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn pid_zero_is_pat() {
    assert!(is_pat_pid(0));
    assert!(!is_pat_pid(0x100));
  }

  #[test]
  fn payload_slice_excludes_adaptation_field() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x40; // payload_unit_start = 1, PID = 0
    pkt[2] = 0x00;
    pkt[3] = 0x30; // adaptation_field_control = 3 (both), CC = 0
    pkt[4] = 3; // adaptation_length
    pkt[5..8].copy_from_slice(&[0xAA, 0xBB, 0xCC]); // adaptation
    pkt[8] = 0x12; // payload starts here
    let h = decode_header(&pkt).unwrap();
    let payload = payload_slice(&pkt, &h);
    assert_eq!(payload[0], 0x12);
  }

  #[test]
  fn payload_slice_returns_empty_for_adaptation_only() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x00;
    pkt[2] = 0x00;
    pkt[3] = 0x20; // adaptation_field_control = 2 (adaptation only)
    let h = decode_header(&pkt).unwrap();
    let payload = payload_slice(&pkt, &h);
    assert!(payload.is_empty());
  }

  #[test]
  fn detect_packet_size_finds_188() {
    let mut probe = Vec::new();
    for _ in 0..8 {
      probe.extend(build_packet(0, false, &[]));
    }
    assert_eq!(detect_packet_size(&probe), Some(PACKET_SIZE_STANDARD));
  }

  #[test]
  fn detect_packet_size_finds_192_bd_m2ts() {
    let mut probe = Vec::new();
    for _ in 0..8 {
      probe.extend_from_slice(&[0u8; 4]); // 4-byte timecode prefix
      probe.extend(build_packet(0, false, &[]));
    }
    assert_eq!(detect_packet_size(&probe), Some(PACKET_SIZE_BD_M2TS));
  }

  #[test]
  fn detect_packet_size_returns_none_on_garbage() {
    assert!(detect_packet_size(&[0xFFu8; 1024]).is_none());
  }

  #[test]
  fn payload_slice_empty_when_no_payload_bit_set() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[3] = 0x00; // adaptation_field_control = 0 (reserved, but no payload)
    let h = decode_header(&pkt).unwrap();
    assert!(payload_slice(&pkt, &h).is_empty());
  }

  #[test]
  fn transport_error_flag_decoded() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x80;
    pkt[3] = 0x10;
    let h = decode_header(&pkt).unwrap();
    assert!(h.transport_error);
  }

  #[test]
  fn continuity_counter_extracted() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[3] = 0x1B; // adaptation = 1, CC = 11
    let h = decode_header(&pkt).unwrap();
    assert_eq!(h.continuity_counter, 11);
  }

  #[test]
  fn adaptation_length_overrun_returns_empty_payload() {
    let mut pkt = vec![0u8; PACKET_SIZE_STANDARD];
    pkt[0] = TS_SYNC_BYTE;
    pkt[3] = 0x30; // both
    pkt[4] = 250; // adaptation_length > remaining bytes
    let h = decode_header(&pkt).unwrap();
    assert!(payload_slice(&pkt, &h).is_empty());
  }
}
