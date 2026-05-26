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

//! Convert the parsed AVI builders into protocol-level `Track`s.

use crate::media_metadata::codec::fourcc;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerProperties;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

use super::avih::MainAviHeader;
use super::odml::OdmlInfo;
use super::strl::{AviStreamKind, BitmapInfoHeader, StreamBuilder, StreamFormat, VideoPropertiesHeader, WaveFormatEx};
use super::subtitles::AviSubtitleDemuxer;

/// Drive the per-stream conversion and stamp container fields.
///
/// PARSER-193: track ids mirror mkvtoolnix's stable numbering
/// (`r_avi.cpp:868-933`): the single video track is id 0, audio tracks are
/// numbered `1..=N` in audio-stream order, and subtitle tracks follow the
/// audio tracks.  Skipped / unsupported stream-list entries never consume an
/// id, so they cannot shift later track numbers.
pub fn finalise(
  avih: Option<MainAviHeader>,
  streams: Vec<StreamBuilder>,
  odml: OdmlInfo,
  subtitles: Vec<AviSubtitleDemuxer>,
  video_frame_par: Option<(u32, u32)>,
  out: &mut MediaMetadata,
) {
  if let Some(avih) = avih {
    set_container_props(&avih, odml, &mut out.container.properties);
  }

  // mkvtoolnix emits at most one video track at id 0 (`identify_video`).
  let mut audio_count: i64 = 0;
  let mut emitted_video = false;
  for builder in streams.into_iter() {
    let kind = match builder.header.as_ref() {
      Some(h) => h.kind,
      None => continue,
    };
    match kind {
      AviStreamKind::Video if !emitted_video => {
        // mkvtoolnix's avilib binds the *first* `vids` stream as the one video
        // track and never falls through to a later one, so the slot is consumed
        // here even when `verify_video_track` rejects the bitmap header
        // (PARSER-273). Subsequent video streams are ignored regardless.
        emitted_video = true;
        if let Some(track) = make_video_track(0, builder, video_frame_par) {
          out.tracks.push(track);
        }
      }
      AviStreamKind::Audio => {
        // Audio id is `i + 1` where `i` is the audio-stream index
        // (`r_avi.cpp:917`).
        let id = audio_count + 1;
        if let Some(track) = make_audio_track(id, builder) {
          out.tracks.push(track);
          audio_count += 1;
        }
      }
      _ => {}
    }
  }

  // Subtitle ids follow the audio tracks: `1 + audio_tracks + i`
  // (`r_avi.cpp:933`).  PARSER-213: SSA/ASS demuxers also contribute their
  // embedded `[Fonts]` / `[Graphics]` attachments, emitted with sequential ids
  // continuing from any already present (mirrors `identify_attachments`'
  // `set_attachment_id_base(g_attachments.size())`).
  let mut attachment_id = out.attachments.len() as u32;
  for (i, mut demuxer) in subtitles.into_iter().enumerate() {
    let id = 1 + audio_count + i as i64;
    let attachments = std::mem::take(&mut demuxer.attachments);
    out.tracks.push(make_subtitle_track(id, demuxer));
    for mut attachment in attachments {
      attachment_id += 1;
      attachment.id = attachment_id;
      out.attachments.push(attachment);
    }
  }

  out.tags.per_track_count = out.tracks.iter().map(|t| t.properties.tags.len() as u32).sum();
}

fn set_container_props(avih: &MainAviHeader, odml: OdmlInfo, props: &mut ContainerProperties) {
  props.bitrate_bps = avih.average_bitrate_bps();
  // Derive total file duration from frame count × per-frame microseconds.
  let frames = odml.total_frames.unwrap_or(avih.total_frames);
  if frames > 0 && avih.microsec_per_frame > 0 {
    let ns: u64 = (frames as u64).saturating_mul(avih.microsec_per_frame as u64) * 1000;
    props.duration = Some(crate::media_metadata::model::duration::DurationValue::from_ns(ns));
  }
  props.is_fragmented = Some(false);
}

