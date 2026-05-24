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

//! Bit-level reader over a byte slice. Used for AVC/HEVC SPS, AAC ASC, FLAC
//! STREAMINFO, etc. Mirrors `mkvtoolnix/src/common/bit_reader.{h,cpp}` with
//! Exp-Golomb and emulation-prevention helpers added.

use super::super::error::ParseError;

#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    /// Bit offset from the start of `bytes` (so byte index = bit_pos / 8).
    bit_pos: u64,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_pos: 0 }
    }

    /// Construct from a slice with emulation-prevention bytes already removed.
    /// Convenience constructor so call sites read more naturally.
    pub fn from_rbsp(rbsp: &'a [u8]) -> Self {
        Self::new(rbsp)
    }

    pub fn position_bits(&self) -> u64 {
        self.bit_pos
    }

    /// Absolute seek to a bit offset. Mirrors `mtx::bits::reader_c::set_bit_position`.
    /// The position may exceed the buffer; subsequent reads then fail with
    /// `UnexpectedEof`, matching the C++ behaviour of throwing on the next read.
    pub fn set_bit_position(&mut self, pos: u64) {
        self.bit_pos = pos;
    }

    pub fn position_bytes(&self) -> u64 {
        self.bit_pos / 8
    }

    pub fn total_bits(&self) -> u64 {
        (self.bytes.len() as u64) * 8
    }

    pub fn remaining_bits(&self) -> u64 {
        self.total_bits().saturating_sub(self.bit_pos)
    }

    pub fn eos(&self) -> bool {
        self.bit_pos >= self.total_bits()
    }

    /// Skip `n` bits forward. `Err(UnexpectedEof)` if the request runs off
    /// the end of the buffer.
    pub fn skip_bits(&mut self, n: u64) -> Result<(), ParseError> {
        let new = self.bit_pos.checked_add(n).ok_or(ParseError::UnexpectedEof {
            offset: self.bit_pos / 8,
            wanted: n.saturating_add(7) / 8,
        })?;
        if new > self.total_bits() {
            return Err(ParseError::UnexpectedEof {
                offset: self.bit_pos / 8,
                wanted: (new - self.total_bits() + 7) / 8,
            });
        }
        self.bit_pos = new;
        Ok(())
    }

    /// Move the bit cursor forward until byte-aligned. No-op if already so.
    pub fn align_to_byte(&mut self) {
        let r = self.bit_pos % 8;
        if r != 0 {
            self.bit_pos += 8 - r;
        }
    }

    /// Read `n` bits (1..=64) and return them right-justified in a `u64`.
    pub fn read_bits(&mut self, n: u32) -> Result<u64, ParseError> {
        assert!(n <= 64, "read_bits n > 64");
        if n == 0 {
            return Ok(0);
        }
        if (self.bit_pos + n as u64) > self.total_bits() {
            return Err(ParseError::UnexpectedEof {
                offset: self.bit_pos / 8,
                wanted: ((n as u64 + 7) / 8).max(1),
            });
        }

        let mut acc: u64 = 0;
        let mut remaining = n;
        while remaining > 0 {
            let byte_idx = (self.bit_pos / 8) as usize;
            let bit_off = (self.bit_pos % 8) as u32;
            let avail_in_byte = 8 - bit_off;
            let take = remaining.min(avail_in_byte);
            let shift = avail_in_byte - take;
            let mask = ((1u32 << take) - 1) as u8;
            let chunk = (self.bytes[byte_idx] >> shift) & mask;
            acc = (acc << take) | (chunk as u64);
            self.bit_pos += take as u64;
            remaining -= take;
        }
        Ok(acc)
    }

    /// Read a single bit as a `bool`. Convenience over `read_bits(1)`.
    pub fn read_bit(&mut self) -> Result<bool, ParseError> {
        Ok(self.read_bits(1)? != 0)
    }

    /// Read N bytes byte-aligned. Returns `Malformed` if the cursor isn't on
    /// a byte boundary — callers should `align_to_byte()` first when they
    /// expect this.
    pub fn read_bytes_aligned(&mut self, n: usize) -> Result<&'a [u8], ParseError> {
        if self.bit_pos % 8 != 0 {
            return Err(ParseError::Malformed {
                format: "bit_reader",
                offset: self.bit_pos / 8,
                reason: format!("read_bytes_aligned at bit-offset {}", self.bit_pos % 8),
            });
        }
        let start = (self.bit_pos / 8) as usize;
        let end = start.checked_add(n).ok_or(ParseError::UnexpectedEof {
            offset: start as u64,
            wanted: n as u64,
        })?;
        if end > self.bytes.len() {
            return Err(ParseError::UnexpectedEof {
                offset: start as u64,
                wanted: (end - self.bytes.len()) as u64,
            });
        }
        let out = &self.bytes[start..end];
        self.bit_pos += (n as u64) * 8;
        Ok(out)
    }

    // ---- Exp-Golomb (used by H.264/H.265 SPS, VPS, PPS) -----------------

    /// Unsigned Exp-Golomb (`ue(v)`). Reads a run of zero bits + a single 1
    /// + that many trailing bits. Capped at 32 leading zeros for sanity.
    pub fn read_ue(&mut self) -> Result<u32, ParseError> {
        let mut zeros = 0u32;
        while !self.eos() {
            if self.read_bit()? {
                let extra = self.read_bits(zeros)? as u32;
                let val = ((1u32 << zeros).wrapping_sub(1)).wrapping_add(extra);
                return Ok(val);
            }
            zeros += 1;
            if zeros > 32 {
                return Err(ParseError::Malformed {
                    format: "bit_reader",
                    offset: self.bit_pos / 8,
                    reason: "ue(v) with > 32 leading zeros".to_string(),
                });
            }
        }
        Err(ParseError::UnexpectedEof {
            offset: self.bit_pos / 8,
            wanted: 1,
        })
    }

    /// Signed Exp-Golomb (`se(v)`). The unsigned codeword is remapped to a
    /// signed value using the H.264 convention.
    pub fn read_se(&mut self) -> Result<i32, ParseError> {
        let k = self.read_ue()?;
        // k=0 -> 0; k=1 -> 1; k=2 -> -1; k=3 -> 2; k=4 -> -2 ...
        let signed = if (k & 1) == 0 {
            -((k as i64) / 2)
        } else {
            ((k as i64) + 1) / 2
        };
        Ok(signed as i32)
    }

    // ---- Emulation-prevention removal -----------------------------------

    /// Strip H.264/H.265 emulation-prevention bytes from a NAL unit payload.
    /// Wherever the byte sequence `00 00 03` appears, the `03` is removed.
    /// Returns a new owned `Vec<u8>` (the RBSP).
    pub fn rbsp_from_nal_unit(nal: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(nal.len());
        let mut i = 0;
        while i < nal.len() {
            if i + 2 < nal.len() && nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 0x03 {
                out.push(0);
                out.push(0);
                i += 3; // skip the 0x03
            } else {
                out.push(nal[i]);
                i += 1;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bit_sequences_match_bits() {
        let bytes = [0b1010_1100, 0b1111_0000];
        let mut br = BitReader::new(&bytes);
        assert!(br.read_bit().unwrap());
        assert!(!br.read_bit().unwrap());
        assert!(br.read_bit().unwrap());
        assert!(!br.read_bit().unwrap());
        // remaining of byte 0 = 1100
        assert_eq!(br.read_bits(4).unwrap(), 0b1100);
        // byte 1 = 11110000
        assert_eq!(br.read_bits(8).unwrap(), 0xF0);
    }

    #[test]
    fn read_bits_across_byte_boundaries() {
        let bytes = [0xAB, 0xCD]; // 1010 1011 1100 1101
        let mut br = BitReader::new(&bytes);
        // top 4 bits = 0xA
        assert_eq!(br.read_bits(4).unwrap(), 0xA);
        // next 8 bits = 1011 1100 = 0xBC
        assert_eq!(br.read_bits(8).unwrap(), 0xBC);
        // remaining 4 bits = 0xD
        assert_eq!(br.read_bits(4).unwrap(), 0xD);
        assert!(br.eos());
    }

    #[test]
    fn read_bits_with_n_zero_is_zero() {
        let bytes = [0xFF];
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_bits(0).unwrap(), 0);
        assert_eq!(br.position_bits(), 0);
    }

    #[test]
    fn read_bits_eos_returns_unexpected_eof() {
        let bytes = [0xFF];
        let mut br = BitReader::new(&bytes);
        let _ = br.read_bits(8).unwrap();
        let err = br.read_bits(1).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn skip_bits_advances_then_eof_when_overrun() {
        let bytes = [0xFF, 0xFF];
        let mut br = BitReader::new(&bytes);
        br.skip_bits(12).unwrap();
        assert_eq!(br.position_bits(), 12);
        let err = br.skip_bits(8).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn align_to_byte_rounds_up_unless_already_aligned() {
        let bytes = [0xFF, 0xFF];
        let mut br = BitReader::new(&bytes);
        br.skip_bits(3).unwrap();
        br.align_to_byte();
        assert_eq!(br.position_bits(), 8);
        br.align_to_byte(); // already aligned
        assert_eq!(br.position_bits(), 8);
    }

    #[test]
    fn ue_known_codewords() {
        // 1            -> 0
        // 010          -> 1
        // 011          -> 2
        // 00100        -> 3
        // 00101        -> 4
        // We pack: 1 010 011 00100 00101 = 1010 0110 0100 0010 1xxx
        // = 0xA6 0x42 0x80 (last byte's trailing bits are unread)
        let bytes = [0xA6, 0x42, 0x80];
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_ue().unwrap(), 0);
        assert_eq!(br.read_ue().unwrap(), 1);
        assert_eq!(br.read_ue().unwrap(), 2);
        assert_eq!(br.read_ue().unwrap(), 3);
        assert_eq!(br.read_ue().unwrap(), 4);
    }

    #[test]
    fn se_known_codewords() {
        // k=0 -> 0 ; k=1 -> 1 ; k=2 -> -1 ; k=3 -> 2 ; k=4 -> -2
        // Bit-pack k = 0, 1, 2, 3, 4 same as above
        let bytes = [0xA6, 0x42, 0x80];
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.read_se().unwrap(), 0);
        assert_eq!(br.read_se().unwrap(), 1);
        assert_eq!(br.read_se().unwrap(), -1);
        assert_eq!(br.read_se().unwrap(), 2);
        assert_eq!(br.read_se().unwrap(), -2);
    }

    #[test]
    fn ue_runaway_is_rejected() {
        // 33 leading zeros: 5 bytes of 0x00 + a stop bit further out.
        let bytes = [0u8; 6];
        let mut br = BitReader::new(&bytes);
        let err = br.read_ue().unwrap_err();
        match err {
            ParseError::Malformed { reason, .. } => assert!(reason.contains("leading zeros")),
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn read_bytes_aligned_requires_alignment() {
        let bytes = [1, 2, 3, 4];
        let mut br = BitReader::new(&bytes);
        let out = br.read_bytes_aligned(2).unwrap();
        assert_eq!(out, &[1, 2]);
        assert_eq!(br.position_bytes(), 2);

        // misalign and retry
        br.skip_bits(3).unwrap();
        let err = br.read_bytes_aligned(1).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn read_bytes_aligned_eof_is_unexpected() {
        let bytes = [1, 2, 3];
        let mut br = BitReader::new(&bytes);
        let err = br.read_bytes_aligned(8).unwrap_err();
        assert!(matches!(err, ParseError::UnexpectedEof { .. }));
    }

    #[test]
    fn rbsp_strips_emulation_prevention_byte() {
        // input: 00 00 03 01 -> 00 00 01
        let stripped = BitReader::rbsp_from_nal_unit(&[0x00, 0x00, 0x03, 0x01]);
        assert_eq!(stripped, vec![0x00, 0x00, 0x01]);
        // No matching trigram -> unchanged
        let same = BitReader::rbsp_from_nal_unit(&[0x12, 0x34, 0x56]);
        assert_eq!(same, vec![0x12, 0x34, 0x56]);
        // Multiple occurrences
        let multi = BitReader::rbsp_from_nal_unit(&[0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0xFF]);
        assert_eq!(multi, vec![0x00, 0x00, 0x00, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn position_helpers_consistent() {
        let bytes = [0xFF; 4];
        let mut br = BitReader::new(&bytes);
        assert_eq!(br.total_bits(), 32);
        assert_eq!(br.remaining_bits(), 32);
        br.skip_bits(20).unwrap();
        assert_eq!(br.position_bits(), 20);
        assert_eq!(br.position_bytes(), 2);
        assert_eq!(br.remaining_bits(), 12);
        assert!(!br.eos());
    }

    #[test]
    fn from_rbsp_constructor_matches_new() {
        let bytes = [0x42];
        let a = BitReader::new(&bytes);
        let b = BitReader::from_rbsp(&bytes);
        assert_eq!(a.total_bits(), b.total_bits());
    }
}
