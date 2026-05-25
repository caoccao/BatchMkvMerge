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

//! End-to-end fixtures for the subtitle readers (SRT, SSA/ASS, WebVTT, USF,
//! MicroDVD, VobSub, PGS, HDMV TextST, VobButton).

use std::io::Write;

use batch_mkvmerge_lib::media_metadata::model::container::ContainerFormat;
use batch_mkvmerge_lib::media_metadata::model::track::TrackType;
use batch_mkvmerge_lib::media_metadata::{ParseOptions, parse};

fn write_tempfile(bytes: &[u8], ext: &str) -> std::path::PathBuf {
  let dir = std::env::temp_dir();
  let pid = std::process::id();
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
  let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
  let path = dir.join(format!("bmm-subs-{pid}-{nanos}-{seq}.{ext}"));
  let mut f = std::fs::File::create(&path).unwrap();
  f.write_all(bytes).unwrap();
  drop(f);
  path
}

#[test]
fn parses_srt_clip() {
  let blob = b"1\r\n00:00:00,000 --> 00:00:02,500\r\nHello world\r\n\r\n";
  let path = write_tempfile(blob, "srt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Srt);
  assert_eq!(m.tracks[0].track_type, TrackType::Subtitles);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/UTF8");
}

#[test]
fn parses_ass_clip() {
  let blob = b"[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\n";
  let path = write_tempfile(blob, "ass");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::SsaAss);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/ASS");
}

#[test]
fn parses_ssa_clip() {
  let blob = b"[Script Info]\nScriptType: v4.00\n\n[V4 Styles]\n";
  let path = write_tempfile(blob, "ssa");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/SSA");
}

/// SSA UUencode-like scheme (mirror of `ssa.rs::decode_uu`): 3 bytes → 4 chars,
/// each 6-bit group offset by +33.
fn ssa_uu_encode(data: &[u8]) -> String {
  let mut out = String::new();
  let mut i = 0;
  while i < data.len() {
    let chunk = &data[i..(i + 3).min(data.len())];
    let mut value: u32 = 0;
    for (idx, b) in chunk.iter().enumerate() {
      value |= u32::from(*b) << ((2 - idx) * 8);
    }
    let chars_out = if chunk.len() == 3 {
      4
    } else if chunk.len() == 2 {
      3
    } else {
      2
    };
    for idx in 0..chars_out {
      out.push(((((value >> (6 * (3 - idx))) & 0x3f) as u8) + 33) as char);
    }
    i += 3;
  }
  out
}

#[test]
fn parses_ass_with_embedded_font_and_graphics() {
  // PARSER-207: `[Fonts]` / `[Graphics]` sections must be excluded from the
  // codec-private global header, UU-decoded into attachments with the decoded
  // byte size, and MIME-typed from the decoded bytes.
  let ttf = [0x00u8, 0x01, 0x00, 0x00, 0xDE, 0xAD, 0xBE];
  let png = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x11];
  let blob = format!(
    "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name\n\n[Fonts]\nfontname: Embedded.ttf\n{}\n\n[Graphics]\nfontname: pic.png\n{}\n\n[Events]\nFormat: Layer, Start, End\n",
    ssa_uu_encode(&ttf),
    ssa_uu_encode(&png)
  );
  let path = write_tempfile(blob.as_bytes(), "ass");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);

  assert_eq!(m.container.format, ContainerFormat::SsaAss);
  assert_eq!(m.attachments.len(), 2);

  assert_eq!(m.attachments[0].file_name, "Embedded.ttf");
  assert_eq!(m.attachments[0].size, ttf.len() as u64);
  assert_eq!(m.attachments[0].mime_type.as_deref(), Some("font/sfnt"));

  assert_eq!(m.attachments[1].file_name, "pic.png");
  assert_eq!(m.attachments[1].size, png.len() as u64);
  assert_eq!(m.attachments[1].mime_type.as_deref(), Some("image/png"));

  // Codec private (global header) excludes the embedded-media sections.
  let private = m.tracks[0].codec.codec_private.as_ref().unwrap();
  let bytes = hex_to_bytes(&private.hex);
  let header = String::from_utf8_lossy(&bytes);
  assert!(header.contains("[Script Info]"));
  assert!(!header.contains("[Fonts]"));
  assert!(!header.contains("[Graphics]"));
  assert!(!header.contains("fontname:"));
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
  (0..hex.len())
    .step_by(2)
    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
    .collect()
}