/// Build the shared common-track properties from a stream's `strh`.  The
/// model's `number` is `id + 1` per the rest of the parser (mkvtoolnix's
/// track id maps to our `id`).
fn common_props(id: i64, header: &super::strl::StreamHeader, name: Option<String>) -> CommonTrackProperties {
  let mut common = CommonTrackProperties::default();
  common.number = Some(id as u64 + 1);
  common.track_name = name;
  if header.length > 0 {
    common.num_index_entries = Some(header.length as u64);
  }
  if header.language != 0 {
    // AVI's language is an LCID — we don't have a full mapping table.
    // Surface the raw value when present.
    common.language = Some(Language::resolve(
      None,
      Some(&format!("lcid-{}", header.language)),
      false,
    ));
  }
  common
}

/// Build the single video track (id 0).  Returns `None` for non-video streams
/// or streams without a BITMAPINFOHEADER `strf`.
fn make_video_track(id: i64, builder: StreamBuilder, video_frame_par: Option<(u32, u32)>) -> Option<Track> {
  let header = builder.header.as_ref()?;
  if header.kind != AviStreamKind::Video {
    return None;
  }
  let StreamFormat::Video(bmih) = builder.format.as_ref()? else {
    return None;
  };
  // PARSER-273: mirror `avi_reader_c::verify_video_track` (`r_avi.cpp:112-114`).
  // mkvtoolnix only identifies a video track when the BITMAPINFOHEADER is at
  // least `sizeof(alBITMAPINFOHEADER)` (40) bytes and both the (sign-stripped)
  // width and height are nonzero. A malformed stream that fails any of these
  // is suppressed rather than emitted as a false-positive track.
  if bmih.size < 40 || bmih.width.unsigned_abs() == 0 || bmih.height.unsigned_abs() == 0 {
    return None;
  }
  let codec_id = fourcc_to_string(&bmih.compression);
  let codec_name = fourcc::lookup(&codec_id).map(|e| e.name.to_string());
  let video = video_properties(bmih, header, builder.vprp.as_ref(), video_frame_par);
  // PARSER-085: surface `BITMAPINFOHEADER + extradata` as the video
  // codec_private blob.  Mirrors mkvtoolnix's `r_avi.cpp:188-206`.
  let private = Some(CodecPrivate::from_bytes(&bmih_codec_private(bmih)));
  let common = common_props(id, header, builder.name.clone());
  Some(Track {
    id,
    track_type: TrackType::Video,
    codec: CodecInfo {
      id: codec_id,
      name: codec_name,
      codec_private: private,
    },
    properties: TrackProperties {
      common,
      video: Some(video),
      ..TrackProperties::default()
    },
  })
}

/// Build an audio track.  Returns `None` for non-audio streams or streams
/// without a WAVEFORMATEX `strf`.
fn make_audio_track(id: i64, builder: StreamBuilder) -> Option<Track> {
  let header = builder.header.as_ref()?;
  if header.kind != AviStreamKind::Audio {
    return None;
  }
  let StreamFormat::Audio(wf) = builder.format.as_ref()? else {
    return None;
  };
  // WAVE_FORMAT_EXTENSIBLE (0xFFFE) carries the real format tag in the
  // SubFormat GUID's 32-bit data1 field. mkvtoolnix only unwraps when the
  // declared extension is a complete alWAVEFORMATEXTENSION (22 bytes).
  let effective_tag = if wf.format_tag == 0xFFFE && wf.extra.len() >= 22 {
    u32::from_le_bytes([wf.extra[6], wf.extra[7], wf.extra[8], wf.extra[9]])
  } else {
    wf.format_tag as u32
  };
  let codec_id = if effective_tag <= u16::MAX as u32 {
    format!("0x{:04X}", effective_tag)
  } else {
    format!("0x{:08X}", effective_tag)
  };
  let codec_name = u16::try_from(effective_tag).ok().and_then(name_from_wave_tag);
  let private = if wf.extra.is_empty() {
    None
  } else {
    Some(CodecPrivate::from_bytes(&wf.extra))
  };
  let audio = audio_properties(wf);
  let common = common_props(id, header, builder.name.clone());
  Some(Track {
    id,
    track_type: TrackType::Audio,
    codec: CodecInfo {
      id: codec_id,
      name: codec_name,
      codec_private: private,
    },
    properties: TrackProperties {
      common,
      audio: Some(audio),
      ..TrackProperties::default()
    },
  })
}

