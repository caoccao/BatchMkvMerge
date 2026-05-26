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

//! Top-level `AviReader` — drives the RIFF walk.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::reader::Reader;

use super::avih::{self, MainAviHeader};
use super::identify;
use super::odml::{self, OdmlInfo};
use super::riff::{self, ChildAction, ChunkHeader};
use super::strl::{self, AviStreamKind, StreamBuilder};
use super::subtitles;

#[derive(Debug, Default, Clone, Copy)]
pub struct AviReader;

impl Reader for AviReader {
  fn name(&self) -> &'static str {
    "avi"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = [0u8; 12];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 12 {
      return Ok(false);
    }
    // Only a primary `RIFF/AVI ` file is claimed. `AVIX` chunks are OpenDML
    // extension segments, not standalone files (PARSER-061).
    Ok(&head[0..4] == b"RIFF" && &head[8..12] == b"AVI ")
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    src.seek_to(0)?;
    let riff_header = riff::read_chunk_header(src)?;
    if &riff_header.kind != b"RIFF" {
      return Err(ParseError::Malformed {
        format: "avi",
        offset: riff_header.start,
        reason: format!("expected RIFF, got '{}'", riff::fourcc_string(&riff_header.kind)),
      });
    }
    let form_type = riff::read_list_subtype(src)?;
    if &form_type != b"AVI " {
      return Err(ParseError::Malformed {
        format: "avi",
        offset: riff_header.start,
        reason: format!("RIFF form '{}' is not AVI", riff::fourcc_string(&form_type)),
      });
    }
    out.container.format = ContainerFormat::Avi;
    out.container.recognized = true;
    out.container.supported = true;

    let mut avih: Option<MainAviHeader> = None;
    let mut streams: Vec<StreamBuilder> = Vec::new();
    let mut odml_info = OdmlInfo::default();
    let mut found_hdrl = false;
    let mut movi: Option<ChunkHeader> = None;

    // Walk children of the outer RIFF list.  We don't use walk_list_children
    // here because we've already consumed the form_type FOURCC.
    let parent_end = riff_header.payload_end();
    let stream_end = src.length();
    loop {
      deadline.check("avi::reader")?;
      let pos = src.position();
      if pos >= parent_end {
        break;
      }
      if let Some(end) = stream_end {
        if pos >= end {
          break;
        }
        if end - pos < 8 {
          break;
        }
      }
      if parent_end - pos < 8 {
        break;
      }
      let child = match riff::read_chunk_header(src) {
        Ok(h) => h,
        Err(ParseError::UnexpectedEof { .. }) => break,
        Err(e) => return Err(e),
      };

      if child.is_list_container() {
        // Read the LIST sub-type to dispatch.
        let sub = riff::read_list_subtype(src)?;
        // Rewind so the LIST helpers see the sub-type FOURCC at the
        // expected offset.
        src.seek_to(child.payload_start())?;
        match &sub {
          b"hdrl" => {
            found_hdrl = true;
            parse_hdrl(src, &child, deadline, &mut avih, &mut streams, &mut odml_info)?;
          }
          b"odml" => {
            odml_info = odml::parse_odml_list(src, &child, deadline)?;
          }
          b"movi" => {
            // Remember where the movie data lives so we can read the
            // first text chunks for subtitle detection (PARSER-192).
            movi = Some(child);
          }
          _ => {}
        }
        riff::skip_payload_with_pad(src, &child)?;
      } else {
        riff::skip_payload_with_pad(src, &child)?;
      }
    }

    if !found_hdrl {
      return Err(ParseError::Malformed {
        format: "avi",
        offset: 0,
        reason: "no hdrl LIST found".to_string(),
      });
    }

    // PARSER-192: subtitle tracks come from the first GAB2 text chunks in the
    // `movi` list, not from the text stream's strf.  Determine the overall
    // stream index of every text stream (its position inside `hdrl`) so we can
    // match the `NNtx` chunk tags, then read only those first chunks.
    let text_streams: Vec<usize> = streams
      .iter()
      .enumerate()
      .filter(|(_, s)| s.header.as_ref().map(|h| h.kind) == Some(AviStreamKind::Text))
      .map(|(idx, _)| idx)
      .collect();
    let subtitles = match (&movi, text_streams.is_empty()) {
      (Some(movi), false) => subtitles::parse_subtitle_chunks(src, movi, &text_streams, deadline)?,
      _ => Vec::new(),
    };