#[test]
fn parses_webvtt_clip() {
  let blob = b"WEBVTT\n\n00:00.000 --> 00:02.000\nHello\n";
  let path = write_tempfile(blob, "vtt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Webvtt);
}

#[test]
fn parses_webvtt_with_header_text_after_magic() {
  // PARSER-196: mkvtoolnix claims any file whose first line starts with
  // `WEBVTT`, even when text immediately follows the magic with no separator.
  let blob = b"WEBVTT-some-header\n\n00:00.000 --> 00:02.000\nHello\n";
  let path = write_tempfile(blob, "vtt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Webvtt);
}

#[test]
fn parses_webvtt_with_style_header_over_one_kib() {
  // PARSER-197: a STYLE block exceeding the old 1 KiB cap before the first cue
  // must be captured in full as codec-private (mkvtoolnix parses the whole file).
  let mut blob = String::from("WEBVTT\n\nSTYLE\n");
  blob.push_str(&"::cue(.row) { color: rgb(0, 0, 0) }\n".repeat(64));
  blob.push_str("::cue(.sentinel) { color: rebeccapurple }\n");
  assert!(blob.len() > 1024, "header must exceed the old 1 KiB cap");
  blob.push_str("\n00:00.000 --> 00:02.000\nHello\n");
  let path = write_tempfile(blob.as_bytes(), "vtt");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Webvtt);
  let private = m.tracks[0].codec.codec_private.as_ref().unwrap();
  assert!(private.length as usize > 1024);
  // "sentinel" = 73 65 6e 74 69 6e 65 6c
  assert!(private.hex.contains("73656e74696e656c"));
}

#[test]
fn parses_usf_clip() {
  let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\">\n</USFSubtitles>\n";
  let path = write_tempfile(blob, "usf");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::Usf);
}

#[test]
fn parses_usf_with_metadata_and_per_track_language() {
  // PARSER-208 / PARSER-209: a real XML parse extracts the default language
  // from `<metadata><language code>`, one track per `<subtitles>` element with
  // its own `<language code>`, and applies the default to tracks lacking one.
  let blob = b"<?xml version=\"1.0\"?>\n<USFSubtitles version=\"1.1\">\
    <metadata><title>Demo</title><language code=\"ger\"/></metadata>\
    <subtitles><language code=\"eng\"/><subtitle start=\"0\" stop=\"1\">hi</subtitle></subtitles>\
    <subtitles><subtitle start=\"1\" stop=\"2\">da</subtitle></subtitles>\
    </USFSubtitles>\n";
  let path = write_tempfile(blob, "usf");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);

  assert_eq!(m.container.format, ContainerFormat::Usf);
  assert_eq!(m.tracks.len(), 2);
  assert_eq!(m.tracks[0].codec.id, "S_TEXT/USF");
  // Track 0 carries its own language.
  assert_eq!(
    m.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
    "eng"
  );
  // Track 1 has no language → inherits the metadata default (deu).
  assert_eq!(
    m.tracks[1].properties.common.language.as_ref().unwrap().iso639_2,
    "deu"
  );

  // Shared codec private is the whole document with both `<subtitles>` subtrees
  // removed (mkvtoolnix `create_codec_private`).
  let private = m.tracks[0].codec.codec_private.as_ref().unwrap();
  let header = String::from_utf8_lossy(&hex_to_bytes(&private.hex)).into_owned();
  assert!(header.contains("USFSubtitles"));
  assert!(header.contains("<metadata>"));
  assert!(header.contains("<title>Demo</title>"));
  assert!(!header.contains("<subtitles"));
  assert!(!header.contains("<subtitle "));
  // Both tracks share the same codec private document.
  assert_eq!(
    m.tracks[0].codec.codec_private.as_ref().unwrap().hex,
    m.tracks[1].codec.codec_private.as_ref().unwrap().hex
  );
}

#[test]
fn rejects_usf_without_xml_marker() {
  // PARSER-208: mkvtoolnix's probe requires an `<?xml` or `<!--` marker before
  // it loads the document; a bare root element is not claimed as USF.
  let blob = b"<USFSubtitles><subtitles/></USFSubtitles>";
  let path = write_tempfile(blob, "usf");
  let result = parse(&path, ParseOptions::default());
  let _ = std::fs::remove_file(&path);
  // Not recognised as USF (the .usf extension only biases probe order).
  assert!(result.is_err() || result.unwrap().container.format != ContainerFormat::Usf);
}

#[test]
fn parses_microdvd_clip() {
  let blob = b"{1}{125}Hello world\n{126}{250}Second line\n";
  let path = write_tempfile(blob, "sub");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MicroDvd);
}

#[test]
fn parses_vobsub_idx_with_per_language_entries() {
  let blob = b"# VobSub index file, v7
id: en, index: 0
timestamp: 00:00:01:000, filepos: 000000000
id: ja, index: 1
timestamp: 00:00:02:000, filepos: 000000100
";
  let path = write_tempfile(blob, "idx");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::VobSub);
  assert_eq!(m.tracks.len(), 2);
}

