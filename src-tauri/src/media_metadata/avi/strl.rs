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

//! Per-stream `strl` walker (one LIST per track inside `hdrl`).
//!
//! `strl` contains:
//!
//! - `strh` (stream header, ~56 bytes) — fcc_type (`vids`, `auds`, `txts`),
//!   fcc_handler (codec FOURCC), flags, scale/rate (timebase), length, etc.
//! - `strf` (stream format) — variable size, format depends on fcc_type:
//!   - `vids` → BITMAPINFOHEADER (40 bytes minimum).
//!   - `auds` → WAVEFORMATEX (18 bytes) or WAVEFORMATEXTENSIBLE (40 bytes).
//! - `strn` (stream name) — optional NUL-terminated ASCII / UTF-8.
//! - `strd` (private data) — optional codec-private blob.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use super::riff::{self, ChildAction, ChunkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AviStreamKind {
    Video,
    Audio,
    Text,
    Midi,
    Unknown,
}

impl AviStreamKind {
    pub fn from_fcc(fcc: &[u8; 4]) -> Self {
        match fcc {
            b"vids" => Self::Video,
            b"auds" => Self::Audio,
            b"txts" => Self::Text,
            b"mids" => Self::Midi,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamHeader {
    pub kind: AviStreamKind,
    /// Codec FOURCC (video) or 0 (audio).  For audio, the actual codec lives
    /// in WAVEFORMATEX::wFormatTag.
    pub fcc_handler: [u8; 4],
    pub flags: u32,
    pub priority: u16,
    pub language: u16,
    pub scale: u32,
    pub rate: u32,
    pub start: u32,
    pub length: u32,
    pub sample_size: u32,
}

impl StreamHeader {
    /// Frame rate in Hz when both scale and rate are non-zero.
    pub fn frame_rate(&self) -> Option<f64> {
        if self.scale == 0 || self.rate == 0 {
            None
        } else {
            Some(self.rate as f64 / self.scale as f64)
        }
    }

    /// Frame duration in nanoseconds when the timebase is valid.
    pub fn frame_duration_ns(&self) -> Option<u64> {
        if self.scale == 0 || self.rate == 0 {
            None
        } else {
            Some(((self.scale as u128) * 1_000_000_000 / self.rate as u128) as u64)
        }
    }
}

/// Parse the `strh` chunk payload.  We read the first 56 bytes; modern AVI
/// occasionally appends extra fields we don't need.
pub fn parse_strh(src: &mut FileSource, header: &ChunkHeader) -> Result<StreamHeader, ParseError> {
    if header.size < 48 {
        return Err(ParseError::Malformed {
            format: "avi",
            offset: header.start,
            reason: format!("strh payload {} bytes too small", header.size),
        });
    }
    let fcc_type = src.read_array::<4>()?;
    let fcc_handler = src.read_array::<4>()?;
    let flags = src.read_u32_le()?;
    let priority = src.read_u16_le()?;
    let language = src.read_u16_le()?;
    let _initial_frames = src.read_u32_le()?;
    let scale = src.read_u32_le()?;
    let rate = src.read_u32_le()?;
    let start = src.read_u32_le()?;
    let length = src.read_u32_le()?;
    let _suggested_buffer_size = src.read_u32_le()?;
    let _quality = src.read_u32_le()?;
    let sample_size = src.read_u32_le()?;
    // Remaining fields (rcFrame: top, left, right, bottom — 4×u16) are
    // omitted; we have what we need.
    Ok(StreamHeader {
        kind: AviStreamKind::from_fcc(&fcc_type),
        fcc_handler,
        flags,
        priority,
        language,
        scale,
        rate,
        start,
        length,
        sample_size,
    })
}

#[derive(Debug, Clone)]
pub enum StreamFormat {
    Video(BitmapInfoHeader),
    Audio(WaveFormatEx),
    /// Raw bytes for text / unknown formats.
    Other(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
pub struct BitmapInfoHeader {
    pub size: u32,
    pub width: i32,
    pub height: i32,
    pub planes: u16,
    pub bit_count: u16,
    pub compression: [u8; 4],
    pub image_size: u32,
}

impl BitmapInfoHeader {
    /// `true` when the FOURCC corresponds to uncompressed RGB.
    pub fn is_uncompressed(&self) -> bool {
        self.compression == *b"DIB " || self.compression == [0u8; 4]
    }
}

#[derive(Debug, Clone)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// Extra format-specific bytes (codec config).
    pub extra: Vec<u8>,
}

const MAX_STRF_BYTES: u64 = 64 * 1024;

pub fn parse_strf(
    src: &mut FileSource,
    header: &ChunkHeader,
    stream: &StreamHeader,
) -> Result<StreamFormat, ParseError> {
    let bytes = riff::read_payload(src, header, MAX_STRF_BYTES)?;
    match stream.kind {
        AviStreamKind::Video => parse_bitmapinfoheader(&bytes, header.start),
        AviStreamKind::Audio => parse_waveformatex(&bytes, header.start),
        _ => Ok(StreamFormat::Other(bytes)),
    }
}

fn parse_bitmapinfoheader(bytes: &[u8], offset: u64) -> Result<StreamFormat, ParseError> {
    if bytes.len() < 40 {
        return Err(ParseError::Malformed {
            format: "avi",
            offset,
            reason: format!(
                "BITMAPINFOHEADER payload {} bytes too small",
                bytes.len()
            ),
        });
    }
    let size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let width = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let height = i32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let planes = u16::from_le_bytes([bytes[12], bytes[13]]);
    let bit_count = u16::from_le_bytes([bytes[14], bytes[15]]);
    let compression = [bytes[16], bytes[17], bytes[18], bytes[19]];
    let image_size = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok(StreamFormat::Video(BitmapInfoHeader {
        size,
        width,
        height,
        planes,
        bit_count,
        compression,
        image_size,
    }))
}

fn parse_waveformatex(bytes: &[u8], offset: u64) -> Result<StreamFormat, ParseError> {
    if bytes.len() < 14 {
        return Err(ParseError::Malformed {
            format: "avi",
            offset,
            reason: format!("WAVEFORMATEX payload {} bytes too small", bytes.len()),
        });
    }
    let format_tag = u16::from_le_bytes([bytes[0], bytes[1]]);
    let channels = u16::from_le_bytes([bytes[2], bytes[3]]);
    let samples_per_sec = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let avg_bytes_per_sec =
        u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let block_align = u16::from_le_bytes([bytes[12], bytes[13]]);
    let (bits_per_sample, extra_offset) = if bytes.len() >= 16 {
        (u16::from_le_bytes([bytes[14], bytes[15]]), 16usize)
    } else {
        (0, bytes.len())
    };
    let extra = if bytes.len() > extra_offset + 2 {
        let cb_size =
            u16::from_le_bytes([bytes[extra_offset], bytes[extra_offset + 1]]) as usize;
        let start = extra_offset + 2;
        let end = (start + cb_size).min(bytes.len());
        bytes[start..end].to_vec()
    } else {
        Vec::new()
    };
    Ok(StreamFormat::Audio(WaveFormatEx {
        format_tag,
        channels,
        samples_per_sec,
        avg_bytes_per_sec,
        block_align,
        bits_per_sample,
        extra,
    }))
}

#[derive(Debug, Default, Clone)]
pub struct StreamBuilder {
    pub header: Option<StreamHeader>,
    pub format: Option<StreamFormat>,
    pub name: Option<String>,
    pub private: Option<Vec<u8>>,
}

/// Walk one `strl` LIST.  Returns the populated builder.
pub fn parse_strl(
    src: &mut FileSource,
    parent: &ChunkHeader,
    deadline: &Deadline,
) -> Result<StreamBuilder, ParseError> {
    let mut builder = StreamBuilder::default();
    let mut deferred_strf_offset: Option<u64> = None;
    let mut deferred_strf_size: Option<u32> = None;

    riff::walk_list_children(
        src,
        parent,
        "avi::strl",
        deadline,
        |src, child| match &child.kind {
            b"strh" => {
                builder.header = Some(parse_strh(src, child)?);
                Ok(ChildAction::Consumed)
            }
            b"strf" => {
                // Defer until we have the header (strh comes first by convention,
                // so this is just defensive).
                if let Some(h) = builder.header.clone() {
                    builder.format = Some(parse_strf(src, child, &h)?);
                    Ok(ChildAction::Consumed)
                } else {
                    deferred_strf_offset = Some(child.payload_start());
                    deferred_strf_size = Some(child.size);
                    Ok(ChildAction::Skip)
                }
            }
            b"strn" => {
                let bytes = riff::read_payload(src, child, 4 * 1024)?;
                let trimmed: Vec<u8> = bytes
                    .into_iter()
                    .take_while(|b| *b != 0)
                    .collect();
                builder.name = Some(String::from_utf8_lossy(&trimmed).into_owned());
                Ok(ChildAction::Consumed)
            }
            b"strd" => {
                builder.private = Some(riff::read_payload(src, child, 64 * 1024)?);
                Ok(ChildAction::Consumed)
            }
            _ => Ok(ChildAction::Skip),
        },
    )?;

    // Late strf rescue: if strh appeared after strf we re-read.
    if builder.format.is_none() {
        if let (Some(off), Some(size), Some(h)) =
            (deferred_strf_offset, deferred_strf_size, builder.header.clone())
        {
            src.seek_to(off)?;
            let synthetic = ChunkHeader {
                start: off - 8,
                kind: *b"strf",
                size,
            };
            builder.format = Some(parse_strf(src, &synthetic, &h)?);
        }
    }
    Ok(builder)
}

#[cfg(test)]
pub(crate) fn build_strh_payload(
    fcc_type: &[u8; 4],
    fcc_handler: &[u8; 4],
    scale: u32,
    rate: u32,
    length: u32,
    sample_size: u32,
) -> Vec<u8> {
    let mut p = Vec::with_capacity(56);
    p.extend_from_slice(fcc_type);
    p.extend_from_slice(fcc_handler);
    p.extend_from_slice(&0u32.to_le_bytes()); // flags
    p.extend_from_slice(&0u16.to_le_bytes()); // priority
    p.extend_from_slice(&0u16.to_le_bytes()); // language
    p.extend_from_slice(&0u32.to_le_bytes()); // initial_frames
    p.extend_from_slice(&scale.to_le_bytes());
    p.extend_from_slice(&rate.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // start
    p.extend_from_slice(&length.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // suggested buffer
    p.extend_from_slice(&0u32.to_le_bytes()); // quality
    p.extend_from_slice(&sample_size.to_le_bytes());
    p.extend_from_slice(&[0u8; 8]); // rcFrame
    p
}

#[cfg(test)]
pub(crate) fn build_bitmapinfoheader(
    width: i32,
    height: i32,
    bit_count: u16,
    compression: &[u8; 4],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(40);
    p.extend_from_slice(&40u32.to_le_bytes()); // size
    p.extend_from_slice(&width.to_le_bytes());
    p.extend_from_slice(&height.to_le_bytes());
    p.extend_from_slice(&1u16.to_le_bytes()); // planes
    p.extend_from_slice(&bit_count.to_le_bytes());
    p.extend_from_slice(compression);
    p.extend_from_slice(&0u32.to_le_bytes()); // image_size
    p.extend_from_slice(&[0u8; 16]); // remaining BMIH fields
    p
}

#[cfg(test)]
pub(crate) fn build_waveformatex(
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    extra: &[u8],
) -> Vec<u8> {
    let mut p = Vec::with_capacity(18 + extra.len());
    p.extend_from_slice(&format_tag.to_le_bytes());
    p.extend_from_slice(&channels.to_le_bytes());
    p.extend_from_slice(&samples_per_sec.to_le_bytes());
    p.extend_from_slice(&avg_bytes_per_sec.to_le_bytes());
    p.extend_from_slice(&block_align.to_le_bytes());
    p.extend_from_slice(&bits_per_sample.to_le_bytes());
    p.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    p.extend_from_slice(extra);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::avi::riff::{self, encode_chunk, encode_list};
    use crate::media_metadata::deadline::Deadline;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn read_strh(payload: Vec<u8>) -> StreamHeader {
        let bytes = encode_chunk(b"strh", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = riff::read_chunk_header(&mut s).unwrap();
        parse_strh(&mut s, &h).unwrap()
    }

    #[test]
    fn video_strh_classified_as_video() {
        let p = build_strh_payload(b"vids", b"H264", 1001, 24000, 240, 0);
        let h = read_strh(p);
        assert_eq!(h.kind, AviStreamKind::Video);
        assert_eq!(&h.fcc_handler, b"H264");
        assert_eq!(h.scale, 1001);
        assert_eq!(h.rate, 24000);
        assert_eq!(h.length, 240);
        let fps = h.frame_rate().unwrap();
        assert!((fps - 23.976).abs() < 0.01);
        assert_eq!(h.frame_duration_ns(), Some(41_708_333));
    }

    #[test]
    fn audio_strh_classified_as_audio() {
        let p = build_strh_payload(b"auds", b"\0\0\0\0", 1, 48000, 0, 4);
        let h = read_strh(p);
        assert_eq!(h.kind, AviStreamKind::Audio);
        assert_eq!(h.sample_size, 4);
    }

    #[test]
    fn unknown_fcc_type_is_unknown_kind() {
        let p = build_strh_payload(b"abcd", b"H264", 1, 1, 0, 0);
        let h = read_strh(p);
        assert_eq!(h.kind, AviStreamKind::Unknown);
    }

    #[test]
    fn text_kind_recognised() {
        assert_eq!(AviStreamKind::from_fcc(b"txts"), AviStreamKind::Text);
    }

    #[test]
    fn midi_kind_recognised() {
        assert_eq!(AviStreamKind::from_fcc(b"mids"), AviStreamKind::Midi);
    }

    #[test]
    fn rejects_truncated_strh_payload() {
        let bytes = encode_chunk(b"strh", &[0u8; 16]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let h = riff::read_chunk_header(&mut s).unwrap();
        let err = parse_strh(&mut s, &h).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn frame_rate_handles_zero_timebase() {
        let p = build_strh_payload(b"vids", b"H264", 0, 0, 0, 0);
        let h = read_strh(p);
        assert!(h.frame_rate().is_none());
        assert!(h.frame_duration_ns().is_none());
    }

    fn parse_strl_payload(strl_payload: Vec<u8>) -> StreamBuilder {
        let bytes = encode_chunk(b"LIST", &{
            let mut p = b"strl".to_vec();
            p.extend(strl_payload);
            p
        });
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let parent = riff::read_chunk_header(&mut s).unwrap();
        parse_strl(&mut s, &parent, &dl()).unwrap()
    }

    #[test]
    fn strl_with_video_strh_and_strf() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1001, 24000, 240, 0),
        );
        let strf = encode_chunk(
            b"strf",
            &build_bitmapinfoheader(1920, 1080, 24, b"H264"),
        );
        let mut payload = strh;
        payload.extend(strf);
        let b = parse_strl_payload(payload);
        let h = b.header.unwrap();
        assert_eq!(h.kind, AviStreamKind::Video);
        let format = b.format.unwrap();
        match format {
            StreamFormat::Video(bmih) => {
                assert_eq!(bmih.width, 1920);
                assert_eq!(bmih.height, 1080);
                assert_eq!(&bmih.compression, b"H264");
            }
            _ => panic!("expected video format"),
        }
    }

    #[test]
    fn strl_with_audio_waveformatex() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"auds", b"\0\0\0\0", 1, 48000, 0, 4),
        );
        let strf = encode_chunk(
            b"strf",
            &build_waveformatex(0x00FF, 2, 48000, 16000, 4, 16, &[]),
        );
        let mut payload = strh;
        payload.extend(strf);
        let b = parse_strl_payload(payload);
        match b.format.unwrap() {
            StreamFormat::Audio(wf) => {
                assert_eq!(wf.format_tag, 0x00FF);
                assert_eq!(wf.channels, 2);
                assert_eq!(wf.samples_per_sec, 48000);
                assert_eq!(wf.bits_per_sample, 16);
                assert!(wf.extra.is_empty());
            }
            _ => panic!("expected audio format"),
        }
    }

    #[test]
    fn strl_extracts_strn_name() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1, 30, 0, 0),
        );
        let strn = encode_chunk(b"strn", b"Track Name\0");
        let mut payload = strh;
        payload.extend(strn);
        let b = parse_strl_payload(payload);
        assert_eq!(b.name.as_deref(), Some("Track Name"));
    }

