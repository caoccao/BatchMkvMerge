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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Audio-track-only properties.  Populated only on tracks whose `trackType` is
/// `Audio`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackProperties {
    pub sampling_frequency: Option<f64>,
    pub output_sampling_frequency: Option<f64>,
    pub channels: Option<u32>,
    pub channel_layout: Option<ChannelLayout>,
    pub bit_depth: Option<u32>,
    pub emphasis: Option<AudioEmphasis>,
    /// Nominal per-frame duration in nanoseconds (Matroska `DefaultDuration`).
    #[specta(type = Option<Number>)]
    pub default_duration_ns: Option<u64>,
    pub codec_config: Option<AudioCodecConfig>,
}

/// Best-effort channel layout description.  `channels` is the count; `kind`
/// is the canonical layout name when known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChannelLayout {
    pub channels: u32,
    pub kind: Option<ChannelLayoutKind>,
    /// Raw bitmap when the source format provides one (Matroska
    /// `ChannelPositions`, WAV channel mask, ...).  Hex-encoded little-endian.
    pub raw_mask_hex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChannelLayoutKind {
    Mono,
    Stereo,
    Layout21,
    Layout30,
    Layout31,
    Layout40,
    Layout41,
    Layout50,
    Layout51,
    Layout61,
    Layout71,
    Layout714,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AudioEmphasis {
    None,
    CdAudio,
    CcittJ17,
    Fm5025,
    Fm7550,
    PhonoRiaa,
    PhonoIecN78,
    PhonoTeldec,
    PhonoEmi,
    PhonoColumbiaLp,
    PhonoLondon,
    PhonoNartb,
    Other,
}

/// Decoded audio codec-private blob.  See [`super::track_properties_video::VideoCodecConfig`]
/// for the rationale on the shared opt-in struct shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AudioCodecConfig {
    pub profile_name: Option<String>,
    /// MPEG-4 Audio Object Type (5 for AAC SBR, 29 for PS, ...).
    pub aac_object_type: Option<u32>,
    pub aac_frame_length: Option<u32>,
    pub aac_sbr_present: Option<bool>,
    pub aac_ps_present: Option<bool>,
    pub flac_min_block_size: Option<u32>,
    pub flac_max_block_size: Option<u32>,
    pub flac_min_frame_size: Option<u32>,
    pub flac_max_frame_size: Option<u32>,
    #[specta(type = Option<Number>)]
    pub flac_total_samples: Option<u64>,
    /// MD5 of the unencoded audio data (FLAC STREAMINFO last 16 bytes).
    pub flac_md5_hex: Option<String>,
    pub raw_hex: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let a = AudioTrackProperties::default();
        assert!(a.sampling_frequency.is_none());
        assert!(a.channel_layout.is_none());
    }

    #[test]
    fn round_trip_through_json() {
        let a = AudioTrackProperties {
            sampling_frequency: Some(48_000.0),
            output_sampling_frequency: Some(96_000.0),
            channels: Some(6),
            channel_layout: Some(ChannelLayout {
                channels: 6,
                kind: Some(ChannelLayoutKind::Layout51),
                raw_mask_hex: Some("3f00".to_owned()),
            }),
            bit_depth: Some(24),
            emphasis: Some(AudioEmphasis::None),
            default_duration_ns: Some(21_333_333),
            codec_config: Some(AudioCodecConfig {
                profile_name: Some("LC".to_owned()),
                aac_object_type: Some(2),
                aac_frame_length: Some(1024),
                aac_sbr_present: Some(false),
                aac_ps_present: Some(false),
                flac_min_block_size: None,
                flac_max_block_size: None,
                flac_min_frame_size: None,
                flac_max_frame_size: None,
                flac_total_samples: None,
                flac_md5_hex: None,
                raw_hex: Some("1190".to_owned()),
            }),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"samplingFrequency\":48000"));
        assert!(s.contains("\"channelLayout\":{"));
        assert!(s.contains("\"kind\":\"layout51\""));
        let back: AudioTrackProperties = serde_json::from_str(&s).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn channel_layout_kind_round_trip() {
        for kind in [
            ChannelLayoutKind::Mono,
            ChannelLayoutKind::Stereo,
            ChannelLayoutKind::Layout21,
            ChannelLayoutKind::Layout30,
            ChannelLayoutKind::Layout31,
            ChannelLayoutKind::Layout40,
            ChannelLayoutKind::Layout41,
            ChannelLayoutKind::Layout50,
            ChannelLayoutKind::Layout51,
            ChannelLayoutKind::Layout61,
            ChannelLayoutKind::Layout71,
            ChannelLayoutKind::Layout714,
            ChannelLayoutKind::Other,
        ] {
            let back: ChannelLayoutKind =
                serde_json::from_str(&serde_json::to_string(&kind).unwrap()).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn emphasis_round_trip() {
        for e in [
            AudioEmphasis::None,
            AudioEmphasis::CdAudio,
            AudioEmphasis::CcittJ17,
            AudioEmphasis::Fm5025,
            AudioEmphasis::Fm7550,
            AudioEmphasis::PhonoRiaa,
            AudioEmphasis::PhonoIecN78,
            AudioEmphasis::PhonoTeldec,
            AudioEmphasis::PhonoEmi,
            AudioEmphasis::PhonoColumbiaLp,
            AudioEmphasis::PhonoLondon,
            AudioEmphasis::PhonoNartb,
            AudioEmphasis::Other,
        ] {
            let back: AudioEmphasis =
                serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
            assert_eq!(back, e);
        }
    }

    #[test]
    fn flac_codec_config_round_trip() {
        let cfg = AudioCodecConfig {
            profile_name: None,
            aac_object_type: None,
            aac_frame_length: None,
            aac_sbr_present: None,
            aac_ps_present: None,
            flac_min_block_size: Some(4096),
            flac_max_block_size: Some(4096),
            flac_min_frame_size: Some(14),
            flac_max_frame_size: Some(16384),
            flac_total_samples: Some(123_456_789),
            flac_md5_hex: Some("00112233445566778899aabbccddeeff".to_owned()),
            raw_hex: None,
        };
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("\"flacTotalSamples\":123456789"));
        let back: AudioCodecConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }
}
