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

//! `trak` (track) box dispatcher — wires `tkhd`, `mdia`, `edts` into a
//! [`TrackBuilder`] that the moov walker later converts into a
//! protocol-level [`crate::media_metadata::model::track::Track`].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use crate::media_metadata::mp4::atom::{self, BoxHeader, ChildAction};

use super::edts;
use super::mdia;
use super::tkhd;

/// Collector populated across the trak walk and (later) the stbl walk.
#[derive(Debug, Default)]
pub struct TrackBuilder {
  pub track_id: Option<u32>,
  pub enabled: Option<bool>,
  pub display_width_fixed: Option<u32>,
  pub display_height_fixed: Option<u32>,
  /// Display rotation derived from the tkhd matrix (PARSER-069).
  pub rotation_degrees: Option<u32>,
  /// `true` when the tkhd matrix signals a horizontal flip.
  pub flipped: bool,
  /// Raw 3×3 track display matrix, combined with the movie matrix to derive
  /// yaw/roll (PARSER-147).
  pub display_matrix: Option<[[i32; 3]; 3]>,

  pub media_timescale: Option<u32>,
  pub media_duration_units: Option<u64>,
  pub language_iso_639_2: Option<String>,

  pub handler_type: Option<[u8; 4]>,
  pub handler_name: Option<String>,

  /// FOURCC of the first sample-entry box (e.g. `avc1`, `mp4a`).
  pub sample_entry_kind: Option<[u8; 4]>,
  /// Display name from the codec catalogue.
  pub codec_name: Option<String>,
  /// Stored string id mkvmerge would render for this codec (FOURCC).
  pub codec_id_str: Option<String>,

  pub video: Option<VideoTrackProperties>,
  pub audio: Option<AudioTrackProperties>,

  /// `stts` first-entry derived default sample duration in media units.
  pub stts_first_sample_delta: Option<u32>,
  /// `stts` first-entry sample count — needed if we ever need to derive
  /// frame rates from `stts`.
  pub stts_first_sample_count: Option<u32>,

  /// Aggregate edit-list duration in movie timescale units.
  pub edts_total_duration: Option<u64>,
  /// `true` when the edit list contains a non-trivial sync point.
  pub edts_has_offset: bool,

  /// Per-track tag list collected from any handler-level meta atom.
  pub tags: Vec<crate::media_metadata::model::tag::TagEntry>,

  /// Hex-encoded codec private blob (set by codec_specific decoders).
  pub codec_private_hex: Option<String>,
  /// Decoded video codec configuration (avcC / hvcC).
  pub video_codec_config: Option<crate::media_metadata::model::track_properties_video::VideoCodecConfig>,
  /// Decoded audio codec configuration (esds).
  pub audio_codec_config: Option<crate::media_metadata::model::track_properties_audio::AudioCodecConfig>,
  /// `esds` objectTypeIndication — distinguishes AAC / MP3 / AC-3 / DTS for
  /// generic `mp4a` / `mp4v` sample entries (PARSER-043).
  pub esds_object_type: Option<u8>,
  /// PARSER-177: length in bytes of the `esds` DecoderSpecificInfo (tag 0x05),
  /// i.e. mkvtoolnix's `esds.decoder_config`.  Used by the reader verification
  /// pass to gate MP4V (must be present) and VobSub (must be ≥ 64 bytes).
  pub esds_decoder_specific_len: Option<usize>,

  /// Set when the track's `mdhd` is unsupported / malformed (bad version or
  /// zero timescale).  Such tracks are dropped from the output rather than
  /// failing the whole file — mirrors mkvtoolnix skipping the track
  /// (PARSER-146).
  pub media_invalid: bool,

  /// Number of samples in the (non-fragmented) track, taken from `stsz`.
  /// Surfaced as `num_index_entries` (PARSER-145).
  pub sample_count: Option<u32>,

  /// PARSER-212: track IDs referenced by this track's `tref/chap` (QuickTime
  /// chapter track references).  Mirrors `m_chapter_track_ids`
  /// (`../mkvtoolnix/src/input/r_qtmp4.cpp:1666-1679`).
  pub chapter_track_ids: Vec<u32>,

  /// PARSER-179: block-addition mappings collected from `dvcC` / `dvvC` /
  /// `hvcE` sample-entry boxes — each is `(fourcc, raw_payload_bytes)`.
  /// mkvtoolnix stores these via `add_data_as_block_addition`
  /// (`r_qtmp4.cpp:3318-3327`) rather than as the primary codec config.
  pub block_additions: Vec<(String, Vec<u8>)>,