/// PARSER-210: dispatching a `.sub` input resolves to the sibling `.idx` and
/// produces VobSub tracks; the sibling `.sub` is recorded under otherFiles.
#[test]
fn parses_vobsub_sub_input_via_sibling_idx() {
  let dir = std::env::temp_dir();
  let pid = std::process::id();
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let stem = dir.join(format!("bmm-vobsub-pair-{pid}-{nanos}"));
  let idx_path = stem.with_extension("idx");
  let sub_path = stem.with_extension("sub");
  std::fs::write(
    &idx_path,
    b"# VobSub index file, v7\nsize: 720x576\nid: en, index: 0\ntimestamp: 00:00:01:000, filepos: 000000000\nid: fr, index: 1\ntimestamp: 00:00:02:000, filepos: 000001000\n",
  )
  .unwrap();
  std::fs::write(&sub_path, &[0u8; 32]).unwrap();

  // Hand the *.sub* file to the public parser; it must resolve the .idx.
  let m = parse(&sub_path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&idx_path);
  let _ = std::fs::remove_file(&sub_path);

  assert_eq!(m.container.format, ContainerFormat::VobSub);
  assert_eq!(m.tracks.len(), 2);
  assert!(m.container.properties.other_files.iter().any(|f| f.ends_with(".sub")));
}

/// PARSER-210: a `.sub` file that is *not* VobSub (no sibling `.idx`) still
/// falls through to the normal cascade — here MicroDVD claims it.
#[test]
fn vobsub_sub_resolution_does_not_steal_microdvd() {
  let blob = b"{1}{125}Hello world\n{126}{250}Second line\n";
  let path = write_tempfile(blob, "sub");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::MicroDvd);
}

/// PARSER-211: codec private is the filtered global-settings text, not the raw
/// manifest — the `id:`/`timestamp:`/`delay:`/`alt:`/`langidx:` control lines
/// are stripped while `size:` / `palette:` survive.
#[test]
fn vobsub_codec_private_is_filtered_idx_data() {
  let blob = b"# VobSub index file, v7
size: 720x576
palette: 000000, ffffff
langidx: 0
id: en, index: 0
alt: english
delay: 00:00:00:000
timestamp: 00:00:01:000, filepos: 000000000
";
  let path = write_tempfile(blob, "idx");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::VobSub);
  assert_eq!(m.tracks.len(), 1);
  let private = m.tracks[0].codec.codec_private.as_ref().unwrap();
  let bytes: Vec<u8> = (0..private.hex.len())
    .step_by(2)
    .map(|i| u8::from_str_radix(&private.hex[i..i + 2], 16).unwrap())
    .collect();
  let header = String::from_utf8_lossy(&bytes);
  assert!(header.contains("size: 720x576"));
  assert!(header.contains("palette: 000000, ffffff"));
  assert!(!header.contains("id:"));
  assert!(!header.contains("timestamp:"));
  assert!(!header.contains("delay:"));
  assert!(!header.contains("alt:"));
  assert!(!header.contains("langidx:"));
}

fn build_pgs_segment(seg_type: u8, payload_len: u16) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"PG");
  bytes.extend_from_slice(&[0u8; 4]); // PTS
  bytes.extend_from_slice(&[0u8; 4]); // DTS
  bytes.push(seg_type);
  bytes.extend_from_slice(&payload_len.to_be_bytes());
  bytes.extend(std::iter::repeat(0u8).take(payload_len as usize));
  bytes
}

#[test]
fn parses_pgs_sup_clip() {
  let mut blob = build_pgs_segment(0x16, 11); // PCS
  blob.extend(build_pgs_segment(0x17, 9)); // WDS
  let path = write_tempfile(&blob, "sup");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::HdmvPgs);
  let sub = m.tracks[0].properties.subtitle.as_ref().unwrap();
  assert!(!sub.text_subtitles);
}

#[test]
fn parses_hdmv_textst_clip() {
  let mut blob = b"TextST".to_vec();
  blob.push(0x81); // Dialog Style
  blob.extend_from_slice(&(8u16).to_be_bytes());
  blob.extend_from_slice(&[0u8; 8]);
  let path = write_tempfile(&blob, "textst");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::HdmvTextSt);
}

#[test]
fn parses_vobbtn_clip() {
  let mut blob = b"butonDVD".to_vec();
  blob.extend_from_slice(&[0u8; 8]);
  blob.extend_from_slice(&[0x00, 0x00, 0x01, 0xBF]);
  blob.extend_from_slice(&[0x03, 0xD4, 0x00]);
  blob.extend_from_slice(&[0u8; 16]);
  let path = write_tempfile(&blob, "btn");
  let m = parse(&path, ParseOptions::default()).unwrap();
  let _ = std::fs::remove_file(&path);
  assert_eq!(m.container.format, ContainerFormat::VobButton);
  assert_eq!(m.tracks[0].track_type, TrackType::Buttons);
}
