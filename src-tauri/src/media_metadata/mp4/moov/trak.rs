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
use crate::media_metadata::model::track_properties_video::{
    Dimensions2D, VideoTrackProperties,
};

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
    }
}

pub fn parse(
    src: &mut FileSource,
    parent: &BoxHeader,
    deadline: &Deadline,
) -> Result<TrackBuilder, ParseError> {
    let mut builder = TrackBuilder::default();
    atom::walk_children(src, parent, "mp4::trak", deadline, |src, child| match &child.kind.0 {
        b"tkhd" => {
            let t = tkhd::parse(src, child)?;
            builder.track_id = Some(t.track_id);
            builder.display_width_fixed = Some(t.width_fixed);
            builder.display_height_fixed = Some(t.height_fixed);
            builder.enabled = Some(t.enabled);
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
        _ => Ok(ChildAction::Skip),
    })?;
    builder.merge_codec_config();
    Ok(builder)
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
    fn display_dimensions_none_when_either_zero() {
        let mut b = TrackBuilder::default();
        b.display_width_fixed = Some(0);
        b.display_height_fixed = Some(1080u32 << 16);
        assert!(b.display_dimensions().is_none());
    }

    #[test]
    fn merge_codec_config_bridges_video_config() {
        let mut b = TrackBuilder::default();
        b.video_codec_config = Some(
            crate::media_metadata::model::track_properties_video::VideoCodecConfig {
                profile_idc: Some(100),
                ..Default::default()
            },
        );
        b.merge_codec_config();
        let v = b.video.unwrap();
        assert_eq!(v.codec_config.unwrap().profile_idc, Some(100));
    }

    #[test]
    fn merge_codec_config_bridges_audio_config() {
        let mut b = TrackBuilder::default();
        b.audio_codec_config = Some(
            crate::media_metadata::model::track_properties_audio::AudioCodecConfig {
                aac_object_type: Some(2),
                ..Default::default()
            },
        );
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