  /// PARSER-177: absolute file offset of sample 0 (chunk_offset[0] from
  /// `stco` / `co64`).  Used by the reader's first-sample verification pass
  /// for bounded reads.
  pub first_sample_file_offset: Option<u64>,
  /// PARSER-177: size in bytes of sample 0 (`stsz` first entry, or the fixed
  /// `sample_size` when non-zero).
  pub first_sample_size: Option<u64>,
  /// PARSER-183: bounded ordered list of the FIRST samples as
  /// `(file_offset, size)` pairs, reconstructed from `stsc` (sample-to-chunk
  /// map) + `stco` / `co64` (chunk offsets) + `stsz` / `stz2` (sample sizes).
  /// Mirrors the prefix of mkvtoolnix's `m_index` so the reader's
  /// `read_first_bytes` can collect up to `MAX_FIRST_BYTES` across MULTIPLE
  /// samples (not just sample 0).  Capped so a tiny-sample stream never makes
  /// us build an unbounded table — see `stbl::MAX_INDEX_SAMPLES`.
  pub first_samples: Vec<(u64, u64)>,
  /// PARSER-177: set true by the reader's verification pass for tracks
  /// mkvtoolnix would reject (broken / missing decoder config).  `finalise`
  /// drops these before assigning compact ids.
  pub probe_failed: bool,

  /// PARSER-230: raw `esds` DecoderSpecificInfo bytes (tag 0x05), needed by the
  /// verification pass to unlace Vorbis-in-MP4 private data into its three Xiph
  /// headers and derive channels / sample rate from the identification header.
  pub esds_decoder_specific_data: Option<Vec<u8>>,
}

impl TrackBuilder {
  /// Pixel display dimensions, derived from the 16.16 tkhd fields.
  pub fn display_dimensions(&self) -> Option<Dimensions2D> {
    match (self.display_width_fixed, self.display_height_fixed) {
      (Some(w), Some(h)) if w != 0 && h != 0 => Some(Dimensions2D {
        width: tkhd::fixed_to_pixels(w),
        height: tkhd::fixed_to_pixels(h),
      }),
      _ => None,
    }
  }

  /// Apply track-level cross-references between fields before assembly:
  /// stts default-duration → video frame duration; codec config → video /
  /// audio bind.
  pub fn merge_codec_config(&mut self) {
    if let Some(cfg) = self.video_codec_config.clone() {
      let video = self.video.get_or_insert_with(VideoTrackProperties::default);
      video.codec_config = Some(cfg);
    }
    if let Some(cfg) = self.audio_codec_config.clone() {
      let audio = self.audio.get_or_insert_with(AudioTrackProperties::default);
      audio.codec_config = Some(cfg);
    }
    if self.rotation_degrees.is_some() || self.flipped {
      let video = self.video.get_or_insert_with(VideoTrackProperties::default);
      if video.rotation_degrees.is_none() {
        video.rotation_degrees = self.rotation_degrees;
      }
      if video.flipped.is_none() && self.flipped {
        video.flipped = Some(true);
      }
    }
  }
}

pub fn parse(src: &mut FileSource, parent: &BoxHeader, deadline: &Deadline) -> Result<TrackBuilder, ParseError> {
  let mut builder = TrackBuilder::default();
  atom::walk_children(src, parent, "mp4::trak", deadline, |src, child| match &child.kind.0 {
    b"tkhd" => {
      let t = tkhd::parse(src, child)?;
      builder.track_id = Some(t.track_id);
      builder.display_width_fixed = Some(t.width_fixed);
      builder.display_height_fixed = Some(t.height_fixed);
      builder.enabled = Some(t.enabled);
      builder.rotation_degrees = t.rotation_degrees;
      builder.flipped = t.flipped;
      builder.display_matrix = Some(t.matrix);
      Ok(ChildAction::Consumed)
    }
    b"mdia" => {
      mdia::parse(src, child, deadline, &mut builder)?;
      Ok(ChildAction::Consumed)
    }
    b"edts" => {
      edts::parse(src, child, deadline, &mut builder)?;
      Ok(ChildAction::Consumed)
    }
    b"tref" => {
      parse_tref(src, child, deadline, &mut builder)?;
      Ok(ChildAction::Consumed)
    }
    _ => Ok(ChildAction::Skip),
  })?;
  builder.merge_codec_config();
  Ok(builder)
}

