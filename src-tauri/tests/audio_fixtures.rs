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

//! End-to-end audio fixtures: WAV / FLAC / WAVPACK / TTA / CoreAudio /
//! MP3 / AC-3 / DTS / TrueHD (synthetic — frame-sync formats need padding
//! to satisfy the 8-consecutive-frames probe).

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{parse, ParseOptions};

fn write_tempfile(bytes: &[u8], ext: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("bmm-audio-{pid}-{nanos}-{seq}.{ext}"));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    drop(f);
    path
}

// -- WAV ----------------------------------------------------------------------

fn build_wav(sample_rate: u32, channels: u16, bits: u16, data_bytes: u32) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
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
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    bytes
}

#[test]
fn parses_minimal_wav() {
    let bytes = build_wav(48_000, 2, 16, 192_000);
    let path = write_tempfile(&bytes, "wav");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Wav);
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].track_type, TrackType::Audio);
    let a = m.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
}

// -- FLAC ---------------------------------------------------------------------

fn build_flac_native(sample_rate: u32, channels: u32, bps: u32, total_samples: u64) -> Vec<u8> {
    let mut bytes = b"fLaC".to_vec();
    bytes.push(0x80);
    bytes.extend_from_slice(&[0u8, 0u8, 34]);
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = ((sample_rate as u64) << 44)
        | (((channels - 1) as u64 & 0x7) << 41)
        | (((bps - 1) as u64 & 0x1F) << 36)
        | (total_samples & 0x0F_FFFF_FFFF);
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    bytes
}

#[test]
fn parses_native_flac() {
    let bytes = build_flac_native(48_000, 2, 24, 96_000);
    let path = write_tempfile(&bytes, "flac");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Flac);
    let a = m.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.bit_depth, Some(24));
}

// -- WAVPACK ------------------------------------------------------------------

#[test]
fn parses_wavpack() {
    let sr_index: u32 = 9; // 44_100
    let flags = (sr_index << 23) | 1u32; // bps_index 1 = 16-bit
    let mut bytes = vec![0u8; 32];
    bytes[..4].copy_from_slice(b"wvpk");
    bytes[4..8].copy_from_slice(&100u32.to_le_bytes());
    bytes[8..10].copy_from_slice(&0x0407u16.to_le_bytes());
    bytes[12..16].copy_from_slice(&88_200u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&1024u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&flags.to_le_bytes());
    let path = write_tempfile(&bytes, "wv");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Wavpack);
}

// -- TTA ----------------------------------------------------------------------

#[test]
fn parses_tta() {
    let mut bytes = Vec::with_capacity(22);
    bytes.extend_from_slice(b"TTA1");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&88_200u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let path = write_tempfile(&bytes, "tta");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Tta);
}

// -- CoreAudio ----------------------------------------------------------------

#[test]
fn parses_coreaudio_caf() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"caff");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&0u16.to_be_bytes());
    bytes.extend_from_slice(b"desc");
    bytes.extend_from_slice(&32i64.to_be_bytes());
    bytes.extend_from_slice(&(48_000f64).to_bits().to_be_bytes());
    bytes.extend_from_slice(b"alac");
    bytes.extend_from_slice(&[0u8; 16]);
    bytes.extend_from_slice(&2u32.to_be_bytes()); // channels
    bytes.extend_from_slice(&16u32.to_be_bytes()); // bits
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&100i64.to_be_bytes());
    bytes.extend_from_slice(&[0u8; 100]);
    let path = write_tempfile(&bytes, "caf");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::CoreAudio);
    assert_eq!(m.tracks[0].codec.name.as_deref(), Some("ALAC (Apple Lossless)"));
}

// -- MP3 ----------------------------------------------------------------------

