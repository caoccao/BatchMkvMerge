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

//! Audio TrackEntry sub-tree.  Port of
//! `r_matroska.cpp::read_headers_track_audio` (lines 1258-1266).
//!
//! Fields covered:
//! - SamplingFrequency (default 8000 Hz per spec).
//! - OutputSamplingFrequency.
//! - Channels (default 1).
//! - BitDepth.
//! - Emphasis (Matroska element 0x52F1).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::{
    AudioEmphasis, AudioTrackProperties,
};

use crate::media_metadata::matroska::ebml::{self, ChildAction, ElementHeader};
use crate::media_metadata::matroska::ids;

#[derive(Debug, Default)]
pub struct AudioBuilder {
    pub sampling_frequency: Option<f64>,
    pub output_sampling_frequency: Option<f64>,
    pub channels: Option<u32>,
    pub bit_depth: Option<u32>,
    pub emphasis: Option<AudioEmphasis>,
}

impl AudioBuilder {
    pub fn build(self) -> AudioTrackProperties {
        // Matroska defaults: SamplingFrequency 8000 Hz, Channels 1 when the
        // elements are absent (mkvtoolnix `read_headers_track_audio`,
        // PARSER-033).
        AudioTrackProperties {
            sampling_frequency: Some(self.sampling_frequency.unwrap_or(8000.0)),
            output_sampling_frequency: self.output_sampling_frequency,
            channels: Some(self.channels.unwrap_or(1)),
            channel_layout: None,
            bit_depth: self.bit_depth,
            emphasis: self.emphasis,
            default_duration_ns: None,
            codec_config: None,
        }
    }
}

pub fn parse(
    src: &mut FileSource,
    parent: &ElementHeader,
    deadline: &Deadline,
    builder: &mut AudioBuilder,
) -> Result<(), ParseError> {
    ebml::walk_children(
        src,
        parent,
        "matroska::track_audio",
        deadline,
        |src, child| match child.id {
            ids::AUDIO_SAMPLING_FREQ => {
                builder.sampling_frequency = Some(ebml::read_float(src, child)?);
                Ok(ChildAction::Consumed)
            }
            ids::AUDIO_OUTPUT_SAMPLING_FREQ => {
                builder.output_sampling_frequency = Some(ebml::read_float(src, child)?);
                Ok(ChildAction::Consumed)
            }
            ids::AUDIO_CHANNELS => {
                builder.channels = Some(ebml::read_uint(src, child)? as u32);
                Ok(ChildAction::Consumed)
            }
            ids::AUDIO_BIT_DEPTH => {
                builder.bit_depth = Some(ebml::read_uint(src, child)? as u32);
                Ok(ChildAction::Consumed)
            }
            ids::AUDIO_EMPHASIS => {
                builder.emphasis = Some(classify_emphasis(ebml::read_uint(src, child)?));
                Ok(ChildAction::Consumed)
            }
            _ => Ok(ChildAction::Skip),
        },
    )
}