    #[test]
    fn strl_extracts_strd_private_data() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1, 30, 0, 0),
        );
        let strd = encode_chunk(b"strd", &[1, 2, 3, 4]);
        let mut payload = strh;
        payload.extend(strd);
        let b = parse_strl_payload(payload);
        assert_eq!(b.private.as_deref(), Some(&[1, 2, 3, 4][..]));
    }

    #[test]
    fn waveformatex_with_extra_bytes_round_trips() {
        let extra_bytes = vec![0x11, 0x90, 0xAA, 0xBB];
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"auds", b"\0\0\0\0", 1, 48000, 0, 0),
        );
        let strf = encode_chunk(
            b"strf",
            &build_waveformatex(0x00FF, 2, 48000, 16000, 4, 16, &extra_bytes),
        );
        let mut payload = strh;
        payload.extend(strf);
        let b = parse_strl_payload(payload);
        match b.format.unwrap() {
            StreamFormat::Audio(wf) => assert_eq!(wf.extra, extra_bytes),
            _ => panic!("expected audio format"),
        }
    }

    #[test]
    fn strl_with_strf_before_strh_still_decodes() {
        let strf_payload = build_bitmapinfoheader(1280, 720, 24, b"H264");
        let strf = encode_chunk(b"strf", &strf_payload);
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1, 30, 0, 0),
        );
        // strf first
        let mut payload = strf;
        payload.extend(strh);
        let b = parse_strl_payload(payload);
        assert!(b.header.is_some());
        match b.format.unwrap() {
            StreamFormat::Video(bmih) => assert_eq!(bmih.width, 1280),
            _ => panic!("expected video format"),
        }
    }

    #[test]
    fn unknown_strl_child_is_skipped() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1, 30, 0, 0),
        );
        let bogus = encode_chunk(b"xxxx", &[1, 2, 3, 4]);
        let mut payload = strh;
        payload.extend(bogus);
        let b = parse_strl_payload(payload);
        assert!(b.header.is_some());
        assert!(b.private.is_none());
    }

    #[test]
    fn bitmapinfoheader_uncompressed_predicate() {
        let bmih = BitmapInfoHeader {
            size: 40,
            width: 1,
            height: 1,
            planes: 1,
            bit_count: 24,
            compression: [0; 4],
            image_size: 0,
        };
        assert!(bmih.is_uncompressed());

        let dib = BitmapInfoHeader { compression: *b"DIB ", ..bmih };
        assert!(dib.is_uncompressed());

        let h264 = BitmapInfoHeader { compression: *b"H264", ..bmih };
        assert!(!h264.is_uncompressed());
    }

    #[test]
    fn parse_strl_uses_walk_list_children() {
        // Just compile-time / shape check.
        let _ = parse_strl_payload(vec![]);
    }

    #[test]
    fn rejects_truncated_bmih_inside_strl() {
        let strh = encode_chunk(
            b"strh",
            &build_strh_payload(b"vids", b"H264", 1, 30, 0, 0),
        );
        let strf = encode_chunk(b"strf", &[0u8; 8]); // way too small for BMIH
        let mut payload = strh;
        payload.extend(strf);
        let strl_bytes = encode_list(b"LIST", b"strl", &[payload]);
        let mut s = FileSource::from_reader_for_test(Cursor::new(strl_bytes));
        let parent = riff::read_chunk_header(&mut s).unwrap();
        let err = parse_strl(&mut s, &parent, &dl()).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }
}