fn build_mp3_frame(bitrate_kbps: u32, sample_rate: u32) -> Vec<u8> {
    const BITRATE_TABLE_V1_LAYER3: [u32; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const SAMPLE_RATE_TABLE_V1: [u32; 4] = [44_100, 48_000, 32_000, 0];
    let bitrate_index = BITRATE_TABLE_V1_LAYER3
        .iter()
        .position(|&b| b == bitrate_kbps)
        .unwrap() as u8;
    let sr_index = SAMPLE_RATE_TABLE_V1
        .iter()
        .position(|&s| s == sample_rate)
        .unwrap() as u8;
    let mut header = [0u8; 4];
    header[0] = 0xFF;
    header[1] = 0xFB;
    header[2] = (bitrate_index << 4) | (sr_index << 2);
    header[3] = 0x00; // stereo
    let frame_length = 144 * bitrate_kbps as usize * 1000 / sample_rate as usize;
    let mut bytes = Vec::with_capacity(frame_length);
    bytes.extend_from_slice(&header);
    bytes.resize(frame_length, 0);
    bytes
}

#[test]
fn parses_mp3_stream() {
    let mut bytes = Vec::new();
    for _ in 0..10 {
        bytes.extend(build_mp3_frame(128, 44_100));
    }
    let path = write_tempfile(&bytes, "mp3");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Mp3);
}

// -- AC-3 ---------------------------------------------------------------------

fn build_ac3_frame() -> Vec<u8> {
    const FRAME_SIZES: [[u16; 3]; 38] = [
        [64, 69, 96], [64, 70, 96], [80, 87, 120], [80, 88, 120],
        [96, 104, 144], [96, 105, 144], [112, 121, 168], [112, 122, 168],
        [128, 139, 192], [128, 140, 192], [160, 174, 240], [160, 175, 240],
        [192, 208, 288], [192, 209, 288], [224, 243, 336], [224, 244, 336],
        [256, 278, 384], [256, 279, 384], [320, 348, 480], [320, 349, 480],
        [384, 417, 576], [384, 418, 576], [448, 487, 672], [448, 488, 672],
        [512, 557, 768], [512, 558, 768], [640, 696, 960], [640, 697, 960],
        [768, 835, 1152], [768, 836, 1152], [896, 975, 1344], [896, 976, 1344],
        [1024, 1114, 1536], [1024, 1115, 1536], [1152, 1253, 1728], [1152, 1254, 1728],
        [1280, 1393, 1920], [1280, 1394, 1920],
    ];
    let fscod = 0u8;
    let frmsizecod = 8u8; // 128 kbps @ 48 kHz
    let len = (FRAME_SIZES[frmsizecod as usize][fscod as usize] as usize) * 2;
    let mut bytes = vec![0u8; len];
    bytes[0] = 0x0B;
    bytes[1] = 0x77;
    bytes[4] = (fscod << 6) | (frmsizecod & 0x3F);
    bytes[5] = 8 << 3; // bsid = 8
    bytes[6] = 0x40; // acmod = 2 (stereo)
    bytes
}

#[test]
fn parses_ac3_stream() {
    let mut bytes = Vec::new();
    for _ in 0..10 {
        bytes.extend(build_ac3_frame());
    }
    let path = write_tempfile(&bytes, "ac3");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Ac3);
}

// -- DTS ----------------------------------------------------------------------

#[test]
fn parses_dts_stream() {
    let mut bytes = vec![0u8; 64];
    bytes[0] = 0x7F;
    bytes[1] = 0xFE;
    bytes[2] = 0x80;
    bytes[3] = 0x01;
    bytes[7] = 0x01;
    bytes[8] = 0b1011_0100;
    let path = write_tempfile(&bytes, "dts");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Dts);
}

// -- TrueHD -------------------------------------------------------------------

#[test]
fn parses_truehd_stream() {
    let mut bytes = vec![0u8; 32];
    bytes[0] = 0xF8;
    bytes[1] = 0x72;
    bytes[2] = 0x6F;
    bytes[3] = 0xBB;
    bytes[8] = 0x10; // sr_index 1 = 96 kHz
    bytes[9] = 0x07;
    let path = write_tempfile(&bytes, "thd");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::TrueHd);
}
