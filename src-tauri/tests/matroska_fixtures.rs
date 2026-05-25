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

//! End-to-end matroska fixtures.  Each test builds a complete synthetic .mkv
//! blob, writes it to a tempfile, then drives `media_metadata::parse` against
//! the file path.  These exercise the full pipeline — probe cascade, EBML
//! head, Segment walker, deferred L1 elements, per-track parsers — through
//! one entry point.

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::matroska::ids;
use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{parse, ParseError, ParseOptions};

// =============================================================================
//   EBML / VINT encoders (duplicated minimally so the test crate stays
//   independent of the internal `matroska::ebml::encode_*` helpers).
// =============================================================================

fn encode_id(id: u32, width: u8) -> Vec<u8> {
    let mut out = vec![0u8; width as usize];
    for i in 0..width as usize {
        out[width as usize - 1 - i] = ((id >> (8 * i)) & 0xFF) as u8;
    }
    out
}

fn encode_size(size: u64) -> Vec<u8> {
    for width in 1u8..=8 {
        let bits = 7 * width as u32;
        let max = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        if size < max {
            let marker_bit = 1u64 << (8 * width as u64 - width as u64);
            let value = marker_bit | size;
            let mut out = vec![0u8; width as usize];
            for i in 0..width as usize {
                out[width as usize - 1 - i] = ((value >> (8 * i)) & 0xFF) as u8;
            }
            return out;
        }
    }
    panic!("size {size} too large for 8-byte VINT");
}

fn elem(id: u32, id_w: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = encode_id(id, id_w);
    out.extend(encode_size(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn elem_uint(id: u32, id_w: u8, value: u64) -> Vec<u8> {
    if value == 0 {
        return elem(id, id_w, &[0u8]);
    }
    let mut bytes_needed = 0usize;
    for byte in 0..8 {
        if (value >> (8 * (7 - byte))) & 0xFF != 0 {
            bytes_needed = 8 - byte;
            break;
        }
    }
    let bytes_needed = bytes_needed.max(1);
    let mut payload = Vec::with_capacity(bytes_needed);
    for i in 0..bytes_needed {
        payload.push(((value >> (8 * (bytes_needed - 1 - i))) & 0xFF) as u8);
    }
    elem(id, id_w, &payload)
}

fn elem_str(id: u32, id_w: u8, value: &str) -> Vec<u8> {
    elem(id, id_w, value.as_bytes())
}

fn elem_float(id: u32, id_w: u8, value: f64) -> Vec<u8> {
    elem(id, id_w, &value.to_bits().to_be_bytes())
}

// =============================================================================
//   Fixture builders
// =============================================================================

fn ebml_head(doc_type: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(elem_uint(ids::EBML_VERSION, 2, 1));
    p.extend(elem_uint(ids::EBML_READ_VERSION, 2, 1));
    p.extend(elem_uint(ids::EBML_MAX_ID_LENGTH, 2, 4));
    p.extend(elem_uint(ids::EBML_MAX_SIZE_LENGTH, 2, 8));
    p.extend(elem_str(ids::DOC_TYPE, 2, doc_type));
    p.extend(elem_uint(ids::DOC_TYPE_VERSION, 2, 4));
    p.extend(elem_uint(ids::DOC_TYPE_READ_VERSION, 2, 2));
    elem(ids::EBML, 4, &p)
}

fn info_block(title: &str, muxing_app: &str, writing_app: &str) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(elem_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000));
    p.extend(elem_float(ids::DURATION, 2, 60_000.0));
    p.extend(elem_str(ids::TITLE, 2, title));
    p.extend(elem_str(ids::MUXING_APP, 2, muxing_app));
    p.extend(elem_str(ids::WRITING_APP, 2, writing_app));
    p.extend(elem(ids::SEGMENT_UID, 2, &[0xCA, 0xFE, 0xBA, 0xBE]));
    elem(ids::INFO, 4, &p)
}

fn video_track() -> Vec<u8> {
    let mut tp = Vec::new();
    tp.extend(elem_uint(ids::TRACK_NUMBER, 1, 1));
    tp.extend(elem_uint(ids::TRACK_UID, 2, 0xCAFE0001));
    tp.extend(elem_uint(ids::TRACK_TYPE, 1, 1)); // video
    tp.extend(elem_uint(ids::FLAG_DEFAULT, 1, 1));
    tp.extend(elem_uint(ids::FLAG_FORCED, 2, 0));
    tp.extend(elem_str(ids::CODEC_ID, 1, "V_MPEG4/ISO/AVC"));
    tp.extend(elem_str(ids::TRACK_LANGUAGE, 3, "eng"));

    let mut vp = Vec::new();
    vp.extend(elem_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920));
    vp.extend(elem_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    vp.extend(elem_uint(ids::VIDEO_DISPLAY_WIDTH, 2, 1920));
    vp.extend(elem_uint(ids::VIDEO_DISPLAY_HEIGHT, 2, 1080));

    tp.extend(elem(ids::TRACK_VIDEO, 1, &vp));
    elem(ids::TRACK_ENTRY, 1, &tp)
}

fn audio_track() -> Vec<u8> {
    let mut tp = Vec::new();
    tp.extend(elem_uint(ids::TRACK_NUMBER, 1, 2));
    tp.extend(elem_uint(ids::TRACK_UID, 2, 0xCAFE0002));
    tp.extend(elem_uint(ids::TRACK_TYPE, 1, 2)); // audio
    tp.extend(elem_str(ids::CODEC_ID, 1, "A_AAC"));
    tp.extend(elem_str(ids::TRACK_LANGUAGE, 3, "jpn"));

    let mut ap = Vec::new();
    ap.extend(elem_float(ids::AUDIO_SAMPLING_FREQ, 1, 48_000.0));
    ap.extend(elem_uint(ids::AUDIO_CHANNELS, 1, 6));
    ap.extend(elem_uint(ids::AUDIO_BIT_DEPTH, 2, 16));

    tp.extend(elem(ids::TRACK_AUDIO, 1, &ap));
    elem(ids::TRACK_ENTRY, 1, &tp)
}

fn subtitle_track() -> Vec<u8> {
    let mut tp = Vec::new();
    tp.extend(elem_uint(ids::TRACK_NUMBER, 1, 3));
    tp.extend(elem_uint(ids::TRACK_UID, 2, 0xCAFE0003));
    tp.extend(elem_uint(ids::TRACK_TYPE, 1, 17)); // subtitle
    tp.extend(elem_uint(ids::FLAG_FORCED, 2, 1));
    tp.extend(elem_str(ids::CODEC_ID, 1, "S_TEXT/UTF8"));
    tp.extend(elem_str(ids::TRACK_LANGUAGE, 3, "fra"));
    elem(ids::TRACK_ENTRY, 1, &tp)
}

fn tracks_block() -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(video_track());
    p.extend(audio_track());
    p.extend(subtitle_track());
    elem(ids::TRACKS, 4, &p)
}

