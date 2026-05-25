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

//! Top-level `Mp4Reader` — implements the `Reader` trait + drives the moov
//! walk + fragment aggregation.

use std::collections::HashMap;

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::reader::Reader;

use super::atom;
use super::ftyp::{self, FileType};
use super::moov::{self, MoovBuilder};

#[derive(Debug, Default, Clone, Copy)]
pub struct Mp4Reader;

impl Reader for Mp4Reader {
  fn name(&self) -> &'static str {
    "mp4"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; 8];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 8 {
      return Ok(false);
    }
    // Recognise the top-level atoms a file may start with.  PARSER-163 adds
    // `pdin` and `mfra` to match mkvtoolnix's `s_top_level_atoms`.
    let kind = &head[4..8];
    Ok(matches!(
      kind,
      b"ftyp" | b"pdin" | b"moov" | b"moof" | b"mfra" | b"mdat" | b"pnot" | b"styp" | b"sidx" | b"free" | b"skip" | b"wide"
    ))
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let mut filetype: Option<FileType> = None;
    let mut moov_builder = MoovBuilder::default();
    let mut have_moov = false;
    let mut have_mdat = false;
    let mut is_fragmented = false;
    let mut fragment_counts: HashMap<u32, u32> = HashMap::new();

    let stream_end = src.length();
    src.seek_to(0)?;

    // No fixed iteration cap (PARSER-048): mkvtoolnix scans every top-level
    // atom until EOF. We guard only against a box that fails to advance the
    // cursor, which would otherwise loop forever.
    loop {
      deadline.check("mp4::read_headers")?;
      if let Some(end) = stream_end {
        if src.position() >= end {
          break;
        }
      }
      let box_start = src.position();
      let header = match atom::read_box_header(src) {
        Ok(h) => h,
        Err(ParseError::UnexpectedEof { .. }) => break,
        // PARSER-076: mkvtoolnix's `resync_to_top_level_atom` scans
        // forward for the next known FOURCC when the current header is
        // malformed.  Mirror that here for `Malformed` outcomes — the
        // alternative would be aborting parse on first bad atom.
        Err(ParseError::Malformed { .. }) => {
          if !resync_to_top_level_atom(src, stream_end, deadline)? {
            break;
          }
          continue;
        }
        Err(e) => return Err(e),
      };

      match &header.kind.0 {
        b"ftyp" => {
          let ft = ftyp::parse(src, &header)?;
          out.container.format = ft.classify();
          out.container.properties.major_brand = Some(ft.major_brand.clone());
          out.container.properties.compatible_brands = ft.compatible_brands.clone();
          filetype = Some(ft);
        }
        b"moov" => {
          if !have_moov {
            moov::parse(src, &header, deadline, &mut moov_builder, out)?;
            have_moov = true;
          }
          atom::skip_payload(src, &header)?;
        }
        b"moof" => {
          is_fragmented = true;
          let summary = super::fragments::parse_moof(src, &header, deadline)?;
          for run in summary.track_runs {
            *fragment_counts.entry(run.track_id).or_insert(0) += run.sample_count;
          }
          atom::skip_payload(src, &header)?;
        }
        b"mdat" => {
          have_mdat = true;
          atom::skip_payload(src, &header)?;
        }
        // PARSER-163: `pdin` (progressive download info) and `mfra` (movie
        // fragment random access) are valid top-level atoms — skip past them
        // rather than treating them as unknown junk.
        b"free" | b"skip" | b"wide" | b"pnot" | b"sidx" | b"pdin" | b"mfra" => {
          atom::skip_payload(src, &header)?;
        }
        b"meta" => {
          super::meta::udta::parse_meta(src, &header, deadline, out)?;
          atom::skip_payload(src, &header)?;
        }
        b"uuid" => {
          atom::skip_payload(src, &header)?;
        }
        _ => {
          // PARSER-076: unknown box.  If the FOURCC isn't
          // human-readable, treat it as junk and try to resync to
          // the next known top-level atom instead of skipping past
          // a potentially-bogus declared size.  Mirrors mkvtoolnix's
          // `r_qtmp4.cpp:220-265` behaviour.
          let printable = header.kind.0.iter().all(|b| (0x20..=0x7E).contains(b));
          if !printable {
            src.seek_to(box_start)?;
            if !resync_to_top_level_atom(src, stream_end, deadline)? {
              break;
            }
            continue;
          }
          atom::skip_payload(src, &header)?;
        }
      }

      // Guard against non-advancing boxes (size-0 that isn't to-EOF, etc.).
      if src.position() <= box_start {
        break;
      }
    }

    if !have_moov {
      return Err(ParseError::Malformed {
        format: "mp4",
        offset: 0,
        reason: "no moov box found".to_string(),
      });
    }
    if !have_mdat {
      // mkvtoolnix's r_qtmp4 errors with "No movie data found." when no
      // mdat atom is present (PARSER-040).
      return Err(ParseError::Malformed {
        format: "mp4",
        offset: 0,
        reason: "no mdat box found".to_string(),
      });
    }

    // If ftyp was absent (legacy QuickTime), default container.format to Mp4.
    if filetype.is_none() && out.container.format == ContainerFormat::Unknown {
      out.container.format = ContainerFormat::Mp4;
    }

    out.container.recognized = true;
    out.container.supported = true;

    super::identify::finalise(moov_builder, is_fragmented, fragment_counts, out);
    Ok(())
  }
}

