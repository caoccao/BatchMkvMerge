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

//! MPEG-PS PES packet header decoder.  Same wire format as the MPEG-TS PES
//! header — different surrounding context (no transport packetisation).

use crate::media_metadata::error::ParseError;

#[derive(Debug, Clone, Copy)]
pub struct PesHeader {
  pub stream_id: u8,
  pub packet_length: u16,
}

pub fn parse(bytes: &[u8]) -> Result<PesHeader, ParseError> {
  if bytes.len() < 6 {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: format!("PES header {} bytes too small", bytes.len()),
    });
  }
  if !(bytes[0] == 0x00 && bytes[1] == 0x00 && bytes[2] == 0x01) {
    return Err(ParseError::Malformed {
      format: "mpeg_ps",
      offset: 0,
      reason: "missing PES start code".to_string(),
    });
  }
  Ok(PesHeader {
    stream_id: bytes[3],
    packet_length: u16::from_be_bytes([bytes[4], bytes[5]]),
  })
}

/// Offset within the PES packet of the first elementary-payload byte (for
/// `0xBD` private-stream-1 packets this is the sub-stream-id byte the caller
/// reads next).
///
/// PARSER-272: this is a port of `mpeg_ps_reader_c::parse_packet`'s
/// depacketiser (`r_mpeg_ps.cpp:343-468`), which supports both the **MPEG-1**
/// and the **MPEG-2** PES optional-header layouts.  Earlier code assumed the
/// MPEG-2 shape unconditionally (`9 + header_data_length`); for MPEG-1 program
/// streams that has no `PES_header_data_length` at byte 8, so stuffing bytes,
/// a PTS/DTS field, or elementary data was misread as a header length.
///
/// Starting at byte 6 the parser, exactly as upstream:
///  1. skips `0xff` stuffing bytes,
///  2. skips a 2-byte STD buffer size when `c & 0xc0 == 0x40`,
///  3. consumes the MPEG-1 PTS (`c & 0xf0 == 0x20`, 4 more bytes) or PTS+DTS
///     (`c & 0xf0 == 0x30`, 9 more bytes), the MPEG-2 optional header
///     (`c & 0xc0 == 0x80`, `flags + header_data_length + that many bytes`),
///     or — for the MPEG-1 no-timestamp marker `0x0f` — nothing,
///  4. and returns at the first unrecognised marker (no elementary payload).
///
/// All bounds are taken from the declared `packet_length`; when it is zero
/// (unbounded MPEG-2 video) the available buffer is used instead.
pub fn pes_payload_offset(bytes: &[u8]) -> usize {
  if bytes.len() < 7 {
    return bytes.len();
  }
  let declared = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
  let avail = bytes.len() - 6;
  // `m_length` counts the bytes that follow the 6-byte PES prefix.
  let mut len = if declared == 0 { avail } else { declared.min(avail) };
  let end = (6 + len).min(bytes.len());
  let mut p = 6usize;

  // Skip stuffing bytes (`0xff`); `c` is the first non-stuffing byte and has
  // already been consumed by the time the loop exits.
  let mut c = 0u8;
  let mut consumed = false;
  while len > 0 {
    c = bytes[p];
    p += 1;
    len -= 1;
    consumed = true;
    if c != 0xff {
      break;
    }
  }
  if !consumed {
    return end;
  }

  // Skip the STD buffer size (`01xxxxxx`): one ignored byte + the next marker.
  if (c & 0xc0) == 0x40 {
    if len < 2 {
      return end;
    }
    len -= 2;
    c = bytes[p + 1];
    p += 2;
  }

  if (c & 0xf0) == 0x20 {
    // MPEG-1 PTS only: 4 additional bytes.
    if len < 4 {
      return end;
    }
    p += 4;
  } else if (c & 0xf0) == 0x30 {
    // MPEG-1 PTS + DTS: 9 additional bytes.
    if len < 9 {
      return end;
    }
    p += 9;
  } else if (c & 0xc0) == 0x80 {
    // MPEG-2 optional header: flags(1) + header_data_length(1) + that many.
    if len < 2 {
      return end;
    }
    let hdrlen = bytes[p + 1] as usize;
    p += 2;
    len -= 2;
    if hdrlen > len {
      return end;
    }
    p += hdrlen;
  } else if c != 0x0f {
    // Unrecognised marker — upstream returns without exposing payload.
    return end;
  }
  // `c == 0x0f` (MPEG-1, no timestamps) falls through: payload starts at `p`.

  p.min(bytes.len())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn decodes_video_pes_header() {
    let bytes = [0x00, 0x00, 0x01, 0xE0, 0x00, 0x0A];
    let h = parse(&bytes).unwrap();
    assert_eq!(h.stream_id, 0xE0);
    assert_eq!(h.packet_length, 0x0A);
  }

  #[test]
  fn rejects_missing_start_code() {
    let err = parse(&[0xFF; 6]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn rejects_truncated() {
    let err = parse(&[0x00, 0x00, 0x01]).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn pes_payload_offset_for_full_header() {
    // MPEG-2 optional header: marker 0x80, flags 0x80, header_data_length 5,
    // then 5 header bytes and 6 payload bytes → payload starts at 6+1+2+5 = 14.
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x20, 0x80, 0x80, 5];
    bytes.extend_from_slice(&[0u8; 5]); // PES header bytes (PTS/DTS/extensions)
    bytes.extend_from_slice(&[0xAA; 6]); // elementary payload
    assert_eq!(pes_payload_offset(&bytes), 14);
  }

  #[test]
  fn pes_payload_offset_for_short_buffer() {
    assert_eq!(pes_payload_offset(&[0x00, 0x00, 0x01, 0xE0]), 4);
  }

  // ---- PARSER-272: MPEG-1 PES optional-header layouts ------------------

  #[test]
  fn pes_payload_offset_mpeg1_pts_only() {
    // Marker `0x21` (`c & 0xf0 == 0x20`) + 4 PTS bytes → payload at 6+1+4 = 11.
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x14];
    bytes.extend_from_slice(&[0x21, 0x11, 0x11, 0x11, 0x11]); // PTS marker + value
    bytes.extend_from_slice(&[0xAA; 9]); // elementary payload
    assert_eq!(pes_payload_offset(&bytes), 11);
  }

  #[test]
  fn pes_payload_offset_mpeg1_pts_dts() {
    // Marker `0x31` (`c & 0xf0 == 0x30`) + 9 PTS/DTS bytes → payload at 6+1+9.
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x1E];
    bytes.push(0x31);
    bytes.extend_from_slice(&[0x11; 9]); // PTS (4) + marker (1) + DTS (4)
    bytes.extend_from_slice(&[0xAA; 8]);
    assert_eq!(pes_payload_offset(&bytes), 16);
  }

  #[test]
  fn pes_payload_offset_mpeg1_no_timestamp_marker() {
    // Marker `0x0f` (MPEG-1, no timestamps) → payload directly after it.
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x09, 0x0F];
    bytes.extend_from_slice(&[0xAA; 8]);
    assert_eq!(pes_payload_offset(&bytes), 7);
  }

  #[test]
  fn pes_payload_offset_skips_stuffing_before_mpeg2_header() {
    // Two `0xff` stuffing bytes precede the MPEG-2 marker; header_data_length
    // is 2 → payload at 6+2(stuffing)+1(marker)+2(flags+len)+2(hdr) = 13.
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x20];
    bytes.extend_from_slice(&[0xFF, 0xFF, 0x80, 0x80, 0x02, 0x00, 0x00]);
    bytes.extend_from_slice(&[0xAA; 6]);
    assert_eq!(pes_payload_offset(&bytes), 13);
  }

  #[test]
  fn pes_payload_offset_unknown_marker_yields_no_payload() {
    // A marker matching no MPEG-1/MPEG-2 shape (`0x00`, not `0x0f`) exposes no
    // elementary payload — upstream returns at the final `else`, so the offset
    // is the end of the declared packet (`6 + packet_length`).
    let mut bytes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x08];
    bytes.extend_from_slice(&[0u8; 8]); // full declared payload, marker byte 0x00
    assert_eq!(pes_payload_offset(&bytes), 6 + 0x08);
  }
}
