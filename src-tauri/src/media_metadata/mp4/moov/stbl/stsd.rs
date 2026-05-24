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

//! `stsd` (sample description) box.
//!
//! Layout: 1B version + 3B flags + 4B entry_count + `entry_count` sample
//! entry boxes.  Each entry is a sub-box keyed by FOURCC (e.g. `avc1`,
//! `mp4a`, `hev1`, `vp09`).  Sample entries share a common 8-byte header
//! (`reserved[6] + data_reference_index`), after which the layout depends on
//! the handler type:
//!
//! - Video entries: 16B QuickTime-style preamble + width/height + 8B
//!   resolution + 4B reserved + 2B frame_count + 32B compressor_name
//!   + 2B depth + 2B color_table_id, then child boxes (`avcC`, `hvcC`,
//!   `colr`, `pasp`, `dvcC`, ...).
//! - Audio entries: 8B QuickTime version+revision+vendor + 2B channels
//!   + 2B sample_size + 4B reserved + 4B sample_rate, then v1/v2 extras and
//!   child boxes (`esds`, `dec1` ...).
//!
//! We extract:
//! - Sample entry FOURCC (mapped through `codec::fourcc::lookup`).
//! - Video width/height + depth (bit depth).
//! - Audio sample rate, channels, sample size.
//! - Dispatch into [`crate::media_metadata::mp4::codec_specific`] for every
//!   nested codec-config box.

use crate::media_metadata::codec::fourcc;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use crate::media_metadata::mp4::atom::{self, BoxHeader};
use crate::media_metadata::mp4::codec_specific;

use crate::media_metadata::mp4::moov::trak::TrackBuilder;

const VIDEO_PREAMBLE_BYTES: u64 = 16; // version+revision+vendor+temporal+spatial
// hres(4) + vres(4) + reserved(4) + frame_count(2) + compressor_name(32)
// + depth(2) + color_table_id(2) = 50
const VIDEO_TAIL_BYTES: u64 = 4 + 4 + 4 + 2 + 32 + 2 + 2;
const AUDIO_PREAMBLE_BYTES: u64 = 8; // version+revision+vendor
const AUDIO_FIXED_BYTES: u64 = 12; // channels+sample_size+reserved+sample_rate

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    let payload = parent.payload_size().unwrap_or(0);
    if payload < 8 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: parent.start,
            reason: format!("stsd payload {payload} bytes is too small"),
        });
    }
    // 1B version + 3B flags + 4B entry_count = 8B header
    src.skip(4)?;
    let entry_count = src.read_u32_be()?;
    if entry_count == 0 {
        return Ok(());
    }
    // mkvmerge identification reads only the first entry.  Walk the rest in
    // case future phases need them but only populate the builder on entry 0.
    let mut entry_idx = 0u32;
    let stop_at = parent.end();
    while entry_idx < entry_count {
        let pos = src.position();
        if let Some(end) = stop_at {
            if pos >= end {
                break;
            }
        }
        deadline.check("mp4::stsd")?;
        let entry = atom::read_box_header(src)?;
        if entry_idx == 0 {
            parse_first_entry(src, &entry, builder, deadline)?;
        }
        // Advance to next sibling regardless.
        atom::skip_payload(src, &entry)?;
        entry_idx += 1;
    }
    Ok(())
}

