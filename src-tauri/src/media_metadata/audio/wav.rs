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

//! WAV reader.  Supports both classic `RIFF/WAVE` and `RF64/WAVE` (for
//! files > 4 GB).  Walks chunks until `fmt ` + `data` are seen:
//!
//! ```text
//! RIFF | RF64
//!   WAVE
//!     ds64 (RF64 only) — extended sizes
//!     fmt  — WAVEFORMATEX or WAVEFORMATEXTENSIBLE
//!     data — payload
//! ```

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

const PROBE_BYTES: usize = 16 * 1024;
const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Debug, Clone)]
pub struct WaveFormat {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub extra: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WavMetadata {
    pub is_rf64: bool,
    pub format: WaveFormat,
    pub data_bytes: u64,
}

pub fn parse(bytes: &[u8]) -> Option<WavMetadata> {
    if bytes.len() < 12 {
        return None;
    }
    let is_rf64 = match &bytes[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    let mut format: Option<WaveFormat> = None;
    let mut data_bytes: Option<u64> = None;
    let mut data_size_override: Option<u64> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        let body_start = pos + 8;
        let body_end = body_start + size;
        if body_end > bytes.len() && id != b"data" {
            // Don't bail if data chunk extends past our probe — we only need
            // its size header, not the payload.
            break;
        }
        match id {
            b"ds64" if is_rf64 && size >= 16 => {
                // riff_size_low/high then data_size_low/high — we want data.
                let data = u64::from_le_bytes(bytes[body_start + 8..body_start + 16].try_into().ok()?);
                data_size_override = Some(data);
            }
            b"fmt " => {
                format = parse_fmt_chunk(&bytes[body_start..body_start.saturating_add(size).min(bytes.len())]);
            }
            b"data" => {
                let data_size = if size == 0xFFFF_FFFF {
                    data_size_override.unwrap_or(0)
                } else {
                    size as u64
                };
                data_bytes = Some(data_size);
                break;
            }
            _ => {}
        }
        // Pad chunk sizes to 2-byte boundary
        let advance = if size & 1 != 0 { size + 1 } else { size };
        pos = body_start.saturating_add(advance);
    }
    let format = format?;
    Some(WavMetadata {
        is_rf64,
        format,
        data_bytes: data_bytes.unwrap_or(0),
    })
}

fn parse_fmt_chunk(bytes: &[u8]) -> Option<WaveFormat> {
    if bytes.len() < 16 {
        return None;
    }
    let format_tag = u16::from_le_bytes([bytes[0], bytes[1]]);
    let channels = u16::from_le_bytes([bytes[2], bytes[3]]);
    let sample_rate = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let avg_bytes_per_sec = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let block_align = u16::from_le_bytes([bytes[12], bytes[13]]);
    let bits_per_sample = u16::from_le_bytes([bytes[14], bytes[15]]);
    let extra = if bytes.len() >= 18 {
        let cb = u16::from_le_bytes([bytes[16], bytes[17]]) as usize;
        if 18 + cb <= bytes.len() {
            bytes[18..18 + cb].to_vec()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    Some(WaveFormat {
        format_tag,
        channels,
        sample_rate,
        avg_bytes_per_sec,
        block_align,
        bits_per_sample,
        extra,
    })
}

fn codec_name(format_tag: u16) -> &'static str {
    match format_tag {
        WAVE_FORMAT_PCM => "PCM",
        WAVE_FORMAT_IEEE_FLOAT => "IEEE Float",
        WAVE_FORMAT_EXTENSIBLE => "WAVEFORMATEXTENSIBLE",
        0x0055 => "MP3",
        0x00FF => "AAC",
        _ => "Unknown",
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WavReader;

impl Reader for WavReader {
    fn name(&self) -> &'static str {
        "wav"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 12];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        if read < 12 {
            return Ok(false);
        }
        let prefix = &head[0..4];
        Ok((prefix == b"RIFF" || prefix == b"RF64") && &head[8..12] == b"WAVE")
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut probe = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut probe)?;
        let metadata = parse(&probe[..read]).ok_or(ParseError::Unrecognised)?;

        out.container.format = ContainerFormat::Wav;
        out.container.recognized = true;
        out.container.supported = true;
        if metadata.format.sample_rate > 0 && metadata.format.block_align > 0 {
            let samples = metadata.data_bytes / metadata.format.block_align as u64;
            let ns = (samples as u128) * 1_000_000_000 / metadata.format.sample_rate as u128;
            out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
        }

        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let audio = AudioTrackProperties {
            channels: if metadata.format.channels == 0 {
                None
            } else {
                Some(metadata.format.channels as u32)
            },
            sampling_frequency: if metadata.format.sample_rate == 0 {
                None
            } else {
                Some(metadata.format.sample_rate as f64)
            },
            bit_depth: if metadata.format.bits_per_sample == 0 {
                None
            } else {
                Some(metadata.format.bits_per_sample as u32)
            },
            ..AudioTrackProperties::default()
        };
        let codec_private = if metadata.format.extra.is_empty() {
            None
        } else {
            Some(CodecPrivate::from_bytes(&metadata.format.extra))
        };
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: format!("0x{:04X}", metadata.format.format_tag),
                name: Some(codec_name(metadata.format.format_tag).to_string()),
                codec_private,
            },
            properties: TrackProperties {
                common,
                audio: Some(audio),
                ..TrackProperties::default()
            },
        });
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn build_wav(
    sample_rate: u32,
    channels: u16,
    bits: u16,
    data_bytes: u32,
) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&sample_rate.to_le_bytes());
    fmt.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());

    let mut payload = Vec::new();
    payload.extend_from_slice(b"WAVE");
    payload.extend_from_slice(b"fmt ");
    payload.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    payload.extend_from_slice(&fmt);
    payload.extend_from_slice(b"data");
    payload.extend_from_slice(&data_bytes.to_le_bytes());
    let mut bytes = Vec::with_capacity(8 + payload.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_riff_wave_pcm() {
        let bytes = build_wav(48_000, 2, 24, 96_000);
        let m = parse(&bytes).unwrap();
        assert!(!m.is_rf64);
        assert_eq!(m.format.sample_rate, 48_000);
        assert_eq!(m.format.channels, 2);
        assert_eq!(m.format.bits_per_sample, 24);
        assert_eq!(m.data_bytes, 96_000);
    }

    #[test]
    fn parses_rf64_format_marker() {
        let mut bytes = build_wav(48_000, 2, 16, 12);
        bytes[0..4].copy_from_slice(b"RF64");
        let m = parse(&bytes).unwrap();
        assert!(m.is_rf64);
    }

    #[test]
    fn rejects_non_wave_payload() {
        let mut bytes = build_wav(48_000, 2, 16, 12);
        bytes[8..12].copy_from_slice(b"AVI ");
        assert!(parse(&bytes).is_none());
    }

    #[test]
    fn probe_accepts_riff_wave() {
        let bytes = build_wav(48_000, 2, 16, 4);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(WavReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_populates_audio_track_and_duration() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_wav(48_000, 2, 16, 192_000); // 1 second @ 48 kHz stereo 16-bit
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.wav", 0);
        WavReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.bit_depth, Some(16));
        assert_eq!(out.container.properties.duration.unwrap().ns, 1_000_000_000);
    }

    #[test]
    fn codec_name_table_covers_common_tags() {
        assert_eq!(codec_name(0x0001), "PCM");
        assert_eq!(codec_name(0x0003), "IEEE Float");
        assert_eq!(codec_name(0xFFFE), "WAVEFORMATEXTENSIBLE");
        assert_eq!(codec_name(0x0055), "MP3");
        assert_eq!(codec_name(0x00FF), "AAC");
        assert_eq!(codec_name(0xCAFE), "Unknown");
    }
}
