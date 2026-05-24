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

/// Number of bytes inside the PES packet that precede the actual payload.
/// For MPEG-2 PES packets (`stream_id` not in the static-content set), this
/// is `6 + (pes_header_data_length + 3)`.  We only need this to skip past
/// the metadata when scanning for the next start code.
pub fn pes_payload_offset(bytes: &[u8]) -> usize {
    if bytes.len() < 9 {
        return bytes.len();
    }
    // bytes[6..9] = mark bits + flags + header_data_length
    let data_len = bytes[8] as usize;
    9 + data_len
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
        // bytes[8] = 5 → offset = 9 + 5 = 14
        let bytes = [0x00, 0x00, 0x01, 0xE0, 0x00, 0x20, 0x80, 0x80, 5];
        assert_eq!(pes_payload_offset(&bytes), 14);
    }

    #[test]
    fn pes_payload_offset_for_short_buffer() {
        assert_eq!(pes_payload_offset(&[0x00, 0x00, 0x01, 0xE0]), 4);
    }
}