fn classify_emphasis(v: u64) -> AudioEmphasis {
    match v {
        0 => AudioEmphasis::None,
        1 => AudioEmphasis::CdAudio,
        3 => AudioEmphasis::CcittJ17,
        4 => AudioEmphasis::Fm5025,
        5 => AudioEmphasis::Fm7550,
        6 => AudioEmphasis::PhonoRiaa,
        7 => AudioEmphasis::PhonoIecN78,
        8 => AudioEmphasis::PhonoTeldec,
        9 => AudioEmphasis::PhonoEmi,
        10 => AudioEmphasis::PhonoColumbiaLp,
        11 => AudioEmphasis::PhonoLondon,
        12 => AudioEmphasis::PhonoNartb,
        _ => AudioEmphasis::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::matroska::ebml::{
        encode_element, encode_element_float, encode_element_uint,
    };
    use std::io::Cursor;

    fn no_deadline() -> Deadline {
        Deadline::new(60_000)
    }

    fn build_audio(payload: Vec<u8>) -> (Vec<u8>, ElementHeader, FileSource) {
        let bytes = encode_element(ids::TRACK_AUDIO, 1, &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
        let header = ebml::read_element_header(&mut s).unwrap();
        (bytes, header, s)
    }

    #[test]
    fn sampling_and_channels_round_trip() {
        let mut payload = Vec::new();
        payload.extend(encode_element_float(ids::AUDIO_SAMPLING_FREQ, 1, 48_000.0));
        payload.extend(encode_element_uint(ids::AUDIO_CHANNELS, 1, 6));
        payload.extend(encode_element_uint(ids::AUDIO_BIT_DEPTH, 2, 24));
        let (_b, h, mut s) = build_audio(payload);
        let mut builder = AudioBuilder::default();
        parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
        let a = builder.build();
        assert_eq!(a.sampling_frequency, Some(48_000.0));
        assert_eq!(a.channels, Some(6));
        assert_eq!(a.bit_depth, Some(24));
    }

    #[test]
    fn output_sampling_frequency_decoded() {
        let mut payload = Vec::new();
        payload.extend(encode_element_float(ids::AUDIO_OUTPUT_SAMPLING_FREQ, 2, 96_000.0));
        let (_b, h, mut s) = build_audio(payload);
        let mut builder = AudioBuilder::default();
        parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
        let a = builder.build();
        assert_eq!(a.output_sampling_frequency, Some(96_000.0));
    }

    #[test]
    fn emphasis_classified() {
        let mut payload = Vec::new();
        payload.extend(encode_element_uint(ids::AUDIO_EMPHASIS, 2, 5));
        let (_b, h, mut s) = build_audio(payload);
        let mut builder = AudioBuilder::default();
        parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
        assert_eq!(builder.build().emphasis, Some(AudioEmphasis::Fm7550));
    }

    #[test]
    fn unknown_emphasis_byte_falls_back_to_other() {
        assert_eq!(classify_emphasis(99), AudioEmphasis::Other);
    }

    #[test]
    fn empty_audio_block_applies_matroska_defaults() {
        // PARSER-033: absent SamplingFrequency/Channels default to 8000 Hz / 1.
        let (_b, h, mut s) = build_audio(Vec::new());
        let mut builder = AudioBuilder::default();
        parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
        let a = builder.build();
        assert_eq!(a.sampling_frequency, Some(8000.0));
        assert_eq!(a.channels, Some(1));
        assert!(a.bit_depth.is_none());
    }

    #[test]
    fn full_emphasis_table_round_trip() {
        for (raw, expected) in [
            (0u64, AudioEmphasis::None),
            (1, AudioEmphasis::CdAudio),
            (3, AudioEmphasis::CcittJ17),
            (4, AudioEmphasis::Fm5025),
            (5, AudioEmphasis::Fm7550),
            (6, AudioEmphasis::PhonoRiaa),
            (7, AudioEmphasis::PhonoIecN78),
            (8, AudioEmphasis::PhonoTeldec),
            (9, AudioEmphasis::PhonoEmi),
            (10, AudioEmphasis::PhonoColumbiaLp),
            (11, AudioEmphasis::PhonoLondon),
            (12, AudioEmphasis::PhonoNartb),
            (99, AudioEmphasis::Other),
        ] {
            assert_eq!(classify_emphasis(raw), expected, "raw={raw}");
        }
    }

    #[test]
    fn unknown_child_is_skipped() {
        let mut payload = Vec::new();
        // Some arbitrary unknown 1-byte element id with payload
        payload.extend(encode_element(0x80, 1, &[1, 2, 3]));
        payload.extend(encode_element_uint(ids::AUDIO_CHANNELS, 1, 2));
        let (_b, h, mut s) = build_audio(payload);
        let mut builder = AudioBuilder::default();
        parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
        assert_eq!(builder.build().channels, Some(2));
    }
}