/// Build a subtitle track from a recognised `movi` GAB2 demuxer (PARSER-192).
/// Only SRT / SSA demuxers reach this point — mkvtoolnix drops unknown content.
fn make_subtitle_track(id: i64, demuxer: AviSubtitleDemuxer) -> Track {
  let mut common = CommonTrackProperties::default();
  common.number = Some(id as u64 + 1);
  let subtitle = SubtitleTrackProperties {
    text_subtitles: true,
    encoding: demuxer.encoding.clone(),
    variant: Some(demuxer.kind.codec_name().to_string()),
    teletext_page: None,
  };
  Track {
    id,
    track_type: TrackType::Subtitles,
    codec: CodecInfo {
      id: demuxer.kind.codec_id().to_string(),
      name: Some(demuxer.kind.codec_name().to_string()),
      codec_private: None,
    },
    properties: TrackProperties {
      common,
      subtitle: Some(subtitle),
      ..TrackProperties::default()
    },
  }
}

fn video_properties(
  bmih: &BitmapInfoHeader,
  header: &super::strl::StreamHeader,
  vprp: Option<&VideoPropertiesHeader>,
  video_frame_par: Option<(u32, u32)>,
) -> VideoTrackProperties {
  let width = bmih.width.max(0) as u32;
  let height = bmih.height.unsigned_abs(); // height may be negative ⇒ top-down DIB
  let pixel = if width > 0 && height > 0 {
    Some(Dimensions2D { width, height })
  } else {
    None
  };
  // PARSER-084: when `vprp` carries a frame aspect ratio, derive display
  // dimensions from it instead of mirroring the pixel dimensions.  Mirrors
  // `avi_reader_c::handle_video_aspect_ratio` (`r_avi.cpp:241-273`).
  //
  // PARSER-241: otherwise, an MPEG-4 Part 2 (DivX/Xvid) stream may carry the
  // pixel aspect ratio only in the first frame's VOL header; when that frame
  // PAR was extracted, apply it (`extended_identify_mpeg4_l2`,
  // `r_avi.cpp:843-865`).
  let display = match (pixel, vprp) {
    (Some(p), Some(v))
      if v.frame_aspect_ratio_x != 0 && v.frame_aspect_ratio_y != 0 && p.width != 0 && p.height != 0 =>
    {
      Some(display_dimensions_from_aspect(p, v))
    }
    (Some(p), _) => match video_frame_par {
      Some((par_num, par_den)) => {
        let (w, h) = super::mpeg4_par::display_dimensions(p.width, p.height, par_num, par_den);
        Some(Dimensions2D { width: w, height: h })
      }
      None => pixel,
    },
    _ => pixel,
  };
  let default_duration_ns = header.frame_duration_ns();
  let mut props = VideoTrackProperties {
    pixel_dimensions: pixel,
    display_dimensions: display,
    default_duration_ns,
    ..VideoTrackProperties::default()
  };
  if bmih.bit_count != 0 && bmih.bit_count != 24 {
    let mut color = crate::media_metadata::model::track_properties_video::ColorMetadata::default();
    color.bits_per_channel = Some(bmih.bit_count as u32);
    props.color = Some(color);
  }
  props
}

fn display_dimensions_from_aspect(pixel: Dimensions2D, vprp: &VideoPropertiesHeader) -> Dimensions2D {
  let x = vprp.frame_aspect_ratio_x as u64;
  let y = vprp.frame_aspect_ratio_y as u64;
  let pw = pixel.width as u64;
  let ph = pixel.height as u64;
  // Compare aspect ratio (x/y) with pixel ratio (pw/ph) via cross multiply.
  if x * ph >= y * pw {
    Dimensions2D {
      width: ((x * ph + y / 2) / y) as u32,
      height: pixel.height,
    }
  } else {
    Dimensions2D {
      width: pixel.width,
      height: ((y * pw + x / 2) / x) as u32,
    }
  }
}

