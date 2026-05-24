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

//! Top-level `CoreAudioReader`.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::caf::{self, CAFF_MAGIC};

const PROBE_BYTES: usize = 64 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct CoreAudioReader;

impl Reader for CoreAudioReader {
    fn name(&self) -> &'static str {
        "coreaudio"
    }

    fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
        let mut head = [0u8; 4];
        let read = src.read_at_most(&mut head)?;
        src.seek_to(0)?;
        Ok(read == 4 && head == CAFF_MAGIC)
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        _deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut buf = vec![0u8; PROBE_BYTES];
        src.seek_to(0)?;
        let read = src.read_at_most(&mut buf)?;
        let metadata = caf::parse(&buf[..read])?;

        out.container.format = ContainerFormat::CoreAudio;
        out.container.recognized = true;
        out.container.supported = true;
        let description = metadata.description.ok_or(ParseError::Malformed {
            format: "coreaudio",
            offset: 0,
            reason: "missing desc chunk".to_string(),
        })?;
        let mut common = CommonTrackProperties::default();
        common.number = Some(1);
        let mut audio = AudioTrackProperties::default();
        if description.sample_rate > 0.0 {
            audio.sampling_frequency = Some(description.sample_rate);
        }
        if description.channels > 0 {
            audio.channels = Some(description.channels);
        }
        if description.bits_per_channel > 0 {
            audio.bit_depth = Some(description.bits_per_channel);
        }
        let codec_id = format!("CAF/{}", caf::fourcc_string(&description.format_id));
        let codec_name = codec_name_for(&description.format_id);
        out.tracks.push(Track {
            id: 0,
            track_type: TrackType::Audio,
            codec: CodecInfo {
                id: codec_id,
                name: Some(codec_name.to_string()),
                codec_private: None,
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

fn codec_name_for(format_id: &[u8; 4]) -> &'static str {
    match format_id {
        b"lpcm" => "PCM",
        b"alac" => "ALAC (Apple Lossless)",
        b"aac " => "AAC",
        b"ulaw" => "G.711 \u{00B5}-law",
        b"alaw" => "G.711 A-law",
        b"MAC3" => "MACE 3:1",
        b"MAC6" => "MACE 6:1",
        b"ima4" => "IMA ADPCM",
        b".mp1" => "MPEG-1 Layer I",
        b".mp2" => "MPEG-1 Layer II",
        b".mp3" => "MP3",
        b"ac-3" => "AC-3",
        _ => "CoreAudio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::coreaudio::caf::build_caf;
    use std::io::Cursor;

    #[test]
    fn probe_accepts_caff_magic() {
        let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        assert!(CoreAudioReader.probe(&mut s).unwrap());
    }

    #[test]
    fn probe_rejects_other_magic() {
        let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
        assert!(!CoreAudioReader.probe(&mut s).unwrap());
    }

    #[test]
    fn read_headers_extracts_lpcm_track() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_caf(b"lpcm", 48_000.0, 2, 24);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.caf", 0);
        CoreAudioReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.container.format, ContainerFormat::CoreAudio);
        let a = out.tracks[0].properties.audio.as_ref().unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.bit_depth, Some(24));
        assert_eq!(a.sampling_frequency, Some(48_000.0));
        assert_eq!(out.tracks[0].codec.id, "CAF/lpcm");
        assert_eq!(out.tracks[0].codec.name.as_deref(), Some("PCM"));
    }

    #[test]
    fn read_headers_recognises_alac() {
        use crate::media_metadata::deadline::Deadline;
        let bytes = build_caf(b"alac", 44_100.0, 2, 16);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.caf", 0);
        CoreAudioReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap();
        assert_eq!(out.tracks[0].codec.name.as_deref(), Some("ALAC (Apple Lossless)"));
    }

    #[test]
    fn codec_name_for_table_covers_common_formats() {
        assert_eq!(codec_name_for(b"lpcm"), "PCM");
        assert_eq!(codec_name_for(b"aac "), "AAC");
        assert_eq!(codec_name_for(b".mp3"), "MP3");
        assert_eq!(codec_name_for(b"ac-3"), "AC-3");
        assert_eq!(codec_name_for(b"XXXX"), "CoreAudio");
    }

    #[test]
    fn read_headers_returns_malformed_without_desc_chunk() {
        use crate::media_metadata::deadline::Deadline;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"caff");
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
        let mut out = MediaMetadata::new("clip.caf", 0);
        let err = CoreAudioReader.read_headers(&mut s, &Deadline::new(60_000), &mut out).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }
}
