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

//! `MatroskaReader` — top-level `Reader` impl + `read_headers_internal`
//! pipeline.  Port of `mkvtoolnix/src/input/r_matroska.cpp:1583-1779`.
//!
//! Pipeline (header-only — no clusters, no extractor):
//! 1. Read the EBML head — DocType must be `matroska` or `webm`.
//! 2. Find the next Segment element.
//! 3. Walk Segment's L1 children; for each, either:
//!    - dispatch immediately (Info / Tracks / Attachments / Chapters / Tags),
//!    - or queue the position via SeekHead for a later seek.
//! 4. Stop at the first Cluster or EOF.
//! 5. Run the deferred L1 elements in order (Info → Tracks → Attachments →
//!    Tags → Chapters).
//!
//! Memory shape mirrors mkvtoolnix's `m_deferred_l1_positions` /
//! `m_handled_l1_positions` so the cross-check is unambiguous.

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::reader::Reader;

use super::attachments;
use super::chapters;
use super::ebml::{self, ChildAction, ElementHeader};
use super::identify;
use super::ids;
use super::info;
use super::seek_head;
use super::tags;
use super::tracks;

/// Soft cap on the segment-info / tracks / attachments element size. We never
/// allocate more than this for a single EBML element payload — corrupt
/// containers cannot drive an OOM via a 16-EiB size VINT.
/// The matroska reader.  Zero-sized — every parse owns its own `FileSource`,
/// `Deadline`, and `MediaMetadata`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MatroskaReader;

/// The five deferred Level-1 element buckets mkvmerge collects from SeekHead.
/// We mirror the enum so the cross-check is trivial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DeferredL1 {
  Info,
  Tracks,
  Attachments,
  Chapters,
  Tags,
  /// A SeekHead that another SeekHead points at (chained SeekHeads,
  /// PARSER-038).
  SeekHead,
}

#[derive(Debug, Default)]
pub(crate) struct DeferredL1Positions {
  inner: HashMap<DeferredL1, Vec<u64>>,
  handled: HashMap<DeferredL1, Vec<u64>>,
}

impl DeferredL1Positions {
  pub(crate) fn push(&mut self, kind: DeferredL1, pos: u64) {
    self.inner.entry(kind).or_default().push(pos);
  }

  pub(crate) fn take(&mut self, kind: DeferredL1) -> Vec<u64> {
    self.inner.remove(&kind).unwrap_or_default()
  }

  pub(crate) fn mark_handled(&mut self, kind: DeferredL1, pos: u64) {
    self.handled.entry(kind).or_default().push(pos);
  }

  pub(crate) fn has_been_handled(&self, kind: DeferredL1, pos: u64) -> bool {
    self.handled.get(&kind).map(|v| v.contains(&pos)).unwrap_or(false)
  }
}