/// Serialise the BITMAPINFOHEADER + trailing extradata into a contiguous
/// 40+N-byte blob (PARSER-085).  Mirrors mkvtoolnix's
/// `r_avi.cpp:188-206` codec_private layout.
fn bmih_codec_private(bmih: &BitmapInfoHeader) -> Vec<u8> {
  if bmih.raw.len() >= 40 {
    return bmih.raw.clone();
  }
  let mut bytes = Vec::with_capacity(40 + bmih.extra.len());
  bytes.extend_from_slice(&bmih.size.to_le_bytes());
  bytes.extend_from_slice(&bmih.width.to_le_bytes());
  bytes.extend_from_slice(&bmih.height.to_le_bytes());
  bytes.extend_from_slice(&bmih.planes.to_le_bytes());
  bytes.extend_from_slice(&bmih.bit_count.to_le_bytes());
  bytes.extend_from_slice(&bmih.compression);
  bytes.extend_from_slice(&bmih.image_size.to_le_bytes());
  bytes.extend_from_slice(&[0u8; 16]);
  bytes.extend_from_slice(&bmih.extra);
  bytes
}

fn audio_properties(wf: &WaveFormatEx) -> AudioTrackProperties {
  AudioTrackProperties {
    channels: if wf.channels == 0 {
      None
    } else {
      Some(wf.channels as u32)
    },
    sampling_frequency: if wf.samples_per_sec == 0 {
      None
    } else {
      Some(wf.samples_per_sec as f64)
    },
    bit_depth: if wf.bits_per_sample == 0 {
      None
    } else {
      Some(wf.bits_per_sample as u32)
    },
    ..AudioTrackProperties::default()
  }
}

fn fourcc_to_string(bytes: &[u8; 4]) -> String {
  bytes
    .iter()
    .map(|b| if (0x20..=0x7E).contains(b) { *b as char } else { '?' })
    .collect()
}