fn parse_first_entry(
    src: &mut FileSource,
    entry: &BoxHeader,
    builder: &mut TrackBuilder,
    deadline: &Deadline,
) -> Result<(), ParseError> {
    let payload = entry.payload_size().unwrap_or(0);
    if payload < 8 {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: entry.start,
            reason: format!("sample entry payload {payload} too small"),
        });
    }
    builder.sample_entry_kind = Some(entry.kind.0);
    let codec_str: String = entry.kind.0.iter().map(|b| *b as char).collect();
    builder.codec_id_str = Some(codec_str.clone());
    if let Some(catalogue) = fourcc::lookup(&codec_str) {
        builder.codec_name = Some(catalogue.name.to_string());
    }

    // Common 8-byte sample entry header
    src.skip(6)?; // reserved
    let _data_ref_index = src.read_u16_be()?;
    let mut bytes_consumed: u64 = 8;

    let handler = builder.handler_type;
    match handler {
        Some(h) if &h == b"vide" => {
            bytes_consumed += parse_video_sample_entry(src, entry, builder, payload, bytes_consumed)?;
        }
        Some(h) if &h == b"soun" => {
            bytes_consumed += parse_audio_sample_entry(src, entry, builder, payload, bytes_consumed)?;
        }
        _ => {
            // Unknown handler — leave the cursor where it is.
        }
    }

    // Walk any remaining bytes as child sample-entry sub-boxes (avcC, hvcC,
    // esds, colr, pasp, dvcC, ...).
    let remaining = payload.saturating_sub(bytes_consumed);
    if remaining >= 8 {
        let synthetic = BoxHeader {
            start: entry.payload_start() + bytes_consumed,
            kind: entry.kind,
            header_len: 0,
            total_size: Some(remaining),
        };
        src.seek_to(entry.payload_start() + bytes_consumed)?;
        walk_sample_entry_children(src, &synthetic, deadline, builder)?;
    }
    Ok(())
}

fn parse_video_sample_entry(
    src: &mut FileSource,
    entry: &BoxHeader,
    builder: &mut TrackBuilder,
    payload: u64,
    consumed_so_far: u64,
) -> Result<u64, ParseError> {
    let need = VIDEO_PREAMBLE_BYTES + 4 + VIDEO_TAIL_BYTES; // 16 + 4 dims + 48 tail = 68
    if payload < consumed_so_far + need {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: entry.start,
            reason: format!("video sample entry payload too short ({payload} bytes)"),
        });
    }
    src.skip(VIDEO_PREAMBLE_BYTES)?;
    let width = src.read_u16_be()? as u32;
    let height = src.read_u16_be()? as u32;
    src.skip(8 + 4 + 2 + 32)?; // hres+vres+reserved+frame_count+compressor
    let depth = src.read_u16_be()?;
    src.skip(2)?; // color_table_id
    let mut video = VideoTrackProperties::default();
    if width != 0 && height != 0 {
        video.pixel_dimensions = Some(Dimensions2D { width, height });
        video.display_dimensions = builder.display_dimensions().or(Some(Dimensions2D { width, height }));
    }
    if depth != 0 && depth != 24 {
        // Stash QT depth byte as bits-per-channel hint when not the default 24.
        if let Some(color) = video.color.as_mut() {
            color.bits_per_channel = Some(depth as u32);
        } else {
            video.color = Some(
                crate::media_metadata::model::track_properties_video::ColorMetadata {
                    bits_per_channel: Some(depth as u32),
                    ..Default::default()
                },
            );
        }
    }
    builder.video = Some(video);
    Ok(VIDEO_PREAMBLE_BYTES + 4 + VIDEO_TAIL_BYTES)
}

