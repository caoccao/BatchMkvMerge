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

//! Convert per-stream observations into protocol tracks.
//!
//! Classification precedence mirrors `r_mpeg_ps.cpp::found_new_stream`:
//! a Program Stream Map `stream_type` (PARSER-051) wins, then the
//! private-stream-1 substream id (PARSER-050), then the bare stream id.
//! Codec headers in the depacketised payload supply video dimensions, the
//! AVC-vs-MPEG distinction, and audio parameters (PARSER-052).

use crate::media_metadata::audio::{ac3, dts, mp3, truehd};
use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::elementary::{avc, mpeg_video, vc1};
use crate::media_metadata::io::bit_reader::BitReader;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

/// A stream discovered during the start-code walk, with its depacketised
/// elementary payload for codec-header decoding.
#[derive(Debug, Clone)]
pub struct StreamObservation {
  pub stream_id: u8,
  pub sub_id: Option<u8>,
  pub psm_stream_type: Option<u8>,
  pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Codec {
  kind: TrackKind,
  id: &'static str,
  name: &'static str,
}

/// Program-Stream-Map `stream_type` → codec mapping.
fn codec_from_stream_type(stream_type: u8) -> Option<Codec> {
  Some(match stream_type {
    0x01 => Codec {
      kind: TrackKind::Video,
      id: "V_MPEG1",
      name: "MPEG-1 Video",
    },
    0x02 => Codec {
      kind: TrackKind::Video,
      id: "V_MPEG2",
      name: "MPEG-2 Video",
    },
    0x03 => Codec {
      kind: TrackKind::Audio,
      id: "A_MPEG/L2",
      name: "MPEG-1 Audio",
    },
    0x04 => Codec {
      kind: TrackKind::Audio,
      id: "A_MPEG/L2",
      name: "MPEG-2 Audio",
    },
    0x0F | 0x11 => Codec {
      kind: TrackKind::Audio,
      id: "A_AAC",
      name: "AAC",
    },
    0x10 => Codec {
      kind: TrackKind::Video,
      id: "V_MPEG4/ISO/ASP",
      name: "MPEG-4 Visual",
    },
    0x1B => Codec {
      kind: TrackKind::Video,
      id: "V_MPEG4/ISO/AVC",
      name: "AVC/H.264",
    },
    0x80 => Codec {
      kind: TrackKind::Audio,
      id: "A_PCM/INT/BIG",
      name: "LPCM",
    },
    0x81 => Codec {
      kind: TrackKind::Audio,
      id: "A_AC3",
      name: "AC-3",
    },
    _ => return None,
  })
}

/// `0xBD` private-stream-1 substream classification (PARSER-050).
///
/// PARSER-095: unknown sub-IDs are returned as `None`; mkvtoolnix sets
/// `track->type = '?'` and then drops the track instead of defaulting to AC-3
/// (see `r_mpeg_ps.cpp:1031-1033`).
fn codec_from_sub_id(sub_id: u8) -> Option<Codec> {
  Some(match sub_id {
    0x20..=0x3F => Codec {
      kind: TrackKind::Subtitle,
      id: "S_VOBSUB",
      name: "VobSub",
    },
    0x80..=0x87 | 0xC0..=0xC7 => Codec {
      kind: TrackKind::Audio,
      id: "A_AC3",
      name: "AC-3",
    },
    0x88..=0x9F => Codec {
      kind: TrackKind::Audio,
      id: "A_DTS",
      name: "DTS",
    },
    0xA0..=0xA7 => Codec {
      kind: TrackKind::Audio,
      id: "A_PCM/INT/BIG",
      name: "LPCM",
    },
    0xB0..=0xBF => Codec {
      kind: TrackKind::Audio,
      id: "A_TRUEHD",
      name: "TrueHD",
    },
    _ => return None,
  })
}

/// PARSER-094: stream id `0xFD` is VC-1 (mkvtoolnix `r_mpeg_ps.cpp:1042-1044`).
fn codec_from_bare_id(id: u8) -> Option<Codec> {
  match id {
    0xC0..=0xDF => Some(Codec {
      kind: TrackKind::Audio,
      id: "A_MPEG/L3",
      name: "MPEG-1/2 Audio",
    }),
    0xE0..=0xEF => Some(Codec {
      kind: TrackKind::Video,
      id: "V_MPEG2",
      name: "MPEG-2 Video",
    }),
    0xFD => Some(Codec {
      kind: TrackKind::Video,
      id: "V_VC1",
      name: "VC-1",
    }),
    _ => None,
  }
}

/// Backwards-compatible single-byte classification used by older callers/tests.
pub fn classify_stream_id(id: u8) -> Option<StreamObservation> {
  codec_from_bare_id(id)?;
  Some(StreamObservation {
    stream_id: id,
    sub_id: None,
    psm_stream_type: None,
    payload: Vec::new(),
  })
}

fn resolve_codec(obs: &StreamObservation) -> Option<Codec> {
  if let Some(st) = obs.psm_stream_type {
    return codec_from_stream_type(st);
  }
  if obs.stream_id == 0xBD {
    return obs.sub_id.and_then(codec_from_sub_id);
  }
  codec_from_bare_id(obs.stream_id)
}

/// Decode codec headers from the depacketised payload (PARSER-052). Returns
/// `None` when mkvtoolnix's `new_stream_*` probe would throw and block the
/// stream id (PARSER-306).
fn decode_payload(codec: &mut Codec, payload: &[u8]) -> Option<(Option<VideoTrackProperties>, Option<AudioTrackProperties>)> {
  match codec.kind {
    TrackKind::Video => {
      if codec.id == "V_MPEG4/ISO/AVC" {
        let sps = avc::reader::sps_from_complete_annex_b(payload)?;
        let mut v = VideoTrackProperties::default();
        v.pixel_dimensions = Some(Dimensions2D {
          width: sps.display_width,
          height: sps.display_height,
        });
        return Some((Some(v), None));
      }
      if codec.id == "V_VC1" {
        let seq = vc1::decode_sequence_header(payload)?;
        let mut v = VideoTrackProperties::default();
        v.pixel_dimensions = Some(Dimensions2D {
          width: seq.max_coded_width,
          height: seq.max_coded_height,
        });
        return Some((Some(v), None));
      }
      if matches!(codec.id, "V_MPEG1" | "V_MPEG2") {
        // Bare PS video streams default to MPEG-1/2, but mkvtoolnix first
        // sniffs whether the elementary payload is Annex B AVC.
        if let Some(sps) = avc::reader::sps_from_complete_annex_b(payload) {
          codec.id = "V_MPEG4/ISO/AVC";
          codec.name = "AVC/H.264";
          let mut v = VideoTrackProperties::default();
          v.pixel_dimensions = Some(Dimensions2D {
            width: sps.display_width,
            height: sps.display_height,
          });
          return Some((Some(v), None));
        }
        if !mpeg_video::looks_like_mpeg_video_es(payload) {
          return None;
        }
        let seq = mpeg_video::decode_sequence_header(payload)?;
        if seq.horizontal_size != 0 && seq.vertical_size != 0 {
          let mut v = VideoTrackProperties::default();
          v.pixel_dimensions = Some(Dimensions2D {
            width: seq.horizontal_size,
            height: seq.vertical_size,
          });
          return Some((Some(v), None));
        }
      }
      None
    }
    TrackKind::Audio => {
      let mut a = AudioTrackProperties::default();
      if matches!(codec.id, "A_AC3" | "A_EAC3") {
        let (channels, sample_rate) = ac3::first_frame_params(payload)?;
        a.sampling_frequency = Some(sample_rate as f64);
        a.channels = Some(channels);
      } else if codec.id == "A_DTS" {
        // PARSER-176: decode the first DTS header from the accumulated
        // payload (`r_mpeg_ps.cpp:820-844`).
        let (channels, sample_rate, _bits) = dts::first_header_params(payload)?;
        a.channels = Some(channels);
        a.sampling_frequency = Some(sample_rate as f64);
      } else if codec.id == "A_TRUEHD" {
        // PARSER-176: scan TrueHD frames for the first non-AC-3 sync frame
        // (`r_mpeg_ps.cpp:846-884`).  Embedded AC-3 frames are skipped.
        let mut found = false;
        for frame in truehd::parse_frames(payload) {
          if frame.frame_type == truehd::FrameType::Sync && frame.codec != truehd::Codec::Ac3 {
            a.channels = Some(frame.channels);
            a.sampling_frequency = Some(frame.sampling_rate as f64);
            found = true;
            break;
          }
        }
        if !found {
          return None;
        }
      } else if codec.id == "A_PCM/INT/BIG" {
        // PARSER-176: DVD-VOB LPCM header (`new_stream_a_pcm`,
        // `r_mpeg_ps.cpp:886-910`).  NB: this layout differs from BD-TS LPCM.
        let (channels, sample_rate, bits) = decode_lpcm_header(payload)?;
        a.channels = Some(channels);
        a.sampling_frequency = Some(sample_rate as f64);
        a.bit_depth = Some(bits);
      } else if codec.id.starts_with("A_MPEG") {
        // PARSER-252: mkvtoolnix's `new_stream_a_mpeg` decodes a single MPEG
        // audio frame header (`find_mp3_header`, `r_mpeg_ps.cpp`) and replaces
        // the codec with `header.get_codec()`, so the stream-id / PSM default
        // (A_MPEG/L3 or A_MPEG/L2) is corrected to the actual Layer I / II /
        // III.  Use one header (not two) so a short bounded probe that
        // mkvtoolnix accepts is not rejected.
        let (_off, h) = mp3::find_consecutive_mp3_headers(payload, 1)?;
        a.sampling_frequency = Some(h.sampling_frequency as f64);
        a.channels = Some(h.channels);
        let (id, name) = mp3::codec_for_layer(h.layer);
        codec.id = id;
        codec.name = name;
      } else {
        return None;
      }
      Some((None, Some(a)))
    }
    TrackKind::Subtitle => Some((None, None)),
    _ => None,
  }
}

/// Decode the DVD-VOB LPCM header (`new_stream_a_pcm`,
/// `r_mpeg_ps.cpp:886-910`).  Returns `(channels, sample_rate, bits_per_sample)`
/// or `None` when the bit reader underruns or `bits_per_sample == 28` (which
/// mkvtoolnix rejects via `throw false`).
fn decode_lpcm_header(payload: &[u8]) -> Option<(u32, u32, u32)> {
  const LPCM_FREQUENCY_TABLE: [u32; 4] = [48000, 96000, 44100, 32000];
  let mut bc = BitReader::new(payload);
  bc.skip_bits(8).ok()?; // emphasis(1), muse(1), reserved(1), frame number(5)
  let bits_per_sample = 16 + (bc.read_bits(2).ok()? as u32) * 4;
  let sample_rate = LPCM_FREQUENCY_TABLE[bc.read_bits(2).ok()? as usize];
  bc.skip_bits(1).ok()?; // reserved
  let channels = (bc.read_bits(3).ok()? as u32) + 1;
  if bits_per_sample == 28 {
    return None;
  }
  Some((channels, sample_rate, bits_per_sample))
}

pub fn finalise(observations: Vec<StreamObservation>, out: &mut MediaMetadata) {
  out.container.format = ContainerFormat::MpegPs;
  out.container.recognized = true;
  out.container.supported = true;
  out.container.properties.is_fragmented = Some(false);

  let mut prepared = Vec::new();
  for obs in observations {
    let Some(mut codec) = resolve_codec(&obs) else {
      continue;
    };
    let track_type = match codec.kind {
      TrackKind::Video => TrackType::Video,
      TrackKind::Audio => TrackType::Audio,
      TrackKind::Subtitle => TrackType::Subtitles,
      _ => continue,
    };
    let Some((video, audio)) = decode_payload(&mut codec, &obs.payload) else {
      continue;
    };
    prepared.push((obs, codec, track_type, video, audio));
  }

  // PARSER-307: mkvtoolnix sorts tracks before identification by type bucket
  // and encoded stream id, then uses that sorted order for the displayed track
  // ids.
  prepared.sort_by_key(|(obs, _codec, track_type, _, _)| (track_sort_bucket(*track_type), encoded_stream_id(obs)));

  for (idx, (obs, codec, track_type, video, audio)) in prepared.into_iter().enumerate() {
    let mut common = CommonTrackProperties::default();
    // PARSER-175: `number` encodes stream identity, not a 1-based index.
    // mkvtoolnix sets `number = (sub_id << 32) | stream_id`
    // (`r_mpeg_ps.cpp:1406-1408`) while exposing stream_id / sub_stream_id
    // separately; the compact 0-based `idx` stays on `Track.id`.
    common.number = Some(((obs.sub_id.unwrap_or(0) as u64) << 32) | (obs.stream_id as u64));
    common.stream_id = Some(obs.stream_id as u32);
    if let Some(sub) = obs.sub_id {
      common.sub_stream_id = Some(sub as u32);
    }
    let mut properties = TrackProperties {
      common,
      ..TrackProperties::default()
    };
    match track_type {
      TrackType::Video => properties.video = Some(video.unwrap_or_default()),
      TrackType::Audio => properties.audio = Some(audio.unwrap_or_default()),
      TrackType::Subtitles => {
        // PARSER-096: VobSub on private-stream-1 (sub-id 0x20..=0x3F).
        properties.subtitle = Some(SubtitleTrackProperties {
          text_subtitles: false,
          encoding: None,
          variant: Some("VobSub".to_string()),
          teletext_page: None,
        });
      }
      _ => {}
    }
    out.tracks.push(Track {
      id: idx as i64,
      track_type,
      codec: CodecInfo {
        id: codec.id.to_string(),
        name: Some(codec.name.to_string()),
        codec_private: None,
      },
      properties,
    });
  }
}

fn track_sort_bucket(track_type: TrackType) -> u32 {
  match track_type {
    TrackType::Video => 0,
    TrackType::Audio => 1,
    TrackType::Subtitles => 2,
    _ => 3,
  }
}

fn encoded_stream_id(obs: &StreamObservation) -> u32 {
  ((obs.stream_id as u32) << 8) | (obs.sub_id.unwrap_or(0) as u32)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::elementary::mpeg_video;

  fn obs(stream_id: u8, sub_id: Option<u8>, psm: Option<u8>) -> StreamObservation {
    StreamObservation {
      stream_id,
      sub_id,
      psm_stream_type: psm,
      payload: Vec::new(),
    }
  }

  fn obs_payload(stream_id: u8, sub_id: Option<u8>, psm: Option<u8>, payload: Vec<u8>) -> StreamObservation {
    StreamObservation {
      stream_id,
      sub_id,
      psm_stream_type: psm,
      payload,
    }
  }

  fn video_payload() -> Vec<u8> {
    mpeg_video::build_probe_stream(720, 480, 4)
  }

  fn mpeg_audio_payload() -> Vec<u8> {
    mp3::build_mp3_frame_v1(128, 44_100, false)
  }

  fn lpcm_payload() -> Vec<u8> {
    vec![0x00u8, 0b10_00_0_101]
  }

  #[test]
  fn bare_video_and_audio_ids() {
    assert_eq!(resolve_codec(&obs(0xE0, None, None)).unwrap().kind, TrackKind::Video);
    assert_eq!(resolve_codec(&obs(0xC0, None, None)).unwrap().kind, TrackKind::Audio);
    assert!(resolve_codec(&obs(0x42, None, None)).is_none());
  }

  #[test]
  fn private_stream_1_substreams_classified() {
    assert_eq!(resolve_codec(&obs(0xBD, Some(0x20), None)).unwrap().id, "S_VOBSUB");
    assert_eq!(resolve_codec(&obs(0xBD, Some(0x80), None)).unwrap().id, "A_AC3");
    assert_eq!(resolve_codec(&obs(0xBD, Some(0x88), None)).unwrap().id, "A_DTS");
    assert_eq!(resolve_codec(&obs(0xBD, Some(0xA0), None)).unwrap().id, "A_PCM/INT/BIG");
    assert_eq!(resolve_codec(&obs(0xBD, Some(0xB1), None)).unwrap().id, "A_TRUEHD");
  }

  #[test]
  fn psm_stream_type_wins() {
    let c = resolve_codec(&obs(0xE0, None, Some(0x1B))).unwrap();
    assert_eq!(c.id, "V_MPEG4/ISO/AVC");
    let a = resolve_codec(&obs(0xC0, None, Some(0x81))).unwrap();
    assert_eq!(a.id, "A_AC3");
  }

  #[test]
  fn unsupported_psm_stream_type_drops_instead_of_falling_back() {
    assert!(resolve_codec(&obs(0xE0, None, Some(0x24))).is_none());
    assert!(resolve_codec(&obs(0xC0, None, Some(0x82))).is_none());
  }

  #[test]
  fn finalise_emits_tracks_and_sets_container() {
    let mut m = MediaMetadata::new("clip.mpg", 0);
    finalise(
      vec![
        obs_payload(0xE0, None, None, video_payload()),
        obs_payload(0xC0, None, None, mpeg_audio_payload()),
      ],
      &mut m,
    );
    assert_eq!(m.container.format, ContainerFormat::MpegPs);
    assert_eq!(m.tracks.len(), 2);
    assert_eq!(m.tracks[0].track_type, TrackType::Video);
    assert_eq!(m.tracks[1].track_type, TrackType::Audio);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0xE0));
  }

  // ---- PARSER-094: VC-1 stream id 0xFD --------------------------------

  #[test]
  fn vc1_stream_id_classified_as_video() {
    let c = resolve_codec(&obs(0xFD, None, None)).unwrap();
    assert_eq!(c.id, "V_VC1");
    assert_eq!(c.kind, TrackKind::Video);
  }

  // ---- PARSER-095: unknown sub-IDs are dropped ------------------------

  #[test]
  fn unknown_private_substream_is_not_classified() {
    // 0x40 / 0x70 / 0xD0 are not in any documented sub-id range.
    assert!(resolve_codec(&obs(0xBD, Some(0x40), None)).is_none());
    assert!(resolve_codec(&obs(0xBD, Some(0x70), None)).is_none());
    assert!(resolve_codec(&obs(0xBD, Some(0xD0), None)).is_none());
  }

  // ---- PARSER-096: VobSub subtitle props ------------------------------

  #[test]
  fn vobsub_subtitle_track_has_subtitle_props() {
    let mut m = MediaMetadata::new("clip.vob", 0);
    finalise(vec![obs(0xBD, Some(0x20), None)], &mut m);
    assert_eq!(m.tracks.len(), 1);
    let t = &m.tracks[0];
    assert_eq!(t.track_type, TrackType::Subtitles);
    let sub = t.properties.subtitle.as_ref().unwrap();
    assert!(!sub.text_subtitles);
    assert_eq!(sub.variant.as_deref(), Some("VobSub"));
    assert_eq!(t.properties.common.stream_id, Some(0xBD));
    assert_eq!(t.properties.common.sub_stream_id, Some(0x20));
  }

  // ---- PARSER-175: number encodes stream identity ---------------------

  #[test]
  fn number_encodes_stream_and_sub_id() {
    let mut m = MediaMetadata::new("clip.vob", 0);
    finalise(
      vec![
        obs_payload(0xE0, None, None, video_payload()),
        obs_payload(0xBD, Some(0xA0), None, lpcm_payload()),
      ],
      &mut m,
    );
    // Bare video stream: sub_id defaults to 0 → number == stream_id.
    assert_eq!(m.tracks[0].properties.common.number, Some(0xE0));
    assert_eq!(m.tracks[0].id, 0);
    // Private-stream-1 LPCM: number == (sub_id << 32) | stream_id.
    assert_eq!(m.tracks[1].properties.common.number, Some((0xA0u64 << 32) | 0xBD));
    assert_eq!(m.tracks[1].properties.common.stream_id, Some(0xBD));
    assert_eq!(m.tracks[1].properties.common.sub_stream_id, Some(0xA0));
    assert_eq!(m.tracks[1].id, 1);
  }

  // ---- PARSER-176: DTS / TrueHD / LPCM payload probing ----------------

  #[test]
  fn dts_payload_decodes_channels_and_rate() {
    // Build a minimal DTS core frame via the audio::dts test helper so the
    // decode path matches the real header layout.  amode 6 = 3 channels,
    // sfreq idx 13 = 48000.
    let buf = dts::build_dts_core_frame(6, 13);
    let mut m = MediaMetadata::new("clip.vob", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xBD,
        sub_id: Some(0x88),
        psm_stream_type: None,
        payload: buf,
      }],
      &mut m,
    );
    assert_eq!(m.tracks.len(), 1);
    let t = &m.tracks[0];
    assert_eq!(t.codec.id, "A_DTS");
    let a = t.properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48_000.0));
    assert_eq!(a.channels, Some(3));
  }

  #[test]
  fn lpcm_decode_header_helper() {
    // emphasis/muse/reserved/frame-number byte, then bps=24, freq=48000, ch=6.
    let payload = [0x00u8, 0b10_00_0_101];
    let (channels, sample_rate, bits) = decode_lpcm_header(&payload).unwrap();
    assert_eq!(channels, 6);
    assert_eq!(sample_rate, 48_000);
    assert_eq!(bits, 24);
  }

  #[test]
  fn lpcm_decode_header_rejects_28_bit() {
    // bps field = 0b11 → 16 + 3*4 = 28 → mkvtoolnix rejects (throw false).
    let payload = [0x00u8, 0b11_00_0_001];
    assert!(decode_lpcm_header(&payload).is_none());
  }

  #[test]
  fn lpcm_decode_header_underrun_returns_none() {
    assert!(decode_lpcm_header(&[0x00]).is_none());
  }

  #[test]
  fn lpcm_payload_through_finalise() {
    let payload = vec![0x00u8, 0b01_10_0_001]; // bps=20, freq=44100, ch=2
    let mut m = MediaMetadata::new("clip.vob", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xBD,
        sub_id: Some(0xA0),
        psm_stream_type: None,
        payload,
      }],
      &mut m,
    );
    let a = m.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
    assert_eq!(a.bit_depth, Some(20));
  }

  #[test]
  fn truehd_payload_decodes_first_sync_frame() {
    // rate_bits 0 → 48000; chanmap bit 0 set → 2 channels (CHANNEL_COUNT[0]).
    let buf = truehd::build_truehd_frame(0, 0b1);
    let mut m = MediaMetadata::new("clip.vob", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xBD,
        sub_id: Some(0xB1),
        psm_stream_type: None,
        payload: buf,
      }],
      &mut m,
    );
    assert_eq!(m.tracks.len(), 1);
    let t = &m.tracks[0];
    assert_eq!(t.codec.id, "A_TRUEHD");
    let a = t.properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(48_000.0));
    assert_eq!(a.channels, Some(2));
  }

  // ---- PARSER-252: MPEG audio layer specialization --------------------

  #[test]
  fn bare_mpeg_audio_relabelled_to_probed_layer() {
    // A bare audio stream id (0xC0) defaults to A_MPEG/L3, but a Layer II
    // payload must be relabelled to A_MPEG/L2 from a single decoded header.
    let payload = mp3::build_mp3_frame(1, 2, 128, 44100, false);
    let mut m = MediaMetadata::new("clip.mpg", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xC0,
        sub_id: None,
        psm_stream_type: None,
        payload,
      }],
      &mut m,
    );
    assert_eq!(m.tracks.len(), 1);
    let t = &m.tracks[0];
    assert_eq!(t.codec.id, "A_MPEG/L2");
    assert_eq!(t.codec.name.as_deref(), Some("MP2"));
    let a = t.properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  #[test]
  fn psm_mpeg_audio_relabelled_to_layer_three() {
    // PSM stream_type 0x04 defaults to A_MPEG/L2, but a Layer III payload is
    // relabelled to A_MPEG/L3 from a single decoded header.
    let payload = mp3::build_mp3_frame_v1(128, 44100, false);
    let mut m = MediaMetadata::new("clip.mpg", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xC0,
        sub_id: None,
        psm_stream_type: Some(0x04),
        payload,
      }],
      &mut m,
    );
    assert_eq!(m.tracks[0].codec.id, "A_MPEG/L3");
    assert_eq!(m.tracks[0].codec.name.as_deref(), Some("MP3"));
  }

  #[test]
  fn mpeg_audio_without_a_header_is_dropped() {
    // PARSER-306: no decodable frame means mkvtoolnix blocks the stream id.
    let mut m = MediaMetadata::new("clip.mpg", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xC0,
        sub_id: None,
        psm_stream_type: None,
        payload: vec![0u8; 16],
      }],
      &mut m,
    );
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn mpeg_video_dimensions_decoded() {
    let mut m = MediaMetadata::new("c.mpg", 0);
    finalise(
      vec![StreamObservation {
        stream_id: 0xE0,
        sub_id: None,
        psm_stream_type: None,
        payload: video_payload(),
      }],
      &mut m,
    );
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.unwrap().width, 720);
    assert_eq!(v.pixel_dimensions.unwrap().height, 480);
  }

  #[test]
  fn isolated_mpeg_sequence_header_is_dropped() {
    // PARSER-347: a bare sequence header exposes dimensions but is not enough
    // evidence for mkvtoolnix's video probe; require picture + slice state.
    let mut m = MediaMetadata::new("weak.mpg", 0);
    finalise(vec![obs_payload(0xE0, None, None, mpeg_video::build_sequence_header(720, 480, 4))], &mut m);
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn invalid_codec_probes_are_dropped() {
    // PARSER-306: stream ids or PSM entries alone are not enough to create a
    // track; the codec-specific payload probe must validate.
    let mut m = MediaMetadata::new("bad.mpg", 0);
    finalise(
      vec![
        obs_payload(0xE0, None, None, vec![0u8; 16]),
        obs_payload(0xC0, None, None, vec![0u8; 16]),
        obs_payload(0xBD, Some(0x80), None, vec![0u8; 16]),
        obs_payload(0xBD, Some(0x88), None, vec![0u8; 16]),
        obs_payload(0xBD, Some(0xA0), None, vec![0u8; 1]),
        obs_payload(0xBD, Some(0xB0), None, vec![0u8; 16]),
        obs_payload(0xFD, None, None, vec![0u8; 16]),
      ],
      &mut m,
    );
    assert!(m.tracks.is_empty());
  }

  #[test]
  fn finalise_sorts_by_type_bucket_and_encoded_id() {
    let mut m = MediaMetadata::new("sorted.mpg", 0);
    finalise(
      vec![
        obs_payload(0xC1, None, None, mpeg_audio_payload()),
        obs_payload(0xE1, None, None, video_payload()),
        obs_payload(0xC0, None, None, mpeg_audio_payload()),
        obs_payload(0xE0, None, None, video_payload()),
        obs_payload(0xBD, Some(0x20), None, Vec::new()),
      ],
      &mut m,
    );
    assert_eq!(m.tracks.len(), 5);
    assert_eq!(m.tracks[0].properties.common.stream_id, Some(0xE0));
    assert_eq!(m.tracks[0].id, 0);
    assert_eq!(m.tracks[1].properties.common.stream_id, Some(0xE1));
    assert_eq!(m.tracks[1].id, 1);
    assert_eq!(m.tracks[2].properties.common.stream_id, Some(0xC0));
    assert_eq!(m.tracks[2].id, 2);
    assert_eq!(m.tracks[3].properties.common.stream_id, Some(0xC1));
    assert_eq!(m.tracks[3].id, 3);
    assert_eq!(m.tracks[4].track_type, TrackType::Subtitles);
    assert_eq!(m.tracks[4].id, 4);
  }
}