fn attachments_block() -> Vec<u8> {
    let mut att = Vec::new();
    att.extend(elem_str(ids::FILE_NAME, 2, "cover.jpg"));
    att.extend(elem_str(ids::FILE_MIME_TYPE, 2, "image/jpeg"));
    att.extend(elem_uint(ids::FILE_UID, 2, 0xDEADBEEF));
    att.extend(elem(ids::FILE_DATA, 2, &vec![0u8; 64]));
    let attached = elem(ids::ATTACHED_FILE, 2, &att);
    elem(ids::ATTACHMENTS, 4, &attached)
}

fn chapters_block() -> Vec<u8> {
    let mut atom1 = elem_uint(ids::CHAPTER_UID, 2, 1);
    atom1.extend(elem_uint(ids::CHAPTER_TIMESTAMP_START, 1, 0));
    let atom1 = elem(ids::CHAPTER_ATOM, 1, &atom1);

    let mut atom2 = elem_uint(ids::CHAPTER_UID, 2, 2);
    atom2.extend(elem_uint(ids::CHAPTER_TIMESTAMP_START, 1, 1));
    let atom2 = elem(ids::CHAPTER_ATOM, 1, &atom2);

    let mut edition = Vec::new();
    edition.extend(atom1);
    edition.extend(atom2);
    let edition = elem(ids::EDITION_ENTRY, 2, &edition);
    elem(ids::CHAPTERS, 4, &edition)
}

fn tags_block_with_global_title() -> Vec<u8> {
    let mut simple = Vec::new();
    simple.extend(elem_str(ids::TAG_NAME, 2, "TITLE"));
    simple.extend(elem_str(ids::TAG_STRING, 2, "Synthetic Movie"));
    let simple = elem(ids::TAG_SIMPLE, 2, &simple);
    let tag = elem(ids::TAG, 2, &simple);
    elem(ids::TAGS, 4, &tag)
}