impl Reader for MatroskaReader {
  fn name(&self) -> &'static str {
    "matroska"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; 4];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 4 {
      return Ok(false);
    }
    Ok(head == [0x1A, 0x45, 0xDF, 0xA3])
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    // EBML head
    let head = ebml::read_element_header(src)?;
    if head.id != ids::EBML {
      return Err(ParseError::Malformed {
        format: "matroska",
        offset: head.start,
        reason: format!("expected EBML head ({:#x}), got {:#x}", ids::EBML, head.id),
      });
    }
    let doc_type = read_ebml_head_doc_type(src, &head, deadline)?;
    let format = match doc_type.as_deref() {
      Some("webm") => ContainerFormat::WebM,
      _ => ContainerFormat::Matroska,
    };

    // Skip remainder of EBML head if read_ebml_head_doc_type didn't consume it.
    if let Some(end) = head.end() {
      if src.position() < end {
        src.seek_to(end)?;
      }
    }

    // Find the first Segment, tolerating leading Void / CRC32 padding.
    let segment = locate_segment(src, deadline)?;

    out.container.recognized = true;
    out.container.supported = true;
    out.container.format = format;

    // Walk Segment's L1 children. Hand each one off; defer or dispatch.
    let mut deferred = DeferredL1Positions::default();
    walk_segment_l1(src, &segment, deadline, &mut deferred, out)?;

    // Follow chained SeekHeads first (PARSER-038): a SeekHead may point at
    // another SeekHead. Drain the bucket until no new unhandled positions
    // remain (the handled-set guards against cycles).
    loop {
      let positions = deferred.take(DeferredL1::SeekHead);
      if positions.is_empty() {
        break;
      }
      let mut made_progress = false;
      for pos in positions {
        if deferred.has_been_handled(DeferredL1::SeekHead, pos) {
          continue;
        }
        deferred.mark_handled(DeferredL1::SeekHead, pos);
        made_progress = true;
        if src.seek_to(pos).is_err() {
          continue;
        }
        if let Ok(h) = ebml::read_element_header(src) {
          if h.id == ids::SEEK_HEAD {
            // Tolerate errors from a referenced SeekHead.
            let _ = seek_head::collect_deferred(src, &h, deadline, &mut deferred, segment.payload_start());
          }
        }
      }
      if !made_progress {
        break;
      }
    }

    // Process deferred elements in mkvmerge order: Info, Tracks,
    // Attachments, Tags, Chapters.
    for kind in [
      DeferredL1::Info,
      DeferredL1::Tracks,
      DeferredL1::Attachments,
      DeferredL1::Tags,
      DeferredL1::Chapters,
    ] {
      for pos in deferred.take(kind) {
        if deferred.has_been_handled(kind, pos) {
          continue;
        }
        deferred.mark_handled(kind, pos);
        process_deferred(src, kind, pos, deadline, out, &mut deferred)?;
      }
    }

    identify::finalise(out);
    Ok(())
  }
}

/// Decode `EBMLHead` payload to extract `DocType`.  Returns `None` if not
/// present (parser tolerates it for forward compatibility).
fn read_ebml_head_doc_type(
  src: &mut FileSource,
  head: &ElementHeader,
  deadline: &Deadline,
) -> Result<Option<String>, ParseError> {
  let mut doc_type = None;
  ebml::walk_children(src, head, "matroska::ebml_head", deadline, |src, child| {
    match child.id {
      ids::DOC_TYPE => {
        let s = ebml::read_string(src, child, 64)?;
        doc_type = Some(s);
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    }
  })?;
  Ok(doc_type)
}

/// Scan forward for the next Segment header, skipping Void / CRC32 padding.
fn locate_segment(src: &mut FileSource, deadline: &Deadline) -> Result<ElementHeader, ParseError> {
  loop {
    deadline.check("matroska::locate_segment")?;
    let header = ebml::read_element_header(src)?;
    match header.id {
      ids::SEGMENT => return Ok(header),
      ids::VOID | ids::CRC32 => {
        ebml::skip_payload(src, &header)?;
      }
      _ => {
        return Err(ParseError::Malformed {
          format: "matroska",
          offset: header.start,
          reason: format!("expected Segment, found unexpected L0 element {:#x}", header.id),
        });
      }
    }
  }
}

/// Walk the Segment's L1 children once, classifying each.  Stops at Cluster
/// or at the Segment payload boundary — we never descend into Clusters.
fn walk_segment_l1(
  src: &mut FileSource,
  segment: &ElementHeader,
  deadline: &Deadline,
  deferred: &mut DeferredL1Positions,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  let payload_start = segment.payload_start();
  src.seek_to(payload_start)?;
  let segment_end = segment.end();
  let stream_end = src.length();

  loop {
    deadline.check("matroska::walk_segment_l1")?;
    // Compute remaining bytes in the segment payload
    let pos = src.position();
    match (segment_end, stream_end) {
      (Some(end), _) if pos >= end => break,
      (_, Some(end)) if pos >= end => break,
      _ => {}
    }

    let header = match ebml::read_element_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };

    match header.id {
      ids::SEEK_HEAD => {
        seek_head::collect_deferred(src, &header, deadline, deferred, payload_start)?;
      }
      ids::INFO => {
        deferred.push(DeferredL1::Info, header.start);
        ebml::skip_payload(src, &header)?;
      }
      ids::TRACKS => {
        deferred.push(DeferredL1::Tracks, header.start);
        ebml::skip_payload(src, &header)?;
      }
      ids::ATTACHMENTS => {
        deferred.push(DeferredL1::Attachments, header.start);
        ebml::skip_payload(src, &header)?;
      }
      ids::CHAPTERS => {
        deferred.push(DeferredL1::Chapters, header.start);
        ebml::skip_payload(src, &header)?;
      }
      ids::TAGS => {
        deferred.push(DeferredL1::Tags, header.start);
        ebml::skip_payload(src, &header)?;
      }
      ids::CLUSTER => {
        // First cluster = header-only parse complete. Save position
        // for the minimum-timestamp pass (we don't do that yet).
        break;
      }
      ids::VOID | ids::CRC32 => {
        ebml::skip_payload(src, &header)?;
      }
      _ => {
        // Unknown L1 element — skip per mkvmerge behaviour.
        ebml::skip_payload(src, &header)?;
      }
    }
    // Defensive: if a child reported unknown size we cannot continue.
    if header.size.is_none() && header.id != ids::CLUSTER && header.id != ids::SEGMENT {
      break;
    }
  }

  let _ = out; // unused for now; warnings vec may be populated in later phases
  Ok(())
}