    // PARSER-241: MPEG-4 Part 2 (DivX/Xvid) AVI carries the pixel aspect ratio
    // only in the elementary bit-stream, so read the first video frame and
    // decode the VOL header's PAR (mkvtoolnix's `extended_identify_mpeg4_l2`).
    let video_frame_par = compute_mpeg4_video_par(src, movi.as_ref(), &streams, deadline)?;

    identify::finalise(avih, streams, odml_info, subtitles, video_frame_par, out);
    Ok(())
  }
}

/// Maximum number of `movi` child-chunk headers scanned looking for the first
/// video frame of the MPEG-4 Part 2 stream.
const MAX_MOVI_FRAME_SCANS: u32 = 4096;
/// Bytes of the first video frame read for VOL-header PAR decoding — the VOL
/// header sits at the very start, so a bounded prefix is enough.
const MAX_VIDEO_FRAME_BYTES: u64 = 256 * 1024;

/// The two `movi` chunk tags for a video stream at index `idx`: `NNdb`
/// (uncompressed) and `NNdc` (compressed).
fn video_chunk_tags(idx: usize) -> [[u8; 4]; 2] {
  let d1 = (idx / 10) as u8 + b'0';
  let d2 = (idx % 10) as u8 + b'0';
  [[d1, d2, b'd', b'b'], [d1, d2, b'd', b'c']]
}

/// Build an ASCII FOURCC string from the 4-byte BITMAPINFOHEADER compression
/// field for codec matching.
fn fourcc_ascii(bytes: &[u8; 4]) -> String {
  bytes.iter().map(|&b| b as char).collect()
}

/// PARSER-241: when the (single) video stream is MPEG-4 Part 2, read its first
/// `movi` frame and extract the VOL-header pixel aspect ratio.  Returns `None`
/// for any non-MPEG-4-P2 stream, a missing `movi`, or a frame without PAR.
fn compute_mpeg4_video_par(
  src: &mut FileSource,
  movi: Option<&ChunkHeader>,
  streams: &[StreamBuilder],
  deadline: &Deadline,
) -> Result<Option<(u32, u32)>, ParseError> {
  let Some(movi) = movi else {
    return Ok(None);
  };
  let Some(video_idx) = streams
    .iter()
    .position(|s| s.header.as_ref().map(|h| h.kind) == Some(AviStreamKind::Video))
  else {
    return Ok(None);
  };
  let bmih = match streams[video_idx].format.as_ref() {
    Some(super::strl::StreamFormat::Video(bmih)) => bmih,
    _ => return Ok(None),
  };
  if !super::mpeg4_par::is_mpeg4_p2(&fourcc_ascii(&bmih.compression)) {
    return Ok(None);
  }
  let Some(frame) = read_first_video_frame(src, movi, video_idx, deadline)? else {
    return Ok(None);
  };
  Ok(super::mpeg4_par::extract_par(&frame))
}

/// Walk the `movi` LIST for the first frame chunk belonging to the video stream
/// at `video_idx`, returning a bounded prefix of its payload.
fn read_first_video_frame(
  src: &mut FileSource,
  movi: &ChunkHeader,
  video_idx: usize,
  deadline: &Deadline,
) -> Result<Option<Vec<u8>>, ParseError> {
  let tags = video_chunk_tags(video_idx);
  let first_child = movi.payload_start() + 4;
  let parent_end = movi.payload_end();
  let stream_end = src.length();
  src.seek_to(first_child)?;
  let mut scans = 0u32;
  while scans < MAX_MOVI_FRAME_SCANS {
    deadline.check("avi::mpeg4_par")?;
    scans += 1;
    let pos = src.position();
    if pos >= parent_end || parent_end - pos < 8 {
      break;
    }
    if let Some(end) = stream_end {
      if pos >= end || end - pos < 8 {
        break;
      }
    }
    let child = match riff::read_chunk_header(src) {
      Ok(h) => h,
      Err(ParseError::UnexpectedEof { .. }) => break,
      Err(e) => return Err(e),
    };
    if child.payload_end() > parent_end {
      break;
    }
    // Descend into `rec ` interleave sub-lists by skipping the LIST sub-type.
    if &child.kind == b"LIST" {
      src.seek_to(child.payload_start() + 4)?;
      continue;
    }
    if tags.iter().any(|t| &child.kind == t) {
      let bytes = riff::read_payload(src, &child, MAX_VIDEO_FRAME_BYTES)?;
      return Ok(Some(bytes));
    }
    riff::skip_payload_with_pad(src, &child)?;
  }
  Ok(None)
}

