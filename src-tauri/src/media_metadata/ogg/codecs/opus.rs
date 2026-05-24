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

//! Opus identification header (RFC 7845 §5.1):
//!
//! ```text
//! 8   "OpusHead"
//! u8  version           (== 1)
//! u8  channel_count
//! u16 pre_skip          (LE)
//! u32 input_sample_rate (LE — original encoder rate, decode always 48 kHz)
//! u16 output_gain       (LE, signed)
//! u8  channel_mapping_family
//! ```

use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;

use super::BitstreamMetadata;

const SIGNATURE: [u8; 8] = *b"OpusHead";

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
    if packet.len() < 19 || packet[..8] != SIGNATURE {
        return None;
    }
    let channels = packet[9] as u32;
    let _pre_skip = u16::from_le_bytes([packet[10], packet[11]]);
    let input_sample_rate =
        u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]);
    let mut metadata = BitstreamMetadata::audio_only("A_OPUS", "Opus");
    // Decode rate is always 48 kHz per RFC 7845; report the original encoder
    // rate as input_sample_rate when present.
    metadata.audio = Some(AudioTrackProperties {
        channels: Some(channels),
        sampling_frequency: Some(48_000.0),
        output_sampling_frequency: if input_sample_rate == 0 {
            None
        } else {
            Some(input_sample_rate as f64)
        },
        ..AudioTrackProperties::default()
    });
    Some(metadata)
}

#[cfg(test)]
pub(crate) fn build_identification_packet(channels: u8, input_sample_rate: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(19);
    p.extend_from_slice(&SIGNATURE);
    p.push(1); // version
    p.push(channels);
    p.extend_from_slice(&0u16.to_le_bytes()); // pre_skip
    p.extend_from_slice(&input_sample_rate.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes()); // output_gain
    p.push(0); // channel mapping family
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_opus_with_input_sample_rate() {
        let pkt = build_identification_packet(2, 48000);
        let m = sniff(&pkt).unwrap();
        assert_eq!(m.codec_id, "A_OPUS");
        let a = m.audio.unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.sampling_frequency, Some(48000.0));
        assert_eq!(a.output_sampling_frequency, Some(48000.0));
    }

    #[test]
    fn output_sample_rate_none_when_input_is_zero() {
        let pkt = build_identification_packet(1, 0);
        let m = sniff(&pkt).unwrap();
        let a = m.audio.unwrap();
        assert!(a.output_sampling_frequency.is_none());
    }

    #[test]
    fn rejects_non_opus_packet() {
        assert!(sniff(b"\x01vorbis...").is_none());
        assert!(sniff(b"AnotherHead").is_none());
    }

    #[test]
    fn rejects_short_packet() {
        assert!(sniff(b"OpusHead").is_none());
    }
}