fn process_deferred(
  src: &mut FileSource,
  kind: DeferredL1,
  pos: u64,
  deadline: &Deadline,
  out: &mut MediaMetadata,
  deferred: &mut DeferredL1Positions,
) -> Result<(), ParseError> {
  src.seek_to(pos)?;
  let header = ebml::read_element_header(src)?;
  // No blanket size rejection here (PARSER-039): the per-element walkers seek
  // past large binary leaves (e.g. KaxFileData) rather than buffering them,
  // so a multi-gigabyte Attachments element is walked, not rejected.
  match kind {
    DeferredL1::Info => {
      info::parse(src, &header, deadline, out)?;
    }
    DeferredL1::Tracks => {
      tracks::parse(src, &header, deadline, out)?;
    }
    DeferredL1::Attachments => {
      attachments::parse(src, &header, deadline, out)?;
    }
    DeferredL1::Chapters => {
      chapters::parse(src, &header, deadline, out)?;
    }
    DeferredL1::Tags => {
      tags::parse(src, &header, deadline, out)?;
    }
    // SeekHeads are drained in read_headers before this loop runs.
    DeferredL1::SeekHead => {}
  }
  let _ = deferred;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{
    encode_element, encode_element_string, encode_element_uint, encode_id, encode_size,
  };
  use std::io::Cursor;

  fn src(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  const SEEKHEAD_ID_BYTES: [u8; 4] = [0x11, 0x4D, 0x9B, 0x74];
  const TRACKS_ID_BYTES: [u8; 4] = [0x16, 0x54, 0xAE, 0x6B];

  fn seek_entry(target_id_bytes: &[u8], pos: u64) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(encode_element(ids::SEEK_ID, 2, target_id_bytes));
    p.extend(encode_element_uint(ids::SEEK_POSITION, 2, pos));
    encode_element(ids::SEEK, 2, &p)
  }

  fn seek_head_with(entry: Vec<u8>) -> Vec<u8> {
    encode_element(ids::SEEK_HEAD, 4, &entry)
  }

  fn minimal_audio_tracks() -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(encode_element_uint(ids::TRACK_NUMBER, 1, 1));
    t.extend(encode_element_uint(ids::TRACK_TYPE, 1, 2)); // audio
    t.extend(encode_element_string(ids::CODEC_ID, 1, "A_AAC"));
    let entry = encode_element(ids::TRACK_ENTRY, 1, &t);
    encode_element(ids::TRACKS, 4, &entry)
  }

  fn ebml_head() -> Vec<u8> {
    encode_element(ids::EBML, 4, &encode_element_string(ids::DOC_TYPE, 2, "matroska"))
  }

  // ---- PARSER-038: chained SeekHeads ------------------------------------

  #[test]
  fn chained_seekheads_reach_tracks() {
    // Segment payload layout: SeekHead1 → SeekHead2 → Tracks. SeekHead1
    // points at SeekHead2 (segment-relative), SeekHead2 points at Tracks.
    let sh1_probe = seek_head_with(seek_entry(&SEEKHEAD_ID_BYTES, 0));
    let sh2_probe = seek_head_with(seek_entry(&TRACKS_ID_BYTES, 0));
    let sh1_len = sh1_probe.len() as u64;
    let sh2_len = sh2_probe.len() as u64;
    let sh2_off = sh1_len;
    let tracks_off = sh1_len + sh2_len;

    let sh1 = seek_head_with(seek_entry(&SEEKHEAD_ID_BYTES, sh2_off));
    let sh2 = seek_head_with(seek_entry(&TRACKS_ID_BYTES, tracks_off));
    // Position values are small (<128) so the VINT widths — and thus the
    // element lengths — are stable between the probe and final builds.
    assert_eq!(sh1.len() as u64, sh1_len);
    assert_eq!(sh2.len() as u64, sh2_len);

    let mut payload = Vec::new();
    payload.extend(sh1);
    payload.extend(sh2);
    payload.extend(minimal_audio_tracks());
    let segment = encode_element(ids::SEGMENT, 4, &payload);

    let mut bytes = ebml_head();
    bytes.extend(segment);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1, "Tracks reached only via chained SeekHeads");
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
  }

  // ---- PARSER-039: oversized top-level element is not rejected ----------

  #[test]
  fn oversized_attachments_element_is_not_rejected() {
    const HUGE: u64 = 100_000_000;
    // FileData header declaring 100 MB but with no actual payload bytes.
    let filename = encode_element_string(ids::FILE_NAME, 2, "big.bin");
    let mut filedata = encode_id(ids::FILE_DATA, 2);
    filedata.extend(encode_size(HUGE));

    // AttachedFile: declared payload includes the 100 MB FileData payload,
    // but only the header bytes are actually present.
    let af_declared = filename.len() as u64 + filedata.len() as u64 + HUGE;
    let mut attached = encode_id(ids::ATTACHED_FILE, 2);
    attached.extend(encode_size(af_declared));
    let af_header_len = attached.len() as u64;
    attached.extend(&filename);
    attached.extend(&filedata);

    // Attachments: declared payload spans the whole (oversized) AttachedFile;
    // its declared size exceeds the old 64 MiB rejection cap.
    let attachments_declared = af_header_len + af_declared;
    let mut attachments = encode_id(ids::ATTACHMENTS, 4);
    attachments.extend(encode_size(attachments_declared));
    attachments.extend(attached);

    let segment = encode_element(ids::SEGMENT, 4, &attachments);
    let mut bytes = ebml_head();
    bytes.extend(segment);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    // Previously returned Err(OversizedElement); now it walks in and records
    // the attachment.
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].size, 100_000_000);
  }

  fn build_minimal_matroska(doc_type: &str) -> Vec<u8> {
    // EBML head with DocType
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, doc_type);
    let head = encode_element(ids::EBML, 4, &head_payload);

    // Empty segment
    let segment = encode_element(ids::SEGMENT, 4, &[]);

    let mut out = Vec::new();
    out.extend(head);
    out.extend(segment);
    out
  }

  #[test]
  fn probe_accepts_ebml_signature() {
    let mut s = src(vec![0x1A, 0x45, 0xDF, 0xA3, 0x42, 0x86]);
    assert!(MatroskaReader.probe(&mut s).unwrap());
    // cursor must be rewound
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = src(vec![0x1A, 0x45]);
    assert!(!MatroskaReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_non_matroska_magic() {
    let mut s = src(vec![b'R', b'I', b'F', b'F']);
    assert!(!MatroskaReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_handles_minimal_matroska() {
    let bytes = build_minimal_matroska("matroska");
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Matroska);
    assert!(out.container.recognized);
    assert!(out.container.supported);
    assert_eq!(out.tracks.len(), 0);
    assert_eq!(out.attachments.len(), 0);
  }

  #[test]
  fn read_headers_detects_webm_doc_type() {
    let bytes = build_minimal_matroska("webm");
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.webm", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::WebM);
  }

  #[test]
  fn read_headers_rejects_input_without_ebml_head() {
    // Garbage that happens to begin with a valid 1-byte VINT id
    let mut bytes = vec![0x83, 0x82, 1, 2];
    // Pad to make probe succeed in isolation — but here we skip probe and
    // call read_headers directly to exercise the EBML-head check.
    bytes.extend(vec![0u8; 16]);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("garbage", bytes.len() as u64);
    let err = MatroskaReader
      .read_headers(&mut s, &no_deadline(), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn deferred_l1_helpers_round_trip() {
    let mut d = DeferredL1Positions::default();
    d.push(DeferredL1::Info, 100);
    d.push(DeferredL1::Tracks, 200);
    assert!(!d.has_been_handled(DeferredL1::Info, 100));
    d.mark_handled(DeferredL1::Info, 100);
    assert!(d.has_been_handled(DeferredL1::Info, 100));
    assert!(!d.has_been_handled(DeferredL1::Info, 999));
    let took = d.take(DeferredL1::Info);
    assert_eq!(took, vec![100]);
    // taking again clears
    assert!(d.take(DeferredL1::Info).is_empty());
    // Tracks still present
    assert_eq!(d.take(DeferredL1::Tracks), vec![200]);
  }

  #[test]
  fn read_headers_rejects_l0_other_than_segment() {
    // EBML head + unexpected L0 element (e.g. attachments at L0)
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, "matroska");
    let head = encode_element(ids::EBML, 4, &head_payload);
    let bogus_l0 = encode_element(ids::ATTACHMENTS, 4, &[]);
    let mut bytes = head;
    bytes.extend(bogus_l0);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    let err = MatroskaReader
      .read_headers(&mut s, &no_deadline(), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn read_headers_skips_leading_void_before_segment() {
    // EBML head + Void at L0 + Segment with Info
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, "matroska");
    let head = encode_element(ids::EBML, 4, &head_payload);
    let void = encode_element(ids::VOID, 1, &[0u8; 4]);
    let info_payload = encode_element_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000);
    let info = encode_element(ids::INFO, 4, &info_payload);
    let segment = encode_element(ids::SEGMENT, 4, &info);
    let mut bytes = head;
    bytes.extend(void);
    bytes.extend(segment);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(1_000_000));
  }

  #[test]
  fn read_headers_stops_at_cluster() {
    // EBML head + Segment with Info + Cluster (must stop, not error)
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, "matroska");
    let head = encode_element(ids::EBML, 4, &head_payload);
    let info_payload = encode_element_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000);
    let info = encode_element(ids::INFO, 4, &info_payload);
    let cluster = encode_element(ids::CLUSTER, 4, &[0u8; 16]);
    let mut seg = info;
    seg.extend(cluster);
    let segment = encode_element(ids::SEGMENT, 4, &seg);
    let mut bytes = head;
    bytes.extend(segment);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(1_000_000));
  }

  #[test]
  fn read_headers_skips_unknown_l1_elements() {
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, "matroska");
    let head = encode_element(ids::EBML, 4, &head_payload);
    // Arbitrary unknown L1 id with valid VINT and small payload
    let unknown = encode_element(0xAA, 1, &[0u8; 4]);
    let info_payload = encode_element_uint(ids::TIMESTAMP_SCALE, 3, 2_000_000);
    let info = encode_element(ids::INFO, 4, &info_payload);
    let mut seg = unknown;
    seg.extend(info);
    let segment = encode_element(ids::SEGMENT, 4, &seg);
    let mut bytes = head;
    bytes.extend(segment);
    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(2_000_000));
  }

  #[test]
  fn read_headers_handles_segment_with_void_padding() {
    // Build an Info element with TimestampScale=1_000_000
    let scale = encode_element_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000);
    let info = encode_element(ids::INFO, 4, &scale);
    // Wrap Info inside a Void-padded Segment
    let void = encode_element(ids::VOID, 1, &[0u8; 4]);
    let mut seg_payload = void.clone();
    seg_payload.extend(info);
    let segment = encode_element(ids::SEGMENT, 4, &seg_payload);
    let head_payload = encode_element_string(ids::DOC_TYPE, 2, "matroska");
    let head = encode_element(ids::EBML, 4, &head_payload);
    let mut bytes = head;
    bytes.extend(segment);

    let mut s = src(bytes.clone());
    let mut out = MediaMetadata::new("clip.mkv", bytes.len() as u64);
    MatroskaReader.read_headers(&mut s, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(1_000_000));
  }
}
