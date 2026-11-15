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

//! Buffered seekable input shared by every reader. Conceptually the same as
//! `mkvtoolnix/src/common/mm_io.{h,cpp}` but trimmed to the operations the
//! identification path needs.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::super::error::ParseError;
use super::endian;

const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;

/// Boxed read+seek backing so the same `FileSource` type works for real
/// files (production) and `Cursor<Vec<u8>>` (tests). The trait alias
/// `ReadSeek` keeps the box's bounds in one place.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send + ?Sized> ReadSeek for T {}

pub struct FileSource {
  inner: BufReader<Box<dyn ReadSeek>>,
  /// Logical position from the start of the file. Tracked manually so we
  /// avoid the BufReader::stream_position syscall on every call.
  position: u64,
  /// Total length in bytes when known; `None` for streams that don't expose
  /// a length cheaply.
  length: Option<u64>,
}

impl std::fmt::Debug for FileSource {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("FileSource")
      .field("position", &self.position)
      .field("length", &self.length)
      .finish_non_exhaustive()
  }
}

impl FileSource {
  /// Open a real on-disk file. Used by `media_metadata::parse(path, opts)`.
  pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, ParseError> {
    let file = File::open(path.as_ref()).map_err(|e| ParseError::io_at(0, e))?;
    let len = file.metadata().map_err(|e| ParseError::io_at(0, e))?.len();
    let boxed: Box<dyn ReadSeek> = Box::new(file);
    Ok(Self {
      inner: BufReader::with_capacity(DEFAULT_BUFFER_SIZE, boxed),
      position: 0,
      length: Some(len),
    })
  }

  /// Construct over an in-memory byte buffer. Used in production to parse a
  /// decompressed payload (e.g. a zlib-inflated QuickTime `cmov` movie box)
  /// with the same box walkers used for on-disk data.
  pub fn from_memory(bytes: Vec<u8>) -> Self {
    Self::from_reader_for_test(std::io::Cursor::new(bytes))
  }