/// Parse a `tref` (track reference) container, collecting the track IDs of any
/// `chap` (chapter) reference into `builder.chapter_track_ids`.  Mirrors
/// `handle_tref_atom` (`../mkvtoolnix/src/input/r_qtmp4.cpp:1666-1679`).
fn parse_tref(
  src: &mut FileSource,
  parent: &BoxHeader,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<(), ParseError> {
  atom::walk_children(src, parent, "mp4::tref", deadline, |src, child| {
    if &child.kind.0 == b"chap" {
      let payload = atom::read_payload(src, child, 4096)?;
      for chunk in payload.chunks_exact(4) {
        builder
          .chapter_track_ids
          .push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
      }
      Ok(ChildAction::Consumed)
    } else {
      Ok(ChildAction::Skip)
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mp4::atom::encode_box;
  use crate::media_metadata::mp4::moov::tkhd::build_tkhd_payload_v0;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  #[test]
  fn tkhd_populates_track_id_and_dims() {
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(42, 1920, 1080));
    let trak = encode_box(b"trak", &tkhd);
    let mut s = FileSource::from_reader_for_test(Cursor::new(trak));
    let parent = atom::read_box_header(&mut s).unwrap();
    let b = parse(&mut s, &parent, &dl()).unwrap();
    assert_eq!(b.track_id, Some(42));
    let dims = b.display_dimensions().unwrap();
    assert_eq!(dims.width, 1920);
    assert_eq!(dims.height, 1080);
  }

  #[test]
  fn missing_tkhd_leaves_track_id_none() {
    let trak = encode_box(b"trak", &[]);
    let mut s = FileSource::from_reader_for_test(Cursor::new(trak));
    let parent = atom::read_box_header(&mut s).unwrap();
    let b = parse(&mut s, &parent, &dl()).unwrap();
    assert!(b.track_id.is_none());
    assert!(b.display_dimensions().is_none());
  }

  #[test]
  fn tref_chap_collects_chapter_track_ids() {
    // PARSER-212: a `tref/chap` reference records the chapter track id.
    let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(1, 1920, 1080));
    let chap = encode_box(b"chap", &7u32.to_be_bytes());
    let tref = encode_box(b"tref", &chap);
    let mut trak_payload = tkhd;
    trak_payload.extend(tref);
    let trak = encode_box(b"trak", &trak_payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(trak));
    let parent = atom::read_box_header(&mut s).unwrap();
    let b = parse(&mut s, &parent, &dl()).unwrap();
    assert_eq!(b.track_id, Some(1));
    assert_eq!(b.chapter_track_ids, vec![7]);
  }

  #[test]
  fn display_dimensions_none_when_either_zero() {
    let mut b = TrackBuilder::default();
    b.display_width_fixed = Some(0);
    b.display_height_fixed = Some(1080u32 << 16);
    assert!(b.display_dimensions().is_none());
  }

  #[test]
  fn merge_codec_config_bridges_video_config() {
    let mut b = TrackBuilder::default();
    b.video_codec_config = Some(crate::media_metadata::model::track_properties_video::VideoCodecConfig {
      profile_idc: Some(100),
      ..Default::default()
    });
    b.merge_codec_config();
    let v = b.video.unwrap();
    assert_eq!(v.codec_config.unwrap().profile_idc, Some(100));
  }

  #[test]
  fn merge_codec_config_bridges_audio_config() {
    let mut b = TrackBuilder::default();
    b.audio_codec_config = Some(crate::media_metadata::model::track_properties_audio::AudioCodecConfig {
      aac_object_type: Some(2),
      ..Default::default()
    });
    b.merge_codec_config();
    let a = b.audio.unwrap();
    assert_eq!(a.codec_config.unwrap().aac_object_type, Some(2));
  }

  #[test]
  fn merge_codec_config_no_op_when_no_codec_config() {
    let mut b = TrackBuilder::default();
    b.merge_codec_config();
    assert!(b.video.is_none());
    assert!(b.audio.is_none());
  }

  #[test]
  fn edts_payload_records_offset() {
    let elst = crate::media_metadata::mp4::moov::edts::build_elst_v0(&[(1000, 0)]);
    let elst = encode_box(b"elst", &elst);
    let edts = encode_box(b"edts", &elst);
    let trak = encode_box(b"trak", &edts);
    let mut s = FileSource::from_reader_for_test(Cursor::new(trak));
    let parent = atom::read_box_header(&mut s).unwrap();
    let b = parse(&mut s, &parent, &dl()).unwrap();
    assert_eq!(b.edts_total_duration, Some(1000));
    assert!(!b.edts_has_offset);
  }

  #[test]
  fn unknown_child_ignored() {
    let bogus = encode_box(b"junk", &[0u8; 4]);
    let trak = encode_box(b"trak", &bogus);
    let mut s = FileSource::from_reader_for_test(Cursor::new(trak));
    let parent = atom::read_box_header(&mut s).unwrap();
    let b = parse(&mut s, &parent, &dl()).unwrap();
    assert!(b.track_id.is_none());
    assert!(b.video_codec_config.is_none());
  }
}