/// Recognised top-level atom signatures for resync.  PARSER-163 adds `pdin`
/// and `mfra` to match `r_qtmp4.cpp:222`'s `s_top_level_atoms`
/// `{ ftyp, pdin, moov, moof, mfra, mdat, free, skip }`.  `meta` / `uuid` /
/// `pnot` / `sidx` / `styp` / `wide` are kept as additional valid box types
/// our dispatch loop also handles.
const RESYNC_KNOWN_ATOMS: &[&[u8; 4]] = &[
  b"ftyp", b"pdin", b"moov", b"moof", b"mfra", b"mdat", b"free", b"skip", b"meta", b"wide", b"pnot", b"sidx",
  b"styp", b"uuid",
];

/// Largest declared atom size accepted during resync when the stream length is
/// unknown — guards against locking onto a coincidental match with a bogus
/// multi-gigabyte size.
const RESYNC_MAX_ATOM_SIZE: u64 = 256 * 1024 * 1024;

/// PARSER-076 / PARSER-163: port of `r_qtmp4.cpp::resync_to_top_level_atom`.
/// Scan forward from the current cursor for the next byte sequence that looks
/// like a recognised top-level atom signature, testing each `(size, fourcc)`
/// candidate: the size must be the size-0 (to-EOF) sentinel, the size-1
/// (64-bit) form, or `8..=stream_end - pos` so the atom fits inside the file
/// (`r_qtmp4.cpp:228` checks `pos + size <= m_size`).
///
/// PARSER-163: unlike the previous 64 KiB-capped single read, we scan all the
/// way to EOF — mkvtoolnix's `shift_read` loop never gives up early — but in
/// bounded, overlapping windows so memory stays flat and the parse deadline is
/// honoured.  On a match we leave the cursor at the start of the recovered
/// atom so the outer loop's next `read_box_header` succeeds.  The recovered
/// position is always strictly past `start`, guaranteeing forward progress
/// (a malformed atom at `start` can never re-lock onto itself).
fn resync_to_top_level_atom(src: &mut FileSource, stream_end: Option<u64>, deadline: &Deadline) -> Result<bool, ParseError> {
  const CHUNK: usize = 64 * 1024;
  // Overlap so an 8-byte (size + fourcc) signature straddling a chunk boundary
  // is still tested in the following window.
  const OVERLAP: usize = 7;

  let start = src.position();
  let mut chunk_pos = start; // absolute offset of the next chunk to read
  let mut carry: Vec<u8> = Vec::new(); // trailing OVERLAP bytes of the prior window

  loop {
    deadline.check("mp4::resync")?;
    src.seek_to(chunk_pos)?;
    let mut buf = vec![0u8; CHUNK];
    let read = src.read_at_most(&mut buf)?;
    if read == 0 {
      break;
    }
    let window_start = chunk_pos - carry.len() as u64;
    let mut window = std::mem::take(&mut carry);
    window.extend_from_slice(&buf[..read]);

    if window.len() >= 8 {
      for offset in 0..=window.len() - 8 {
        let kind: [u8; 4] = [
          window[offset + 4],
          window[offset + 5],
          window[offset + 6],
          window[offset + 7],
        ];
        if !RESYNC_KNOWN_ATOMS.iter().any(|k| **k == kind) {
          continue;
        }
        let atom_abs = window_start + offset as u64;
        if atom_abs <= start {
          continue; // guarantee progress past the failing atom
        }
        let size = u32::from_be_bytes([window[offset], window[offset + 1], window[offset + 2], window[offset + 3]]) as u64;
        if resync_size_plausible(size, atom_abs, stream_end) {
          src.seek_to(atom_abs)?;
          return Ok(true);
        }
      }
    }

    chunk_pos += read as u64;
    if let Some(end) = stream_end {
      if chunk_pos >= end {
        break;
      }
    }
    if read < CHUNK {
      break; // short read ⇒ EOF
    }
    // Carry the last OVERLAP bytes forward for boundary-spanning signatures.
    let keep = window.len().min(OVERLAP);
    carry = window[window.len() - keep..].to_vec();
  }
  Ok(false)
}