fn parse_audio_sample_entry(
    src: &mut FileSource,
    entry: &BoxHeader,
    builder: &mut TrackBuilder,
    payload: u64,
    consumed_so_far: u64,
) -> Result<u64, ParseError> {
    let need = AUDIO_PREAMBLE_BYTES + AUDIO_FIXED_BYTES;
    if payload < consumed_so_far + need {
        return Err(ParseError::Malformed {
            format: "mp4",
            offset: entry.start,
            reason: format!("audio sample entry payload too short ({payload} bytes)"),
        });
    }
    let version = src.read_u16_be()?;
    let _revision = src.read_u16_be()?;
    let _vendor = src.read_u32_be()?;
    let channels = src.read_u16_be()? as u32;
    let sample_size = src.read_u16_be()? as u32;
    let _compression_id = src.read_u16_be()?;
    let _packet_size = src.read_u16_be()?;
    let sample_rate_fixed = src.read_u32_be()?; // 16.16 fixed-point in v0
    let mut sample_rate_hz = (sample_rate_fixed >> 16) as f64;

    let mut bytes = AUDIO_PREAMBLE_BYTES + AUDIO_FIXED_BYTES;

    // v1: 16 more bytes of layout (samples_per_packet, etc.)
    if version >= 1 {
        let extra = 16u64;
        if payload >= consumed_so_far + bytes + extra {
            src.skip(extra)?;
            bytes += extra;
        }
    }
    // v2: 36 more bytes including an explicit float64 sample rate.
    if version >= 2 {
        let extra = 36u64;
        if payload >= consumed_so_far + bytes + extra {
            let _v2_size = src.read_u32_be()?;
            let raw = src.read_u64_be()?;
            sample_rate_hz = f64::from_bits(raw);
            // skip remaining 24 bytes
            src.skip(24)?;
            bytes += extra;
        }
    }

    let mut audio = AudioTrackProperties::default();
    if channels != 0 {
        audio.channels = Some(channels);
    }
    if sample_rate_hz > 0.0 {
        audio.sampling_frequency = Some(sample_rate_hz);
    }
    if sample_size != 0 {
        audio.bit_depth = Some(sample_size);
    }
    builder.audio = Some(audio);
    Ok(bytes)
}

fn walk_sample_entry_children(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
    builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
    // The synthetic header we built has start = parent.payload_start() + offset,
    // header_len = 0, and total_size = remaining bytes. The atom walker uses
    // payload_start() = start + header_len, so this iterates from `start`.
    let end = parent.end();
    let stream_end = src.length();
    while let Some(remaining) = remaining_in_parent(src, end, stream_end) {
        if remaining < 8 {
            break;
        }
        deadline.check("mp4::sample_entry_children")?;
        let child = match atom::read_box_header(src) {
            Ok(h) => h,
            Err(ParseError::UnexpectedEof { .. }) => break,
            Err(e) => return Err(e),
        };
        if let (Some(end_pos), Some(child_end)) = (end, child.end()) {
            if child_end > end_pos {
                break; // malformed; stop quietly
            }
        }
        match &child.kind.0 {
            b"avcC" => codec_specific::avcc::parse(src, &child, builder)?,
            b"hvcC" => codec_specific::hvcc::parse(src, &child, builder)?,
            b"esds" => codec_specific::esds::parse(src, &child, builder)?,
            b"colr" => codec_specific::colr::parse(src, &child, builder)?,
            b"pasp" => codec_specific::pasp::parse(src, &child, builder)?,
            b"dvcC" | b"dvvC" => codec_specific::dvcc::parse(src, &child, builder)?,
            _ => {}
        }
        atom::skip_payload(src, &child)?;
    }
    Ok(())
}

fn remaining_in_parent(
    src: &FileSource,
    parent_end: Option<u64>,
    stream_end: Option<u64>,
) -> Option<u64> {
    let pos = src.position();
    let p = parent_end.map(|e| e.saturating_sub(pos));
    let s = stream_end.map(|e| e.saturating_sub(pos));
    match (p, s) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

#[cfg(test)]
pub(crate) fn build_video_sample_entry(
    fourcc_kind: &[u8; 4],
    width: u16,
    height: u16,
    depth: u16,
    children: &[u8],
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0u8; 6]); // reserved
    p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    // 16-byte QuickTime preamble
    p.extend_from_slice(&[0u8; 16]);
    p.extend_from_slice(&width.to_be_bytes());
    p.extend_from_slice(&height.to_be_bytes());
    p.extend_from_slice(&[0u8; 8]); // hres + vres
    p.extend_from_slice(&[0u8; 4]); // reserved
    p.extend_from_slice(&[0u8; 2]); // frame_count
    p.extend_from_slice(&[0u8; 32]); // compressor name
    p.extend_from_slice(&depth.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes()); // color_table_id
    p.extend_from_slice(children);
    crate::media_metadata::mp4::atom::encode_box(fourcc_kind, &p)
}

