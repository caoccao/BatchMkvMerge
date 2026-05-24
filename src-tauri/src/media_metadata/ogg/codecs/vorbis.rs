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

//! Vorbis identification header (Vorbis I §4.2.2):
//!
//! ```text
//! u8  packet_type      (== 1)
//! 6   "vorbis"
//! u32 vorbis_version   (LE)
//! u8  audio_channels
//! u32 audio_sample_rate (LE)
//! ... (bitrate fields, blocksizes, framing)
//! ```

use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;

use super::BitstreamMetadata;

const SIGNATURE: [u8; 7] = [0x01, b'v', b'o', b'r', b'b', b'i', b's'];

pub fn sniff(packet: &[u8]) -> Option<BitstreamMetadata> {
    if packet.len() < 23 || packet[..7] != SIGNATURE {
        return None;
    }
    let channels = packet[11] as u32;
    let sample_rate = u32::from_le_bytes([packet[12], packet[13], packet[14], packet[15]]);
    let mut metadata = BitstreamMetadata::audio_only("A_VORBIS", "Vorbis");
    metadata.audio = Some(AudioTrackProperties {
        channels: Some(channels),
        sampling_frequency: Some(sample_rate as f64),
        ..AudioTrackProperties::default()
    });
    Some(metadata)
}

#[cfg(test)]
pub(crate) fn build_identification_packet(channels: u8, sample_rate: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(30);
    p.extend_from_slice(&SIGNATURE);
    p.extend_from_slice(&0u32.to_le_bytes()); // vorbis_version
    p.push(channels);
    p.extend_from_slice(&sample_rate.to_le_bytes());
    // bitrate maximum / nominal / minimum (u32 each)
    p.extend_from_slice(&[0u8; 12]);
    p.push(0xB8); // blocksize_0|blocksize_1
    p.push(0x01); // framing bit
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_44100_stereo() {
        let pkt = build_identification_packet(2, 44100);
        let m = sniff(&pkt).unwrap();
        assert_eq!(m.codec_id, "A_VORBIS");
        let a = m.audio.unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.sampling_frequency, Some(44100.0));
    }

    #[test]
    fn sniffs_48000_mono() {
        let pkt = build_identification_packet(1, 48000);
        let m = sniff(&pkt).unwrap();
        let a = m.audio.unwrap();
        assert_eq!(a.channels, Some(1));
        assert_eq!(a.sampling_frequency, Some(48000.0));
    }

    #[test]
    fn rejects_non_vorbis_packet() {
        assert!(sniff(b"\x02vorbis...").is_none());
        assert!(sniff(b"\x01opushea...").is_none());
    }

    #[test]
    fn rejects_too_short_packet() {
        assert!(sniff(&[0x01, b'v']).is_none());
    }

    #[test]
    fn signature_matches_vorbis_spec() {
        assert_eq!(&SIGNATURE, b"\x01vorbis");
    }
}
