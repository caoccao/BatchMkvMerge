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
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::MediaMetadata;
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
        // Recognise: ftyp / moov / mdat / pnot / styp / free / skip / wide / sidx
        let kind = &head[4..8];
        Ok(matches!(
            kind,
            b"ftyp" | b"moov" | b"mdat" | b"pnot" | b"styp" | b"sidx" | b"free" | b"skip" | b"wide"
        ))
    }

    fn read_headers(
        &self,
        src: &mut FileSource,
        deadline: &Deadline,
        out: &mut MediaMetadata,
    ) -> Result<(), ParseError> {
        let mut filetype: Option<FileType> = None;
        let mut moov_builder = MoovBuilder::default();
        let mut have_moov = false;
        let mut is_fragmented = false;
        let mut fragment_counts: HashMap<u32, u32> = HashMap::new();

        let stream_end = src.length();
        src.seek_to(0)?;

        let mut iteration_guard = 0usize;
        loop {
            deadline.check("mp4::read_headers")?;
            if let Some(end) = stream_end {
                if src.position() >= end {
                    break;
                }
            }
            let header = match atom::read_box_header(src) {
                Ok(h) => h,
                Err(ParseError::UnexpectedEof { .. }) => break,
                Err(e) => return Err(e),
            };
            iteration_guard += 1;
            if iteration_guard > 1024 {
                // Defensive cap — pathological input should not loop here.
                break;
            }

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
                        moov::parse(src, &header, deadline, &mut moov_builder)?;
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
                b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" | b"sidx" => {
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
                    // Unknown / unsupported top-level box; skip past it.
                    atom::skip_payload(src, &header)?;
                }
            }
        }

        if !have_moov {
            return Err(ParseError::Malformed {
                format: "mp4",
                offset: 0,
                reason: "no moov box found".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_metadata::deadline::Deadline;
    use crate::media_metadata::mp4::atom::encode_box;
    use crate::media_metadata::mp4::moov::hdlr::build_hdlr_payload;
    use crate::media_metadata::mp4::moov::mdhd::build_mdhd_payload_v0;
    use crate::media_metadata::mp4::moov::mvhd::build_mvhd_payload_v0;
    use crate::media_metadata::mp4::moov::stbl::stsd::{
        build_audio_sample_entry_v0, build_stsd_payload, build_video_sample_entry,
    };
    use crate::media_metadata::mp4::moov::tkhd::build_tkhd_payload_v0;
    use crate::media_metadata::model::track::TrackType;
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

    fn build_audio_trak(
        track_id: u32,
        codec: &[u8; 4],
        lang: &str,
        sample_rate: u32,
        channels: u16,
    ) -> Vec<u8> {
        let tkhd = encode_box(b"tkhd", &build_tkhd_payload_v0(track_id, 0, 0));
        let mdhd = encode_box(b"mdhd", &build_mdhd_payload_v0(sample_rate, 0, lang));
        let hdlr = encode_box(b"hdlr", &build_hdlr_payload(b"soun", "SoundHandler"));
        let entry = build_audio_sample_entry_v0(codec, channels, 16, sample_rate, &[]);
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
        assert_eq!(
            out.tracks[0].properties.common.num_index_entries,
            Some(30),
        );
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