fn parse_hdrl(
  src: &mut FileSource,
  parent: &ChunkHeader,
  deadline: &Deadline,
  avih: &mut Option<MainAviHeader>,
  streams: &mut Vec<StreamBuilder>,
  odml_info: &mut OdmlInfo,
) -> Result<(), ParseError> {
  riff::walk_list_children(src, parent, "avi::hdrl", deadline, |src, child| match &child.kind {
    b"avih" => {
      *avih = Some(avih::parse(src, child)?);
      Ok(ChildAction::Consumed)
    }
    b"LIST" => {
      // Peek the sub-type: strl (per-stream) or odml (OpenDML header,
      // which conventionally lives inside hdrl — PARSER-058).
      let sub = riff::read_list_subtype(src)?;
      src.seek_to(child.payload_start())?;
      match &sub {
        b"strl" => streams.push(strl::parse_strl(src, child, deadline)?),
        b"odml" => *odml_info = odml::parse_odml_list(src, child, deadline)?,
        _ => {}
      }
      Ok(ChildAction::Skip)
    }
    _ => Ok(ChildAction::Skip),
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::avi::avih::build_avih_payload;
  use crate::media_metadata::avi::riff::{encode_chunk, encode_list};
  use crate::media_metadata::avi::strl::{build_bitmapinfoheader, build_strh_payload, build_waveformatex};
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::model::track::TrackType;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_video_strl(width: u16, height: u16) -> Vec<u8> {
    let strh = encode_chunk(b"strh", &build_strh_payload(b"vids", b"H264", 1001, 24000, 240, 0));
    let strf = encode_chunk(
      b"strf",
      &build_bitmapinfoheader(width as i32, height as i32, 24, b"H264"),
    );
    let mut payload = strh;
    payload.extend(strf);
    encode_list(b"LIST", b"strl", &[payload])
  }

  fn build_audio_strl() -> Vec<u8> {
    let strh = encode_chunk(b"strh", &build_strh_payload(b"auds", b"\0\0\0\0", 1, 48000, 0, 4));
    let strf = encode_chunk(b"strf", &build_waveformatex(0x0055, 2, 48000, 16000, 4, 16, &[]));
    let mut payload = strh;
    payload.extend(strf);
    encode_list(b"LIST", b"strl", &[payload])
  }

  fn build_text_strl() -> Vec<u8> {
    // A text stream carries just a strh — the GAB2 subtitle lives in movi.
    let strh = encode_chunk(b"strh", &build_strh_payload(b"txts", b"DXSA", 1, 1000, 0, 0));
    encode_list(b"LIST", b"strl", &[strh])
  }

  fn gab2_chunk(content: &[u8]) -> Vec<u8> {
    let mut g = b"GAB2\0".to_vec();
    // filename block (id 2)
    g.extend_from_slice(&2u16.to_le_bytes());
    g.extend_from_slice(&4u32.to_le_bytes());
    g.extend_from_slice(b"a.sr");
    // subtitle block (id 4)
    g.extend_from_slice(&4u16.to_le_bytes());
    g.extend_from_slice(&(content.len() as u32).to_le_bytes());
    g.extend_from_slice(content);
    g
  }

  /// Build an AVI whose `movi` list contains the given chunks (each a fully
  /// encoded `NNxx` chunk).  `streams_payload` are the strl LISTs in hdrl
  /// order.
  fn build_avi_with_movi(streams_payload: Vec<Vec<u8>>, movi_chunks: Vec<Vec<u8>>) -> Vec<u8> {
    let avih = encode_chunk(
      b"avih",
      &build_avih_payload(41_708, 5_000_000, 0x10, 240, streams_payload.len() as u32, 1920, 1080),
    );
    let mut hdrl_children = vec![avih];
    hdrl_children.extend(streams_payload);
    let hdrl = encode_list(b"LIST", b"hdrl", &hdrl_children);

    let mut riff_payload = b"AVI ".to_vec();
    riff_payload.extend(hdrl);
    riff_payload.extend(encode_list(b"LIST", b"movi", &movi_chunks));

    let total_size = riff_payload.len() as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&total_size.to_le_bytes());
    bytes.extend(riff_payload);
    bytes
  }

  fn build_avi(streams_payload: Vec<Vec<u8>>, with_odml: bool) -> Vec<u8> {
    let avih = encode_chunk(
      b"avih",
      &build_avih_payload(41_708, 5_000_000, 0x10, 240, streams_payload.len() as u32, 1920, 1080),
    );
    let mut hdrl_children = vec![avih];
    hdrl_children.extend(streams_payload);
    let hdrl = encode_list(b"LIST", b"hdrl", &hdrl_children);

    let mut riff_payload = b"AVI ".to_vec();
    riff_payload.extend(hdrl);
    if with_odml {
      let dmlh = encode_chunk(b"dmlh", &500_000u32.to_le_bytes());
      let odml = encode_list(b"LIST", b"odml", &[dmlh]);
      riff_payload.extend(odml);
    }
    // movi list (empty payload is fine for identification)
    let movi = encode_list(b"LIST", b"movi", &[]);
    riff_payload.extend(movi);

    // Manually wrap as RIFF with the AVI form type as the sub-FOURCC.
    let total_size = riff_payload.len() as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&total_size.to_le_bytes());
    bytes.extend(riff_payload);
    bytes
  }

  #[test]
  fn probe_accepts_riff_avi_header() {
    let bytes = build_avi(vec![build_video_strl(640, 480)], false);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(AviReader.probe(&mut s).unwrap());
    assert_eq!(s.position(), 0);
  }

  #[test]
  fn probe_rejects_short_input() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(b"RIFF".to_vec()));
    assert!(!AviReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_riff_with_non_avi_form_type() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(&[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!AviReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-061: standalone AVIX is not a primary AVI file -------------

  #[test]
  fn probe_rejects_standalone_avix() {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(b"AVIX");
    bytes.extend_from_slice(&[0u8; 4]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(!AviReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_video_and_audio_tracks() {
    let bytes = build_avi(vec![build_video_strl(1920, 1080), build_audio_strl()], false);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.container.format, ContainerFormat::Avi);
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert_eq!(out.tracks[1].track_type, TrackType::Audio);
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 1920);
    let a = out.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn read_headers_uses_odml_total_frames_when_present() {
    let bytes = build_avi(vec![build_video_strl(640, 480)], true);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert!(out.container.properties.duration.is_some());
  }

  #[test]
  fn missing_hdrl_returns_malformed() {
    let mut riff_payload = b"AVI ".to_vec();
    riff_payload.extend(encode_list(b"LIST", b"movi", &[]));
    let total_size = riff_payload.len() as u32;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&total_size.to_le_bytes());
    bytes.extend(riff_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    let err = AviReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }

  // ---- PARSER-192: subtitle detection from movi GAB2 text chunks --------

  #[test]
  fn recognised_gab2_srt_in_movi_creates_subtitle_track() {
    // Streams: video (0), audio (1), text (2).  The text chunk tag is "02tx".
    let srt = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHello world\r\n";
    let text_chunk = encode_chunk(b"02tx", &gab2_chunk(srt));
    let bytes = build_avi_with_movi(
      vec![build_video_strl(1920, 1080), build_audio_strl(), build_text_strl()],
      vec![text_chunk],
    );
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 3);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert_eq!(out.tracks[0].id, 0);
    assert_eq!(out.tracks[1].track_type, TrackType::Audio);
    assert_eq!(out.tracks[1].id, 1);
    let sub = &out.tracks[2];
    assert_eq!(sub.track_type, TrackType::Subtitles);
    assert_eq!(sub.id, 2);
    assert_eq!(sub.codec.id, "S_TEXT/UTF8");
    assert!(sub.properties.subtitle.as_ref().unwrap().text_subtitles);
  }

  #[test]
  fn unrecognised_text_chunk_creates_no_subtitle_track() {
    // The text chunk is GAB2 but its payload is not SRT/SSA — mkvtoolnix
    // would drop it, so no subtitle track is emitted.
    let junk = b"just some plain prose, definitely not a subtitle file";
    let text_chunk = encode_chunk(b"01tx", &gab2_chunk(junk));
    let bytes = build_avi_with_movi(
      vec![build_video_strl(1920, 1080), build_text_strl()],
      vec![text_chunk],
    );
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].track_type, TrackType::Video);
    assert!(out.tracks.iter().all(|t| t.track_type != TrackType::Subtitles));
  }

  #[test]
  fn text_stream_without_movi_chunk_creates_no_subtitle_track() {
    // A declared text stream whose first chunk never appears in movi must
    // not synthesise a generic subtitle track (old behaviour).
    let bytes = build_avi_with_movi(
      vec![build_video_strl(1920, 1080), build_text_strl()],
      vec![],
    );
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert!(out.tracks.iter().all(|t| t.track_type != TrackType::Subtitles));
  }

  #[test]
  fn subtitle_id_follows_audio_tracks_in_movi_flow() {
    // video (0), audio (1), audio (2), text (3) → subtitle id == 3.
    let srt = b"1\r\n00:00:01,000 --> 00:00:02,000\r\nHi\r\n";
    let text_chunk = encode_chunk(b"03tx", &gab2_chunk(srt));
    let bytes = build_avi_with_movi(
      vec![
        build_video_strl(1920, 1080),
        build_audio_strl(),
        build_audio_strl(),
        build_text_strl(),
      ],
      vec![text_chunk],
    );
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 4);
    let sub = out.tracks.iter().find(|t| t.track_type == TrackType::Subtitles).unwrap();
    assert_eq!(sub.id, 3);
    assert_eq!(sub.properties.common.number, Some(4));
  }

  // ---- PARSER-241: MPEG-4 Part 2 frame PAR → display dimensions ---------

  fn build_xvid_video_strl(width: u16, height: u16) -> Vec<u8> {
    let strh = encode_chunk(b"strh", &build_strh_payload(b"vids", b"XVID", 1001, 24000, 240, 0));
    let strf = encode_chunk(
      b"strf",
      &build_bitmapinfoheader(width as i32, height as i32, 24, b"XVID"),
    );
    let mut payload = strh;
    payload.extend(strf);
    encode_list(b"LIST", b"strl", &[payload])
  }

  /// A minimal MPEG-4 Part 2 VOL header carrying aspect_ratio_info = 2 (12:11):
  /// `00 00 01 20` start code, then `random_access(1)=0`, `vo_type(8)=0`,
  /// `is_old_id(1)=0`, `aspect_ratio_info(4)=0b0010`.
  fn xvid_vol_frame_12_11() -> Vec<u8> {
    vec![0x00, 0x00, 0x01, 0x20, 0x00, 0x08]
  }

  #[test]
  fn mpeg4_p2_frame_par_sets_display_dimensions() {
    // 720x480 XVID with a 12:11 PAR in the first frame and no vprp → display
    // dimensions stretched to 785x480 (mkvmerge's extended_identify_mpeg4_l2).
    let frame = encode_chunk(b"00dc", &xvid_vol_frame_12_11());
    let bytes = build_avi_with_movi(vec![build_xvid_video_strl(720, 480)], vec![frame]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 720);
    assert_eq!(
      v.display_dimensions,
      Some(crate::media_metadata::model::track_properties_video::Dimensions2D { width: 785, height: 480 })
    );
  }

  #[test]
  fn non_mpeg4_video_ignores_frame_par() {
    // An H.264 stream with the same frame bytes must not trigger frame-PAR
    // display scaling (the FOURCC is not MPEG-4 Part 2).
    let frame = encode_chunk(b"00dc", &xvid_vol_frame_12_11());
    let bytes = build_avi_with_movi(vec![build_video_strl(720, 480)], vec![frame]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    AviReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.display_dimensions, v.pixel_dimensions);
  }

  #[test]
  fn non_riff_top_level_chunk_is_rejected() {
    let mut bytes = b"FAKE".to_vec();
    bytes.extend_from_slice(&12u32.to_le_bytes());
    bytes.extend_from_slice(b"AVI ");
    bytes.extend_from_slice(&[0u8; 8]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.avi", 0);
    let err = AviReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