/// True when a candidate atom's declared `size` is structurally plausible at
/// `atom_abs`: the to-EOF/64-bit sentinels, or a 32-bit size that fits inside
/// the file (or under a sanity cap when the length is unknown).
fn resync_size_plausible(size: u64, atom_abs: u64, stream_end: Option<u64>) -> bool {
  match size {
    // Size 0 (to-EOF) and size 1 (64-bit large size) are validated by the
    // outer read_box_header; accept them as plausible candidates.
    0 | 1 => true,
    n if n < 8 => false,
    n => match stream_end {
      Some(end) => atom_abs.saturating_add(n) <= end,
      None => n <= RESYNC_MAX_ATOM_SIZE,
    },
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::model::track::TrackType;
  use crate::media_metadata::mp4::atom::encode_box;
  use crate::media_metadata::mp4::moov::hdlr::build_hdlr_payload;
  use crate::media_metadata::mp4::moov::mdhd::build_mdhd_payload_v0;
  use crate::media_metadata::mp4::moov::mvhd::build_mvhd_payload_v0;
  use crate::media_metadata::mp4::moov::stbl::stsd::{
    build_audio_sample_entry_v0, build_stsd_payload, build_video_sample_entry,
  };
  use crate::media_metadata::mp4::moov::tkhd::build_tkhd_payload_v0;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_video_trak(track_id: u32, codec: &[u8; 4], lang: &str, width: u16, height: u16) -> Vec<u8> {
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(track_id, width, height));
    let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(48000, 1024, lang));
    let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"vide", "VideoHandler"));
    let entry = build_video_sample_entry(codec, width, height, 24, &[]);
    let stsd = encode_box(b"stsd", &build_stsd_payload(&[entry]));
    let stbl = encode_box(b"stbl", &stsd);
    let minf = encode_box(b"minf", &stbl);
    let mut mdia = mdhd;
    mdia.extend(hdlr);
    mdia.extend(minf);
    let mdia = encode_box(b"mdia", &mdia);
    let mut trak = tkhd;
    trak.extend(mdia);
    encode_box(b"trak", &trak)
  }

  fn build_audio_trak(track_id: u32, codec: &[u8; 4], lang: &str, sample_rate: u32, channels: u16) -> Vec<u8> {
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(track_id, 0, 0));
    let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(sample_rate, 0, lang));
    let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"soun", "SoundHandler"));
    // mp4a needs an esds with an AAC AudioSpecificConfig to survive PARSER-150
    // filtering, matching how real AAC-in-MP4 files are laid out.
    let esds = encode_box(
      b"esds",
      &crate::media_metadata::mp4::codec_specific::esds::build_esds_payload(0x40, &[0x12, 0x10]),
    );
    let entry = build_audio_sample_entry_v0(codec, channels, 16, sample_rate, &esds);
    let stsd = encode_box(b"stsd", &build_stsd_payload(&[entry]));
    let stbl = encode_box(b"stbl", &stsd);
    let minf = encode_box(b"minf", &stbl);
    let mut mdia = mdhd;
    mdia.extend(hdlr);
    mdia.extend(minf);
    let mdia = encode_box(b"mdia", &mdia);
    let mut trak = tkhd;
    trak.extend(mdia);
    encode_box(b"trak", &trak)
  }

  fn build_minimal_mp4(major_brand: &[u8; 4], traks: Vec<Vec<u8>>) -> Vec<u8> {
    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(major_brand);
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"isom");
    let ftyp = encode_box(b"ftyp", &ftyp_payload);

    let mvhd = encode_box(b"mvhd", &build_mvhd_payload_v0(1000, 60_000, (traks.len() + 1) as u32));
    let mut moov_payload = mvhd;
    for t in traks {
      moov_payload.extend(t);
    }
    let moov = encode_box(b"moov", &moov_payload);
    let mdat = encode_box(b"mdat", &[0u8; 4]);

    let mut bytes = ftyp;
    bytes.extend(moov);
    bytes.extend(mdat);
    bytes
  }

  #[test]
  fn probe_accepts_ftyp_prefix() {
    let bytes = build_minimal_mp4(b"isom", vec![]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(Mp4Reader.probe(&mut s).unwrap());
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn probe_rejects_non_iso_bmff_files() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"matroska_data!!".to_vec()));
    assert!(!Mp4Reader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_picks_quicktime_brand() {
    let trak = build_video_trak(1, b"avc1", "eng", 1920, 1080);
    let bytes = build_minimal_mp4(b"qt  ", vec![trak]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mov", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::QuickTime);
  }

  #[test]
  fn read_headers_extracts_video_and_audio_tracks() {
    let video = build_video_trak(1, b"avc1", "eng", 1920, 1080);
    let audio = build_audio_trak(2, b"mp4a", "jpn", 48000, 2);
    let bytes = build_minimal_mp4(b"mp42", vec![video, audio]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Mp4);
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert_eq!(out.tracks[1].track_type, TrackType::Audio);
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
    let a = out.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.channels, Some(2));
    // Language pipeline
    assert_eq!(
      out.tracks[0].properties.common.language.as_ref().unwrap().iso639_2,
      "eng"
    );
  }

  // ---- PARSER-040: mdat required ---------------------------------------

  #[test]
  fn read_headers_rejects_files_without_mdat() {
    // ftyp + moov (with a track) but NO mdat.
    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(b"mp42");
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"isom");
    let ftyp = encode_box(b"ftyp", &ftyp_payload);
    let trak = build_video_trak(1, b"avc1", "eng", 320, 240);
    let mvhd = encode_box(b"mvhd", &build_mvhd_payload_v0(1000, 60_000, 2));
    let mut moov_payload = mvhd;
    moov_payload.extend(trak);
    let moov = encode_box(b"moov", &moov_payload);
    let mut bytes = ftyp;
    bytes.extend(moov);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    let err = Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  // ---- PARSER-041: compressed (cmov) movie box -------------------------

  #[test]
  fn cmov_compressed_moov_is_decompressed() {
    use crate::media_metadata::mp4::moov::mvhd::build_mvhd_payload_v0;
    use std::io::Write;
    // Build the *real* moov that lives inside the compressed cmvd.
    let trak = build_video_trak(7, b"avc1", "eng", 1280, 720);
    let mvhd = encode_box(b"mvhd", &build_mvhd_payload_v0(1000, 30_000, 8));
    let mut inner_payload = mvhd;
    inner_payload.extend(trak);
    let inner_moov = encode_box(b"moov", &inner_payload);

    // zlib-compress the moov atom; cmvd = uncompressed_size(u32) + zlib data.
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&inner_moov).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut cmvd_payload = (inner_moov.len() as u32).to_be_bytes().to_vec();
    cmvd_payload.extend(compressed);
    let dcom = encode_box(b"dcom", b"zlib");
    let cmvd = encode_box(b"cmvd", &cmvd_payload);
    let mut cmov_payload = dcom;
    cmov_payload.extend(cmvd);
    let cmov = encode_box(b"cmov", &cmov_payload);
    let outer_moov = encode_box(b"moov", &cmov);

    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(b"qt  ");
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"qt  ");
    let ftyp = encode_box(b"ftyp", &ftyp_payload);
    let mdat = encode_box(b"mdat", &[0u8; 4]);
    let mut bytes = ftyp;
    bytes.extend(outer_moov);
    bytes.extend(mdat);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mov", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(
      out.tracks[0]
        .properties
        .video
        .as_ref()
        .unwrap()
        .pixel_dimensions
        .unwrap()
        .width,
      1280
    );
  }

  // ---- PARSER-043: esds object type refines the codec ------------------

  #[test]
  fn esds_mp3_object_type_reports_mp3() {
    use crate::media_metadata::mp4::codec_specific::esds::build_esds_payload;
    use crate::media_metadata::mp4::moov::stbl::stsd::build_audio_sample_entry_v0;
    // mp4a sample entry carrying an esds with objectType 0x6B (MP3).
    let esds = encode_box(b"esds", &build_esds_payload(0x6B, &[]));
    let entry = build_audio_sample_entry_v0(b"mp4a", 2, 16, 48_000, &esds);
    let stsd = encode_box(
      b"stsd",
      &crate::media_metadata::mp4::moov::stbl::stsd::build_stsd_payload(&[entry]),
    );
    let stbl = encode_box(b"stbl", &stsd);
    let minf = encode_box(b"minf", &stbl);
    let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(48_000, 0, "eng"));
    let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"soun", "Sound"));
    let mut mdia = mdhd;
    mdia.extend(hdlr);
    mdia.extend(minf);
    let mdia = encode_box(b"mdia", &mdia);
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(1, 0, 0));
    let mut trak = tkhd;
    trak.extend(mdia);
    let trak = encode_box(b"trak", &trak);
    let bytes = build_minimal_mp4(b"mp42", vec![trak]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks[0].codec.id, "A_MPEG/L3");
    assert_eq!(out.tracks[0].codec.name.as_deref(), Some("MP3"));
  }

  #[test]
  fn read_headers_rejects_files_without_moov() {
    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(b"isom");
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"isom");
    let bytes = encode_box(b"ftyp", &ftyp_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    let err = Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  #[test]
  fn duplicate_moov_uses_first_only() {
    let trak = build_video_trak(1, b"avc1", "eng", 640, 480);
    let mut bytes = build_minimal_mp4(b"mp42", vec![trak.clone()]);
    // Append a second moov with different track count — must be ignored.
    let trak2 = build_video_trak(1, b"avc1", "eng", 1280, 720);
    let mvhd = encode_box(b"mvhd", &build_mvhd_payload_v0(1000, 60_000, 2));
    let mut moov_payload = mvhd;
    moov_payload.extend(trak2);
    bytes.extend(encode_box(b"moov", &moov_payload));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 640);
  }

  #[test]
  fn fragmented_flag_set_when_moof_present() {
    let trak = build_video_trak(1, b"avc1", "eng", 320, 240);
    let mut bytes = build_minimal_mp4(b"mp42", vec![trak]);
    // Append a moof
    let tfhd = encode_box(b"tfhd", &{
      let mut p = vec![0u8; 4];
      p.extend_from_slice(&1u32.to_be_bytes());
      p
    });
    let trun = encode_box(b"trun", &{
      let mut p = vec![0u8; 4];
      p.extend_from_slice(&30u32.to_be_bytes());
      p
    });
    let mut traf = tfhd;
    traf.extend(trun);
    let traf = encode_box(b"traf", &traf);
    let moof = encode_box(b"moof", &traf);
    bytes.extend(moof);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.properties.is_fragmented, Some(true));
    assert_eq!(out.tracks[0].properties.common.num_index_entries, Some(30),);
  }

  #[test]
  fn major_brand_and_compatible_brands_stored() {
    let bytes = build_minimal_mp4(b"mp42", vec![]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.properties.major_brand.as_deref(), Some("mp42"));
    assert!(out.container.properties.compatible_brands.contains(&"isom".to_string()));
  }

  #[test]
  fn movie_duration_derived_from_mvhd() {
    let bytes = build_minimal_mp4(b"mp42", vec![]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    // timescale=1000 + duration=60_000 → 60 s = 60_000_000_000 ns
    assert_eq!(out.container.properties.duration.unwrap().ns, 60_000_000_000);
    assert_eq!(out.container.properties.movie_timescale, Some(1000));
  }

  // ---- PARSER-076: resync to top-level atom -----------------------------

  #[test]
  fn malformed_top_level_box_resyncs_to_next_known_atom() {
    let trak = build_video_trak(1, b"avc1", "eng", 320, 240);
    let bytes = build_minimal_mp4(b"mp42", vec![trak]);
    // Prepend 4 bytes of junk so the first `read_box_header` reports the
    // size as `JUNK` / 0x4A554E4B  (≈ 1.2 GiB) — out of range.  The
    // resync helper should scan past the junk and land on `ftyp`.
    let mut prefixed = b"JUNK".to_vec();
    prefixed.extend(bytes);
    let mut s = FileSource::from_reader_for_test(Cursor::new(prefixed));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Mp4);
    assert_eq!(out.tracks.len(), 1);
  }

  /// A `trak` with a `hint` handler is classified as a non-track and dropped
  /// by `build_track`, so it is useful for the compact-id test.
  fn build_dropped_trak(track_id: u32) -> Vec<u8> {
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(track_id, 0, 0));
    let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(1000, 0, "und"));
    let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"hint", "HintHandler"));
    let entry = build_video_sample_entry(b"avc1", 16, 16, 24, &[]);
    let stsd = encode_box(b"stsd", &build_stsd_payload(&[entry]));
    let stbl = encode_box(b"stbl", &stsd);
    let minf = encode_box(b"minf", &stbl);
    let mut mdia = mdhd;
    mdia.extend(hdlr);
    mdia.extend(minf);
    let mdia = encode_box(b"mdia", &mdia);
    let mut trak = tkhd;
    trak.extend(mdia);
    encode_box(b"trak", &trak)
  }

  #[test]
  fn dropped_leading_track_does_not_shift_ids() {
    // PARSER-161: a dropped (hint-handler) trak ahead of a real video trak
    // must not push the video track's id off 0.
    let dropped = build_dropped_trak(1);
    let video = build_video_trak(2, b"avc1", "eng", 320, 240);
    let bytes = build_minimal_mp4(b"mp42", vec![dropped, video]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].id, 0, "compact id assignment");
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
  }

  #[test]
  fn file_starting_with_pdin_is_recognised() {
    // PARSER-163: a leading `pdin` atom must be accepted and skipped.
    let trak = build_video_trak(1, b"avc1", "eng", 320, 240);
    let mp4 = build_minimal_mp4(b"mp42", vec![trak]);
    let mut bytes = encode_box(b"pdin", &[0u8; 12]);
    bytes.extend(mp4);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(Mp4Reader.probe(&mut s).unwrap());
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
  }

  #[test]
  fn resync_scans_past_more_than_64kib_of_leading_junk() {
    // PARSER-163: over 64 KiB of leading junk must not defeat the resync —
    // mkvtoolnix scans to EOF, not a fixed 64 KiB window.
    let trak = build_video_trak(1, b"avc1", "eng", 320, 240);
    let mp4 = build_minimal_mp4(b"mp42", vec![trak]);
    let mut bytes = vec![0u8; 70 * 1024]; // junk larger than the old cap
    bytes.extend(mp4);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
  }

  #[test]
  fn empty_track_set_still_succeeds() {
    let bytes = build_minimal_mp4(b"mp42", vec![]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.mp4", 0);
    Mp4Reader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.tracks.is_empty());
    assert_eq!(out.container.recognized, true);
    assert_eq!(out.container.supported, true);
  }
}