fn segment_with_all_blocks(doc_type: &str, title: &str) -> Vec<u8> {
    let mut head = ebml_head(doc_type);
    let mut payload = Vec::new();
    payload.extend(info_block(title, "libmkv", "mkvmerge v89.0"));
    payload.extend(tracks_block());
    payload.extend(attachments_block());
    payload.extend(chapters_block());
    payload.extend(tags_block_with_global_title());
    head.extend(elem(ids::SEGMENT, 4, &payload));
    head
}

fn write_to_tempfile(bytes: &[u8], ext: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("bmm-fixture-{pid}-{nanos}-{seq}.{ext}"));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(bytes).unwrap();
    drop(f);
    path
}

// =============================================================================
//   Tests
// =============================================================================

#[test]
fn parses_complete_matroska_with_three_tracks() {
    let bytes = segment_with_all_blocks("matroska", "Full Synthetic Clip");
    let path = write_to_tempfile(&bytes, "mkv");

    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(m.container.format, ContainerFormat::Matroska);
    assert!(m.container.recognized);
    assert!(m.container.supported);
    assert_eq!(
        m.container.properties.title.as_deref(),
        Some("Full Synthetic Clip"),
    );
    assert!(m.container.properties.muxing_app.is_some());
    assert!(m.container.properties.writing_app.is_some());
    assert_eq!(m.container.properties.timestamp_scale, Some(1_000_000));

    assert_eq!(m.tracks.len(), 3);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.tracks[1].track_type, TrackType::Audio);
    assert_eq!(m.tracks[2].track_type, TrackType::Subtitles);

    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
    assert_eq!(v.pixel_dimensions.unwrap().height, 1080);

    let a = m.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48_000.0));
    assert_eq!(a.channels, Some(6));

    let sub = m.tracks[2].properties.subtitle.as_ref().unwrap();
    assert!(sub.text_subtitles);

    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.attachments[0].file_name, "cover.jpg");
    assert_eq!(m.attachments[0].size, 64);

    assert_eq!(m.chapters.num_editions, 1);
    assert_eq!(m.chapters.num_entries, 2);

    assert_eq!(m.tags.global.len(), 1);
    assert_eq!(m.tags.global[0].name, "TITLE");
}

#[test]
fn detects_webm_doc_type() {
    let bytes = segment_with_all_blocks("webm", "WebM clip");
    let path = write_to_tempfile(&bytes, "webm");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.container.format, ContainerFormat::WebM);
}

#[test]
fn missing_file_returns_io_error() {
    let err = parse(
        "definitely-does-not-exist-99999.mkv",
        ParseOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, ParseError::Io { .. }));
}

#[test]
fn garbage_returns_unrecognised() {
    let bytes = vec![0x42u8; 32];
    let path = write_to_tempfile(&bytes, "bin");
    let err = parse(&path, ParseOptions::default()).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert!(matches!(err, ParseError::Unrecognised));
}

#[test]
fn timeout_surfaces_as_parse_error() {
    let bytes = segment_with_all_blocks("matroska", "Test");
    let path = write_to_tempfile(&bytes, "mkv");
    let err = parse(
        &path,
        ParseOptions {
            timeout_ms: 1, // ridiculously low — should fire fast
            max_element_size: 16 * 1024 * 1024,
            subtitle_charset: String::new(),
        },
    );
    let _ = std::fs::remove_file(&path);
    // Depending on hardware we may or may not actually exceed the 1 ms budget;
    // we accept either a Timeout error or a successful parse — the goal is to
    // prove the path is exercised without panicking.
    match err {
        Ok(_) => {}
        Err(ParseError::Timeout { .. }) => {}
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn file_name_field_set_from_path() {
    let bytes = segment_with_all_blocks("matroska", "Name Test");
    let path = write_to_tempfile(&bytes, "mkv");
    let expected_name = path.file_name().unwrap().to_string_lossy().to_string();
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.file_name, expected_name);
}

#[test]
fn file_size_field_matches_on_disk_size() {
    let bytes = segment_with_all_blocks("matroska", "Size Test");
    let path = write_to_tempfile(&bytes, "mkv");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(m.file_size, bytes.len() as u64);
}

#[test]
fn track_default_flag_round_trip_through_parse() {
    let bytes = segment_with_all_blocks("matroska", "Flags");
    let path = write_to_tempfile(&bytes, "mkv");
    let m = parse(&path, ParseOptions::default()).unwrap();
    let _ = std::fs::remove_file(&path);
    use batch_mkvmerge_lib::media_metadata::model::track_properties_common::TrackFlag;
    assert_eq!(m.tracks[0].properties.common.default, TrackFlag::True);
    assert_eq!(m.tracks[2].properties.common.forced, TrackFlag::True);
}