  /// Test-only constructor over an in-memory cursor. Used by unit tests and
  /// by the `Reader` trait's synthetic exercises.
  pub fn from_reader_for_test<R: Read + Seek + Send + 'static>(reader: R) -> Self {
    let boxed: Box<dyn ReadSeek> = Box::new(reader);
    let mut src = Self {
      inner: BufReader::with_capacity(DEFAULT_BUFFER_SIZE, boxed),
      position: 0,
      length: None,
    };
    // Derive length cheaply when the backing reader supports it; ignore
    // failure to keep this constructor infallible.
    if let Ok(len) = src.inner.seek(SeekFrom::End(0)) {
      src.length = Some(len);
    }
    let _ = src.inner.seek(SeekFrom::Start(0));
    src.position = 0;
    src
  }

  /// Current logical offset from byte 0.
  pub fn position(&self) -> u64 {
    self.position
  }

  /// File length in bytes, when known.
  pub fn length(&self) -> Option<u64> {
    self.length
  }

  /// Number of bytes remaining ahead of the cursor, when length is known.
  pub fn remaining(&self) -> Option<u64> {
    self.length.map(|l| l.saturating_sub(self.position))
  }

  /// Seek to an absolute offset.
  pub fn seek_to(&mut self, offset: u64) -> Result<(), ParseError> {
    self
      .inner
      .seek(SeekFrom::Start(offset))
      .map_err(|e| ParseError::io_at(self.position, e))?;
    self.position = offset;
    Ok(())
  }

  /// Advance the cursor by `n` bytes without reading the data into a
  /// user buffer. Cheap for backed-by-File sources via seek; for in-memory
  /// cursors it is also a seek.
  pub fn skip(&mut self, n: u64) -> Result<(), ParseError> {
    let target = self.position.checked_add(n).ok_or_else(|| ParseError::Malformed {
      format: "io",
      offset: self.position,
      reason: format!("skip overflow: position={} n={}", self.position, n),
    })?;
    self.seek_to(target)
  }

  /// Fill `buf` exactly. Returns `UnexpectedEof` if the file ended before
  /// the buffer was full.
  pub fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ParseError> {
    let start = self.position;
    match self.inner.read_exact(buf) {
      Ok(()) => {
        self.position += buf.len() as u64;
        Ok(())
      }
      Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(ParseError::UnexpectedEof {
        offset: start,
        wanted: buf.len() as u64,
      }),
      Err(e) => Err(ParseError::io_at(start, e)),
    }
  }

  /// Best-effort read of up to `buf.len()` bytes. Returns the number of
  /// bytes written; zero indicates EOF. Useful for probe-style reads.
  pub fn read_at_most(&mut self, buf: &mut [u8]) -> Result<usize, ParseError> {
    let start = self.position;
    let mut total = 0usize;
    while total < buf.len() {
      match self.inner.read(&mut buf[total..]) {
        Ok(0) => break,
        Ok(n) => total += n,
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
        Err(e) => return Err(ParseError::io_at(start, e)),
      }
    }
    self.position += total as u64;
    Ok(total)
  }

  /// Read a single byte. Returns `UnexpectedEof` at end of file.
  pub fn read_u8(&mut self) -> Result<u8, ParseError> {
    let mut b = [0u8; 1];
    self.read_exact(&mut b)?;
    Ok(b[0])
  }

  /// Read `N` bytes into a fixed-size array. Convenience over read_exact
  /// for callers that want a `[u8; N]` to pass into the endian helpers.
  pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
    let mut a = [0u8; N];
    self.read_exact(&mut a)?;
    Ok(a)
  }

  /// Allocate and fill a `Vec<u8>` of length `n`. Caps allocation against
  /// `cap` to avoid runaway allocations from malicious / corrupt headers.
  pub fn read_vec_capped(&mut self, n: u64, cap: u64) -> Result<Vec<u8>, ParseError> {
    if n > cap {
      return Err(ParseError::OversizedElement {
        format: "io",
        id: 0,
        size: n,
        cap,
        offset: self.position,
      });
    }
    let mut v = vec![0u8; n as usize];
    self.read_exact(&mut v)?;
    Ok(v)
  }

  // --- multi-byte readers; thin wrappers over `endian::*` -----------------

  pub fn read_u16_be(&mut self) -> Result<u16, ParseError> {
    let a = self.read_array::<2>()?;
    Ok(endian::get_u16_be(&a))
  }
  pub fn read_u16_le(&mut self) -> Result<u16, ParseError> {
    let a = self.read_array::<2>()?;
    Ok(endian::get_u16_le(&a))
  }
  pub fn read_u24_be(&mut self) -> Result<u32, ParseError> {
    let a = self.read_array::<3>()?;
    Ok(endian::get_u24_be(&a))
  }
  pub fn read_u32_be(&mut self) -> Result<u32, ParseError> {
    let a = self.read_array::<4>()?;
    Ok(endian::get_u32_be(&a))
  }
  pub fn read_u32_le(&mut self) -> Result<u32, ParseError> {
    let a = self.read_array::<4>()?;
    Ok(endian::get_u32_le(&a))
  }
  pub fn read_u64_be(&mut self) -> Result<u64, ParseError> {
    let a = self.read_array::<8>()?;
    Ok(endian::get_u64_be(&a))
  }
  pub fn read_u64_le(&mut self) -> Result<u64, ParseError> {
    let a = self.read_array::<8>()?;
    Ok(endian::get_u64_le(&a))
  }

  /// Peek the next `N` bytes without advancing the cursor. Implemented as
  /// `read_exact` + `seek_to(start)` — cost is one buffered seek.
  pub fn peek_array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
    let start = self.position;
    let a = self.read_array::<N>()?;
    self.seek_to(start)?;
    Ok(a)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn src(bytes: &[u8]) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes.to_vec()))
  }

  #[test]
  fn empty_input_has_zero_length_and_zero_position() {
    let s = src(&[]);
    assert_eq!(s.length(), Some(0));
    assert_eq!(s.position(), 0);
    assert_eq!(s.remaining(), Some(0));
  }

  #[test]
  fn read_exact_advances_position() {
    let mut s = src(&[1, 2, 3, 4]);
    let mut buf = [0u8; 2];
    s.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [1, 2]);
    assert_eq!(s.position(), 2);
    assert_eq!(s.remaining(), Some(2));
  }

  #[test]
  fn read_exact_eof_returns_unexpected_eof() {
    let mut s = src(&[1, 2]);
    let mut buf = [0u8; 4];
    let err = s.read_exact(&mut buf).unwrap_err();
    match err {
      ParseError::UnexpectedEof { offset, wanted } => {
        assert_eq!(offset, 0);
        assert_eq!(wanted, 4);
      }
      other => panic!("expected UnexpectedEof, got {other:?}"),
    }
  }

  #[test]
  fn read_at_most_handles_short_read() {
    let mut s = src(&[1, 2, 3]);
    let mut buf = [0u8; 8];
    let n = s.read_at_most(&mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
    assert_eq!(s.position(), 3);
    // second call returns 0 at EOF
    assert_eq!(s.read_at_most(&mut buf).unwrap(), 0);
  }

  #[test]
  fn read_u8_at_eof_is_unexpected_eof() {
    let mut s = src(&[]);
    let err = s.read_u8().unwrap_err();
    assert!(matches!(err, ParseError::UnexpectedEof { .. }));
  }

  #[test]
  fn read_u8_returns_byte_and_advances() {
    let mut s = src(&[0x42, 0x43]);
    assert_eq!(s.read_u8().unwrap(), 0x42);
    assert_eq!(s.read_u8().unwrap(), 0x43);
    assert_eq!(s.position(), 2);
  }

  #[test]
  fn read_array_is_typed_and_endian_helpers_chain() {
    let mut s = src(&[0x12, 0x34, 0xAB, 0xCD]);
    let arr: [u8; 2] = s.read_array().unwrap();
    assert_eq!(arr, [0x12, 0x34]);
    // following endian read picks up at position 2
    assert_eq!(s.read_u16_be().unwrap(), 0xABCD);
  }

  #[test]
  fn seek_to_changes_position() {
    let mut s = src(&[1, 2, 3, 4]);
    s.seek_to(3).unwrap();
    assert_eq!(s.position(), 3);
    assert_eq!(s.read_u8().unwrap(), 4);
  }

  #[test]
  fn skip_advances_position_without_reading() {
    let mut s = src(&[1, 2, 3, 4]);
    s.skip(2).unwrap();
    assert_eq!(s.position(), 2);
    assert_eq!(s.read_u8().unwrap(), 3);
  }

  #[test]
  fn skip_overflow_returns_malformed() {
    let mut s = src(&[1, 2]);
    s.read_u8().unwrap(); // advance past byte 0 so 1 + u64::MAX wraps
    let err = s.skip(u64::MAX).unwrap_err();
    match err {
      ParseError::Malformed { format, .. } => assert_eq!(format, "io"),
      other => panic!("expected Malformed, got {other:?}"),
    }
  }

  #[test]
  fn read_vec_capped_rejects_oversize() {
    let mut s = src(&[1, 2, 3, 4, 5]);
    let err = s.read_vec_capped(1024, 16).unwrap_err();
    match err {
      ParseError::OversizedElement { format, size, cap, .. } => {
        assert_eq!(format, "io");
        assert_eq!(size, 1024);
        assert_eq!(cap, 16);
      }
      other => panic!("expected OversizedElement, got {other:?}"),
    }
    // cursor not advanced on rejection
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn read_vec_capped_succeeds_under_cap() {
    let mut s = src(&[1, 2, 3, 4, 5]);
    let v = s.read_vec_capped(3, 16).unwrap();
    assert_eq!(v, vec![1, 2, 3]);
    assert_eq!(s.position(), 3);
  }

  #[test]
  fn endian_wrapper_reads_match_helpers() {
    let mut s = src(&[
      0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, // u64
    ]);
    assert_eq!(s.read_u64_be().unwrap(), 0x1234_5678_9ABC_DEF0);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u32_be().unwrap(), 0x1234_5678);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u24_be().unwrap(), 0x0012_3456);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u16_be().unwrap(), 0x1234);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u16_le().unwrap(), 0x3412);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u32_le().unwrap(), 0x7856_3412);
    s.seek_to(0).unwrap();
    assert_eq!(s.read_u64_le().unwrap(), 0xF0DE_BC9A_7856_3412);
  }

  #[test]
  fn peek_array_does_not_advance() {
    let mut s = src(&[1, 2, 3, 4]);
    let peeked: [u8; 3] = s.peek_array().unwrap();
    assert_eq!(peeked, [1, 2, 3]);
    assert_eq!(s.position(), 0);
    // subsequent read sees the same bytes
    let read: [u8; 3] = s.read_array().unwrap();
    assert_eq!(read, [1, 2, 3]);
  }

  #[test]
  fn open_nonexistent_returns_io_error_at_offset_zero() {
    let err = FileSource::open("definitely-does-not-exist-12345.mkv").unwrap_err();
    match err {
      ParseError::Io { offset, source } => {
        assert_eq!(offset, 0);
        assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
      }
      other => panic!("expected Io, got {other:?}"),
    }
  }
}