#[cfg(test)]
pub(crate) fn build_audio_sample_entry_v0(
    fourcc_kind: &[u8; 4],
    channels: u16,
    sample_size: u16,
    sample_rate_hz: u32,
    children: &[u8],
) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0u8; 6]); // reserved
    p.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    p.extend_from_slice(&0u16.to_be_bytes()); // version 0
    p.extend_from_slice(&[0u8; 2 + 4]); // revision + vendor
    p.extend_from_slice(&channels.to_be_bytes());
    p.extend_from_slice(&sample_size.to_be_bytes());
    p.extend_from_slice(&[0u8; 2 + 2]); // compression_id + packet_size
    p.extend_from_slice(&(sample_rate_hz << 16).to_be_bytes());
    p.extend_from_slice(children);
    crate::media_metadata::mp4::atom::encode_box(fourcc_kind, &p)
}

#[cfg(test)]
pub(crate) fn build_stsd_payload(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&[0u8; 4]); // version + flags
    p.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for e in entries {
        p.extend_from_slice(e);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mp4::atom::encode_box;
    use std::io::Cursor;

    fn dl() -> Deadline {
        Deadline::new(60_000)
    }

    fn run(payload: Vec<u8>, handler: [u8; 4]) -> TrackBuilder {
        let stsd = encode_box(b"stsd", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(stsd));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        b.handler_type = Some(handler);
        parse(&mut s, &parent, &dl(), &mut b).unwrap();
        b
    }

    #[test]
    fn video_avc1_entry_extracts_dims() {
        let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        assert_eq!(b.codec_id_str.as_deref(), Some("avc1"));
        let v = b.video.unwrap();
        assert_eq!(v.pixel_dimensions, Some(Dimensions2D { width: 1920, height: 1080 }));
    }

    #[test]
    fn video_depth_stored_when_not_24() {
        let entry = build_video_sample_entry(b"avc1", 1920, 1080, 32, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        let v = b.video.unwrap();
        assert_eq!(v.color.unwrap().bits_per_channel, Some(32));
    }

    #[test]
    fn audio_mp4a_entry_extracts_channels_and_rate() {
        let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48000, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"soun");
        let a = b.audio.unwrap();
        assert_eq!(a.channels, Some(2));
        assert_eq!(a.bit_depth, Some(16));
        assert_eq!(a.sampling_frequency, Some(48000.0));
        assert_eq!(b.codec_id_str.as_deref(), Some("mp4a"));
    }

    #[test]
    fn empty_stsd_does_not_populate() {
        let payload = build_stsd_payload(&[]);
        let b = run(payload, *b"vide");
        assert!(b.video.is_none());
        assert!(b.codec_id_str.is_none());
    }

    #[test]
    fn truncated_payload_rejected() {
        let payload = vec![0u8; 4]; // missing entry_count
        let stsd = encode_box(b"stsd", &payload);
        let mut s = FileSource::from_reader_for_test(Cursor::new(stsd));
        let parent = atom::read_box_header(&mut s).unwrap();
        let mut b = TrackBuilder::default();
        b.handler_type = Some(*b"vide");
        let err = parse(&mut s, &parent, &dl(), &mut b).unwrap_err();
        assert!(matches!(err, ParseError::Malformed { .. }));
    }

    #[test]
    fn unknown_handler_keeps_codec_str_but_no_typed_subtree() {
        let entry = build_video_sample_entry(b"junk", 100, 100, 24, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"meta");
        assert_eq!(b.codec_id_str.as_deref(), Some("junk"));
        assert!(b.video.is_none());
        assert!(b.audio.is_none());
    }

    #[test]
    fn audio_zero_channels_skipped() {
        let entry = build_audio_sample_entry_v0(b"mp4a", 0, 0, 0, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"soun");
        let a = b.audio.unwrap();
        assert!(a.channels.is_none());
        assert!(a.sampling_frequency.is_none());
        assert!(a.bit_depth.is_none());
    }

    #[test]
    fn unknown_fourcc_does_not_set_codec_name() {
        let entry = build_video_sample_entry(b"ZZZZ", 100, 100, 24, &[]);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        assert_eq!(b.codec_id_str.as_deref(), Some("ZZZZ"));
        assert!(b.codec_name.is_none());
    }

    #[test]
    fn video_entry_with_avcc_child_decodes_codec_config() {
        // Build an avc1 sample entry that carries an embedded avcC child.
        let avcc_payload = crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
            100, 40, 3, &[&[0u8; 4]], &[&[0u8; 2]], Some((1, 2, 2)),
        );
        let avcc = encode_box(b"avcC", &avcc_payload);
        let entry = build_video_sample_entry(b"avc1", 1920, 1080, 24, &avcc);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        let cfg = b.video_codec_config.unwrap();
        assert_eq!(cfg.profile_idc, Some(100));
        assert_eq!(
            cfg.chroma_format,
            Some(crate::media_metadata::model::track_properties_video::ChromaFormat::Yuv420)
        );
    }

    #[test]
    fn audio_entry_with_esds_child_decodes_codec_config() {
        let esds_payload =
            crate::media_metadata::mp4::codec_specific::esds::build_esds_payload(
                0x40,
                &[0x12u8, 0x10],
            );
        let esds = encode_box(b"esds", &esds_payload);
        let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48_000, &esds);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"soun");
        let cfg = b.audio_codec_config.unwrap();
        assert_eq!(cfg.aac_object_type, Some(2));
    }

    #[test]
    fn second_entry_is_walked_but_only_first_populates_builder() {
        let entry_a = build_video_sample_entry(b"avc1", 1920, 1080, 24, &[]);
        let entry_b = build_video_sample_entry(b"hev1", 3840, 2160, 24, &[]);
        let payload = build_stsd_payload(&[entry_a, entry_b]);
        let b = run(payload, *b"vide");
        // First entry wins.
        assert_eq!(b.codec_id_str.as_deref(), Some("avc1"));
        let v = b.video.unwrap();
        assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
    }

    #[test]
    fn video_entry_with_pasp_child_records_aspect() {
        let pasp = encode_box(
            b"pasp",
            &crate::media_metadata::mp4::codec_specific::pasp::build_pasp_payload(40, 33),
        );
        let entry = build_video_sample_entry(b"avc1", 720, 480, 24, &pasp);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        let cfg = b.video_codec_config.unwrap();
        let par = cfg.sample_aspect_ratio.unwrap();
        assert_eq!(par.num, 40);
        assert_eq!(par.den, 33);
    }

    #[test]
    fn video_entry_with_colr_child_decodes_colour() {
        let colr_payload =
            crate::media_metadata::mp4::codec_specific::colr::build_nclx_payload(
                9, 16, 9, true,
            );
        let colr = encode_box(b"colr", &colr_payload);
        let entry = build_video_sample_entry(b"avc1", 3840, 2160, 24, &colr);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        let v = b.video.unwrap();
        let c = v.color.unwrap();
        assert_eq!(c.primaries, Some(9));
        assert_eq!(c.matrix_coefficients, Some(9));
    }

    #[test]
    fn video_entry_with_dvcc_child_decodes_dolby_vision() {
        let dvcc_payload =
            crate::media_metadata::mp4::codec_specific::dvcc::build_dvcc_payload(8, 6, true, true, true);
        let dvcc = encode_box(b"dvcC", &dvcc_payload);
        let entry = build_video_sample_entry(b"hev1", 3840, 2160, 24, &dvcc);
        let payload = build_stsd_payload(&[entry]);
        let b = run(payload, *b"vide");
        let cfg = b.video_codec_config.unwrap();
        assert!(cfg.profile_name.unwrap().contains("Dolby Vision"));
    }
}