fn name_from_wave_tag(tag: u16) -> Option<String> {
  let name = match tag {
    0x0001 => "PCM",
    0x0002 => "ADPCM",
    0x0003 => "PCM (Float)",
    0x0050 => "MPEG Audio Layer 1/2",
    0x0055 => "MP3",
    0x00FF => "AAC",
    0x0161 => "WMA v2",
    0x0162 => "WMA Pro",
    0x0163 => "WMA Lossless",
    0x2000 => "AC-3",
    0x2001 => "DTS",
    0x6771 => "Vorbis",
    _ => return None,
  };
  Some(name.to_string())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::avi::strl::StreamHeader;
  use crate::media_metadata::avi::subtitles::AviSubtitleKind;

  fn dummy_video_header() -> StreamHeader {
    StreamHeader {
      kind: AviStreamKind::Video,
      fcc_handler: *b"H264",
      flags: 0,
      priority: 0,
      language: 0,
      scale: 1001,
      rate: 24000,
      start: 0,
      length: 240,
      sample_size: 0,
    }
  }

  fn dummy_audio_header() -> StreamHeader {
    StreamHeader {
      kind: AviStreamKind::Audio,
      fcc_handler: [0; 4],
      flags: 0,
      priority: 0,
      language: 0,
      scale: 1,
      rate: 48000,
      start: 0,
      length: 0,
      sample_size: 4,
    }
  }

  fn dummy_text_header() -> StreamHeader {
    StreamHeader {
      kind: AviStreamKind::Text,
      fcc_handler: *b"DXSA",
      flags: 0,
      priority: 0,
      language: 0,
      scale: 1,
      rate: 1000,
      start: 0,
      length: 0,
      sample_size: 0,
    }
  }

  fn video_builder() -> StreamBuilder {
    StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(BitmapInfoHeader {
        size: 40,
        width: 1920,
        height: 1080,
        planes: 1,
        bit_count: 24,
        compression: *b"H264",
        image_size: 0,
        raw: Vec::new(),
        extra: Vec::new(),
      })),
      name: None,
      private: None,
      vprp: None,
    }
  }

  fn audio_builder() -> StreamBuilder {
    StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(WaveFormatEx {
        format_tag: 0x0055,
        channels: 2,
        samples_per_sec: 48000,
        avg_bytes_per_sec: 16000,
        block_align: 4,
        bits_per_sample: 16,
        extra: Vec::new(),
      })),
      name: None,
      private: None,
      vprp: None,
    }
  }

  fn text_builder() -> StreamBuilder {
    StreamBuilder {
      header: Some(dummy_text_header()),
      format: None,
      name: None,
      private: None,
      vprp: None,
    }
  }

  fn no_avih() -> Option<MainAviHeader> {
    None
  }

  #[test]
  fn video_track_emitted_with_dims_and_duration() {
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1920,
      height: 1080,
      planes: 1,
      bit_count: 24,
      compression: *b"H264",
      image_size: 0,
      raw: Vec::new(),
      extra: Vec::new(),
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_video_track(0, builder, None).unwrap();
    assert_eq!(track.track_type, TrackType::Video);
    assert_eq!(track.codec.id, "H264");
    let v = track.properties.video.as_ref().unwrap();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    assert_eq!(v.default_duration_ns, Some(41_708_333));
  }

  // ---- PARSER-084: vprp display aspect ratio ---------------------------

  #[test]
  fn vprp_sixteen_nine_aspect_expands_display_width() {
    // 1440x1080 raster with vprp 16:9 → display 1920x1080.
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1440,
      height: 1080,
      planes: 1,
      bit_count: 24,
      compression: *b"H264",
      image_size: 0,
      raw: Vec::new(),
      extra: Vec::new(),
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: Some(VideoPropertiesHeader {
        frame_aspect_ratio_x: 16,
        frame_aspect_ratio_y: 9,
        frame_width_in_pixels: 1440,
        frame_height_in_lines: 1080,
      }),
    };
    let track = make_video_track(0, builder, None).unwrap();
    let v = track.properties.video.unwrap();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn vprp_invalid_zero_aspect_is_ignored() {
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1920,
      height: 1080,
      planes: 1,
      bit_count: 24,
      compression: *b"H264",
      image_size: 0,
      raw: Vec::new(),
      extra: Vec::new(),
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: Some(VideoPropertiesHeader {
        frame_aspect_ratio_x: 0,
        frame_aspect_ratio_y: 0,
        frame_width_in_pixels: 0,
        frame_height_in_lines: 0,
      }),
    };
    let track = make_video_track(0, builder, None).unwrap();
    let v = track.properties.video.unwrap();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  // ---- PARSER-085: BITMAPINFOHEADER + extradata as codec_private -----

  #[test]
  fn video_extradata_is_stored_in_codec_private() {
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1920,
      height: 1080,
      planes: 1,
      bit_count: 24,
      compression: *b"XVID",
      image_size: 0,
      raw: Vec::new(),
      extra: vec![0x01, 0x02, 0x03, 0x04],
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_video_track(0, builder, None).unwrap();
    let private = track.codec.codec_private.unwrap();
    assert_eq!(private.length, 44);
    // First 4 bytes are the BMIH `size` (40) in LE.
    assert!(private.hex.starts_with("28000000"));
    // Last 8 chars are the 4 extradata bytes.
    assert!(private.hex.ends_with("01020304"));
  }

  #[test]
  fn video_codec_private_preserves_original_bitmap_header_bytes() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&40u32.to_le_bytes());
    raw.extend_from_slice(&1920i32.to_le_bytes());
    raw.extend_from_slice(&1080i32.to_le_bytes());
    raw.extend_from_slice(&1u16.to_le_bytes());
    raw.extend_from_slice(&24u16.to_le_bytes());
    raw.extend_from_slice(b"XVID");
    raw.extend_from_slice(&0u32.to_le_bytes());
    raw.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    raw.extend_from_slice(&[0xaa, 0xbb]);
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1920,
      height: 1080,
      planes: 1,
      bit_count: 24,
      compression: *b"XVID",
      image_size: 0,
      raw: raw.clone(),
      extra: vec![0xaa, 0xbb],
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_video_track(0, builder, None).unwrap();
    let private = track.codec.codec_private.unwrap();
    assert_eq!(private.length, raw.len() as u64);
    assert_eq!(private.hex, raw.iter().map(|b| format!("{b:02x}")).collect::<String>());
  }

  #[test]
  fn negative_height_top_down_dib_is_flipped_positive() {
    let bmih = BitmapInfoHeader {
      size: 40,
      width: 1920,
      height: -1080,
      planes: 1,
      bit_count: 24,
      compression: *b"H264",
      image_size: 0,
      raw: Vec::new(),
      extra: Vec::new(),
    };
    let builder = StreamBuilder {
      header: Some(dummy_video_header()),
      format: Some(StreamFormat::Video(bmih)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_video_track(0, builder, None).unwrap();
    let v = track.properties.video.unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().height, 1080);
  }

  #[test]
  fn audio_track_emitted_with_channels_and_rate() {
    let wf = WaveFormatEx {
      format_tag: 0x0055,
      channels: 2,
      samples_per_sec: 48000,
      avg_bytes_per_sec: 16000,
      block_align: 4,
      bits_per_sample: 16,
      extra: Vec::new(),
    };
    let builder = StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(wf)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_audio_track(1, builder).unwrap();
    assert_eq!(track.track_type, TrackType::Audio);
    assert_eq!(track.codec.id, "0x0055");
    assert_eq!(track.codec.name.as_deref(), Some("MP3"));
    let a = track.properties.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    assert_eq!(a.bit_depth, Some(16));
  }

  #[test]
  fn audio_extra_bytes_become_codec_private() {
    let wf = WaveFormatEx {
      format_tag: 0x00FF,
      channels: 2,
      samples_per_sec: 48000,
      avg_bytes_per_sec: 16000,
      block_align: 4,
      bits_per_sample: 16,
      extra: vec![0x11, 0x90],
    };
    let builder = StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(wf)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_audio_track(1, builder).unwrap();
    let private = track.codec.codec_private.unwrap();
    assert_eq!(private.length, 2);
  }

  #[test]
  fn recognised_subtitle_demuxer_becomes_subtitle_track() {
    let track = make_subtitle_track(
      3,
      AviSubtitleDemuxer {
        kind: AviSubtitleKind::Srt,
        encoding: Some("UTF-8".to_string()),
        attachments: Vec::new(),
      },
    );
    assert_eq!(track.track_type, TrackType::Subtitles);
    assert_eq!(track.id, 3);
    assert_eq!(track.codec.id, "S_TEXT/UTF8");
    let sub = track.properties.subtitle.unwrap();
    assert!(sub.text_subtitles);
    assert_eq!(sub.variant.as_deref(), Some("SRT"));
    assert_eq!(sub.encoding.as_deref(), Some("UTF-8"));
  }

  // ---- PARSER-059: WAVEFORMATEXTENSIBLE resolution ---------------------

  #[test]
  fn extensible_audio_resolves_subformat_tag() {
    // wValidBitsPerSample(2) + dwChannelMask(4) + GUID(16); data1 = 0x0001.
    let mut extra = Vec::new();
    extra.extend_from_slice(&16u16.to_le_bytes()); // valid bits
    extra.extend_from_slice(&3u32.to_le_bytes()); // channel mask
    extra.extend_from_slice(&1u32.to_le_bytes()); // GUID data1 = PCM
    extra.extend_from_slice(&[0u8; 12]); // rest of GUID
    let wf = WaveFormatEx {
      format_tag: 0xFFFE,
      channels: 2,
      samples_per_sec: 48000,
      avg_bytes_per_sec: 192000,
      block_align: 4,
      bits_per_sample: 16,
      extra,
    };
    let builder = StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(wf)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_audio_track(1, builder).unwrap();
    assert_eq!(track.codec.id, "0x0001");
    assert_eq!(track.codec.name.as_deref(), Some("PCM"));
  }

  #[test]
  fn extensible_audio_requires_complete_extension_before_unwrap() {
    let mut extra = Vec::new();
    extra.extend_from_slice(&16u16.to_le_bytes());
    extra.extend_from_slice(&3u32.to_le_bytes());
    extra.extend_from_slice(&1u32.to_le_bytes());
    let wf = WaveFormatEx {
      format_tag: 0xFFFE,
      channels: 2,
      samples_per_sec: 48000,
      avg_bytes_per_sec: 192000,
      block_align: 4,
      bits_per_sample: 16,
      extra,
    };
    let builder = StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(wf)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_audio_track(1, builder).unwrap();
    assert_eq!(track.codec.id, "0xFFFE");
  }

  #[test]
  fn extensible_audio_reads_full_guid_data1() {
    let mut extra = Vec::new();
    extra.extend_from_slice(&16u16.to_le_bytes());
    extra.extend_from_slice(&3u32.to_le_bytes());
    extra.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    extra.extend_from_slice(&[0u8; 12]);
    let wf = WaveFormatEx {
      format_tag: 0xFFFE,
      channels: 2,
      samples_per_sec: 48000,
      avg_bytes_per_sec: 192000,
      block_align: 4,
      bits_per_sample: 16,
      extra,
    };
    let builder = StreamBuilder {
      header: Some(dummy_audio_header()),
      format: Some(StreamFormat::Audio(wf)),
      name: None,
      private: None,
      vprp: None,
    };
    let track = make_audio_track(1, builder).unwrap();
    assert_eq!(track.codec.id, "0x12345678");
    assert!(track.codec.name.is_none());
  }

  #[test]
  fn make_video_track_drops_stream_without_header() {
    assert!(make_video_track(0, StreamBuilder::default(), None).is_none());
  }

  #[test]
  fn make_video_track_rejects_audio_stream() {
    assert!(make_video_track(0, audio_builder(), None).is_none());
  }

  #[test]
  fn make_audio_track_rejects_video_stream() {
    assert!(make_audio_track(1, video_builder()).is_none());
  }

  // ---- PARSER-273: verify_video_track rejection ------------------------

  fn video_builder_with(size: u32, width: i32, height: i32) -> StreamBuilder {
    let mut b = video_builder();
    if let Some(StreamFormat::Video(bmih)) = b.format.as_mut() {
      bmih.size = size;
      bmih.width = width;
      bmih.height = height;
    }
    b
  }

  #[test]
  fn make_video_track_rejects_undersized_bitmap_header() {
    assert!(make_video_track(0, video_builder_with(30, 1920, 1080), None).is_none());
  }

  #[test]
  fn make_video_track_rejects_zero_width() {
    assert!(make_video_track(0, video_builder_with(40, 0, 1080), None).is_none());
  }

  #[test]
  fn make_video_track_rejects_zero_height() {
    assert!(make_video_track(0, video_builder_with(40, 1920, 0), None).is_none());
  }

  #[test]
  fn finalise_suppresses_invalid_video_and_keeps_audio_numbering() {
    // A malformed video stream (zero dimensions) is rejected, but it still
    // consumes the single video slot, so a later valid video stream is not
    // promoted and audio still lands at id 1.
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder_with(40, 0, 0), video_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![],
      None,
      &mut m,
    );
    assert!(m.tracks.iter().all(|t| t.track_type != TrackType::Video));
    let audio = m.tracks.iter().find(|t| t.track_type == TrackType::Audio).unwrap();
    assert_eq!(audio.id, 1);
  }

  // ---- PARSER-193: stable video / audio / subtitle id assignment -------

  #[test]
  fn finalise_assigns_video_id0_and_audio_ids_after_it() {
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder(), audio_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![],
      None,
      &mut m,
    );
    assert_eq!(m.tracks.len(), 3);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.tracks[0].id, 0);
    assert_eq!(m.tracks[1].track_type, TrackType::Audio);
    assert_eq!(m.tracks[1].id, 1);
    assert_eq!(m.tracks[2].track_type, TrackType::Audio);
    assert_eq!(m.tracks[2].id, 2);
    // number == id + 1 per the parser convention.
    assert_eq!(m.tracks[2].properties.common.number, Some(3));
  }

  #[test]
  fn finalise_skipped_stream_entries_do_not_shift_audio_ids() {
    // A Midi stream and a text stream with no recognised subtitle sit
    // between the video and audio streams.  They must not consume an id, so
    // the audio track still gets id 1.
    let mut midi = video_builder();
    midi.header.as_mut().unwrap().kind = AviStreamKind::Midi;
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder(), midi, text_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![],
      None,
      &mut m,
    );
    assert_eq!(m.tracks.len(), 2);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.tracks[0].id, 0);
    assert_eq!(m.tracks[1].track_type, TrackType::Audio);
    assert_eq!(m.tracks[1].id, 1);
  }

  #[test]
  fn finalise_subtitles_numbered_after_audio_tracks() {
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![
        AviSubtitleDemuxer {
          kind: AviSubtitleKind::Srt,
          encoding: Some("UTF-8".to_string()),
          attachments: Vec::new(),
        },
        AviSubtitleDemuxer {
          kind: AviSubtitleKind::Ssa,
          encoding: None,
          attachments: Vec::new(),
        },
      ],
      None,
      &mut m,
    );
    assert_eq!(m.tracks.len(), 4);
    // video=0, audio=1, subtitles=2,3.
    assert_eq!(m.tracks[2].track_type, TrackType::Subtitles);
    assert_eq!(m.tracks[2].id, 2);
    assert_eq!(m.tracks[2].codec.id, "S_TEXT/UTF8");
    assert_eq!(m.tracks[3].id, 3);
    assert_eq!(m.tracks[3].codec.id, "S_TEXT/ASS");
  }

  #[test]
  fn finalise_emits_ssa_embedded_attachments() {
    use crate::media_metadata::model::attachment::Attachment;
    let attachment = Attachment {
      id: 1,
      file_name: "myfont.ttf".to_string(),
      mime_type: Some("font/sfnt".to_string()),
      description: Some("SSA/ASS embedded font".to_string()),
      size: 1024,
      uid_hex: None,
    };
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![AviSubtitleDemuxer {
        kind: AviSubtitleKind::Ssa,
        encoding: None,
        attachments: vec![attachment],
      }],
      None,
      &mut m,
    );
    // PARSER-213: the SSA demuxer's font is surfaced as a global attachment.
    assert_eq!(m.attachments.len(), 1);
    assert_eq!(m.attachments[0].id, 1);
    assert_eq!(m.attachments[0].file_name, "myfont.ttf");
    assert_eq!(m.attachments[0].mime_type.as_deref(), Some("font/sfnt"));
  }

  #[test]
  fn finalise_emits_only_one_video_track() {
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(
      no_avih(),
      vec![video_builder(), video_builder(), audio_builder()],
      OdmlInfo::default(),
      vec![],
      None,
      &mut m,
    );
    // Second video stream is ignored (mkvtoolnix only identifies track 0).
    let video_count = m.tracks.iter().filter(|t| t.track_type == TrackType::Video).count();
    assert_eq!(video_count, 1);
    // The audio track still lands at id 1.
    let audio = m.tracks.iter().find(|t| t.track_type == TrackType::Audio).unwrap();
    assert_eq!(audio.id, 1);
  }

  #[test]
  fn name_from_wave_tag_table() {
    assert_eq!(name_from_wave_tag(0x0001).as_deref(), Some("PCM"));
    assert_eq!(name_from_wave_tag(0x0055).as_deref(), Some("MP3"));
    assert_eq!(name_from_wave_tag(0x00FF).as_deref(), Some("AAC"));
    assert_eq!(name_from_wave_tag(0x2000).as_deref(), Some("AC-3"));
    assert_eq!(name_from_wave_tag(0x6771).as_deref(), Some("Vorbis"));
    assert!(name_from_wave_tag(0xFFFF).is_none());
  }

  #[test]
  fn finalise_sets_container_duration_from_avih() {
    let avih = MainAviHeader {
      microsec_per_frame: 41_708,
      max_bytes_per_sec: 0,
      flags: 0,
      total_frames: 240,
      initial_frames: 0,
      streams: 2,
      width: 1920,
      height: 1080,
    };
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(Some(avih), vec![], OdmlInfo::default(), vec![], None, &mut m);
    assert!(m.container.properties.duration.is_some());
    assert_eq!(m.container.properties.is_fragmented, Some(false));
  }

  #[test]
  fn finalise_uses_odml_frame_count_when_set() {
    let avih = MainAviHeader {
      microsec_per_frame: 41_708,
      max_bytes_per_sec: 0,
      flags: 0,
      total_frames: 0,
      initial_frames: 0,
      streams: 0,
      width: 0,
      height: 0,
    };
    let odml = OdmlInfo {
      total_frames: Some(1000),
    };
    let mut m = MediaMetadata::new("clip.avi", 0);
    finalise(Some(avih), vec![], odml, vec![], None, &mut m);
    assert!(m.container.properties.duration.is_some());
  }
}
