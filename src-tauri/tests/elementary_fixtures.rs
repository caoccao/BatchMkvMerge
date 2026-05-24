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

//! End-to-end fixtures for the elementary video readers (AVC / HEVC / MPEG /
//! VC-1 / Dirac / DV / AV1 OBU).

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
    let path = dir.join(format!("bmm-elem-{pid}-{nanos}-{seq}.{ext}"));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    drop(f);
    path
}

// -- MPEG video --

fn build_mpeg_sequence_header(width: u32, height: u32, frame_rate_code: u8) -> Vec<u8> {
    let mut bytes = vec![0x00u8, 0x00, 0x01, 0xB3];
    bytes.push(((width >> 4) & 0xFF) as u8);
    bytes.push((((width & 0x0F) << 4) | ((height >> 8) & 0x0F)) as u8);
    bytes.push((height & 0xFF) as u8);
    bytes.push((1u8 << 4) | (frame_rate_code & 0x0F));
    bytes.extend_from_slice(&[0u8; 4]);
    bytes
}

#[test]
fn parses_mpeg_video_es() {
    let bytes = build_mpeg_sequence_header(720, 480, 5);
    let path = write_tempfile(&bytes, "m2v");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::MpegVideo);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
}

// -- DV --

#[test]
fn parses_dv_ntsc() {
    let mut bytes = vec![0x1F, 0x07, 0x00, 0x00];
    bytes.extend_from_slice(&[0u8; 76]);
    let path = write_tempfile(&bytes, "dv");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Dv);
}

#[test]
fn parses_dv_pal() {
    let mut bytes = vec![0x1F, 0x07, 0x00, 0x80];
    bytes.extend_from_slice(&[0u8; 76]);
    let path = write_tempfile(&bytes, "dv");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Dv);
    assert_eq!(m.tracks[0].codec.name.as_deref(), Some("DV (PAL)"));
}

// -- Dirac --

#[test]
fn parses_dirac() {
    let mut bytes = b"BBCD".to_vec();
    bytes.push(0x00);
    bytes.extend_from_slice(&[0u8; 16]);
    let path = write_tempfile(&bytes, "drc");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Dirac);
}

// -- VC-1 (Advanced Profile sequence header) --

struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self { buf: Vec::new(), bit_index: 0 }
    }
    fn write_bit(&mut self, b: bool) {
        if self.bit_index == 0 {
            self.buf.push(0);
        }
        if b {
            let last = self.buf.len() - 1;
            self.buf[last] |= 1 << (7 - self.bit_index);
        }
        self.bit_index = (self.bit_index + 1) % 8;
    }
    fn write_bits(&mut self, value: u64, n: u32) {
        for i in 0..n {
            self.write_bit((value >> (n - 1 - i)) & 1 != 0);
        }
    }
    fn write_ue(&mut self, value: u32) {
        let codeword = value as u64 + 1;
        let nb = 64 - codeword.leading_zeros();
        for _ in 0..(nb - 1) {
            self.write_bit(false);
        }
        self.write_bits(codeword, nb);
    }
    fn into_bytes(mut self) -> Vec<u8> {
        while self.bit_index != 0 {
            self.write_bit(false);
        }
        self.buf
    }
}

#[test]
fn parses_vc1_advanced_profile() {
    let mut w = BitWriter::new();
    w.write_bits(3, 2); // profile = Advanced
    w.write_bits(4, 3); // level
    w.write_bits(1, 2); // colordiff
    w.write_bits(0, 3); // frmrtq
    w.write_bits(0, 5); // bitrtq
    w.write_bit(false); // postproc
    w.write_bits(959, 12); // max_w_mb (1920/2-1)
    w.write_bits(539, 12); // max_h_mb (1080/2-1)
    let mut bytes = vec![0x00, 0x00, 0x01, 0x0F];
    bytes.extend(w.into_bytes());
    bytes.extend_from_slice(&[0u8; 8]); // pad so 8-byte safety read succeeds
    let path = write_tempfile(&bytes, "vc1");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Vc1);
}

// -- AVC --

#[test]
fn parses_avc_baseline_1080p() {
    let mut w = BitWriter::new();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(119); // pic_width_in_mbs_minus1
    w.write_ue(67);  // pic_height_in_map_units_minus1
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(0);
    w.write_ue(4);
    // rbsp_trailing_bits
    w.write_bit(true);
    let tail = w.into_bytes();
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67];
    bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
    bytes.extend(tail);
    let path = write_tempfile(&bytes, "h264");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Avc);
}

// -- HEVC --

#[test]
fn parses_hevc_main10_1080p() {
    let mut w = BitWriter::new();
    w.write_bits(0, 4);
    w.write_bits(0, 3);
    w.write_bit(true);
    w.write_bits(0, 2);
    w.write_bit(false);
    w.write_bits(2, 5); // Main 10
    w.write_bits(0, 32);
    w.write_bits(0, 48);
    w.write_bits(120, 8);
    w.write_ue(0);
    w.write_ue(1);
    w.write_ue(1920);
    w.write_ue(1080);
    w.write_bit(false);
    w.write_ue(2);
    w.write_ue(2);
    w.write_bit(true);
    let tail = w.into_bytes();
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x42, 0x01];
    bytes.extend(tail);
    let path = write_tempfile(&bytes, "h265");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::Hevc);
}
