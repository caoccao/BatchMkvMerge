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

//! PARSER-177: first-sample track verification.
//!
//! Port of mkvtoolnix's `qtmp4_demuxer_c::verify_*` family
//! (`r_qtmp4.cpp:3660-3853`).  After the `moov` walk, mkvmerge verifies every
//! track and DROPS any it cannot use; for AVC tracks missing an `avcC` it
//! salvages a configuration record from the first frames
//! (`derive_track_params_from_avc_bitstream`).  This pass runs during
//! `read_headers` (it needs the `FileSource` for bounded first-sample reads,
//! which `identify::finalise` has no access to).
//!
//! All reads are bounded (≤ 16 KiB per track) and deadline-checked — we only
//! locate and read the FIRST sample, never demux.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track::TrackType;
use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoCodecConfig};

use super::codec_specific::hex_encode;
use super::identify;
use super::moov::MoovBuilder;
use super::moov::hdlr::Handler;
use super::moov::trak::TrackBuilder;

/// Hard cap on any single first-sample read.  Mirrors the largest buffer the
/// C++ verification path requests (`read_first_bytes(16384)`).
const MAX_FIRST_BYTES: u64 = 16384;

/// Run the verification pass over every track in `moov`, setting
/// `builder.probe_failed = true` for tracks mkvtoolnix would reject.
pub fn verify_tracks(src: &mut FileSource, deadline: &Deadline, moov: &mut MoovBuilder) -> Result<(), ParseError> {
  for builder in &mut moov.tracks {
    deadline.check("mp4::verify")?;
    if builder.probe_failed || builder.media_invalid {
      continue;
    }
    let track_type = classify(builder);
    match track_type {
      Some(TrackType::Audio) => verify_audio(src, deadline, builder)?,
      Some(TrackType::Video) => verify_video(src, deadline, builder)?,
      Some(TrackType::Subtitles) => verify_subtitles(builder),
      // Buttons / unknown / metadata handlers are dropped later by
      // `build_track`; no first-sample gate applies.
      _ => {}
    }
  }
  Ok(())
}

/// Classify a builder's track type from its handler, mirroring the
/// `handler.classify()` step `build_track` performs.  Returns `None` for
/// handler-less / metadata tracks (which `build_track` already drops).
fn classify(builder: &TrackBuilder) -> Option<TrackType> {
  let handler_type = builder.handler_type?;
  let handler = Handler {
    handler_type,
    name: String::new(),
  };
  if handler.is_metadata_handler() {
    return None;
  }
  Some(handler.classify())
}

// ----- audio --------------------------------------------------------------

/// Port of `qtmp4_demuxer_c::verify_audio_parameters` (r_qtmp4.cpp:3660-3702).
/// First recovers missing channels / sample rate from the first-frame bitstream
/// for MP2/MP3, AC-3 and DTS (the `derive_track_params_from_*` family); then the
/// generic channels/rate==0 drop; then the ALAC and DTS verification gates that
/// drop tracks with a broken / missing codec configuration.
fn verify_audio(src: &mut FileSource, deadline: &Deadline, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  let codec = identify::effective_codec_id(builder);

  // --- first-frame parameter recovery (r_qtmp4.cpp:3662-3669) ---
  // These mirror the `derive_track_params_from_*` helpers, which are `void` in
  // mkvtoolnix: a missing header does NOT drop the track here — the generic
  // gate below decides.  Only DTS has an additional explicit verify gate.
  if is_mp2_mp3(&codec) {
    // r_qtmp4.cpp:3552-3565: read_first_bytes(64) → find/decode MP3 header.
    let buf = read_first_bytes(src, deadline, builder, 64)?;
    if let Some((channels, sample_rate)) = buf
      .as_deref()
      .and_then(crate::media_metadata::audio::mp3::first_header_params)
    {
      recover_audio_params(builder, channels, sample_rate, 0);
    }
  } else if is_ac3(&codec) {
    // r_qtmp4.cpp:3526-3536: read_first_bytes(64) → find AC-3 frame header.
    let buf = read_first_bytes(src, deadline, builder, 64)?;
    if let Some((channels, sample_rate)) = buf
      .as_deref()
      .and_then(crate::media_metadata::audio::ac3::first_frame_params)
    {
      recover_audio_params(builder, channels, sample_rate, 0);
    }
  } else if is_dts(&codec) {
    // r_qtmp4.cpp:3539-3549: read_first_bytes(16384) → DTS header.
    let buf = read_first_bytes(src, deadline, builder, MAX_FIRST_BYTES)?;
    if let Some((channels, sample_rate, bits)) = buf
      .as_deref()
      .and_then(crate::media_metadata::audio::dts::first_header_params)
    {
      recover_audio_params(builder, channels, sample_rate, bits);
    }
  } else if is_pcm(&codec) && builder.codec_id_str.as_deref() == Some("in24") {
    // QuickTime `in24` sample entries are always 24-bit PCM, regardless of the
    // sample-size placeholder (`verify_audio_parameters`, r_qtmp4.cpp:3682-3684).
    let audio = builder.audio.get_or_insert_with(Default::default);
    audio.bit_depth = Some(24);
  }

  // PARSER-230 / PARSER-231: FLAC / Vorbis / Opus verification
  // (r_qtmp4.cpp:3672-3681).  These return EARLY in mkvtoolnix, bypassing the
  // generic channels/rate gate below.
  if is_flac(&codec) {
    // `verify_audio_parameters`: exactly one private blob (the dfLa metadata).
    if codec_private_len(builder) == 0 {
      builder.probe_failed = true;
    }
    return Ok(());
  }
  if is_vorbis(&codec) {
    // `derive_track_params_from_vorbis_private_data`: unlace the esds
    // DecoderSpecificInfo into exactly three Xiph headers and read the
    // identification header for channels / sample rate.  Invalid → drop.
    if !derive_vorbis_params(builder) {
      builder.probe_failed = true;
    }
    return Ok(());
  }
  if is_opus(&codec) {
    // `derive_track_params_from_opus_private_data`: the OpusHead private blob
    // must be at least 19 bytes.
    if codec_private_len(builder) < 19 {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // AUDIO general (r_qtmp4.cpp:3687-3690): zero channels or zero/absent
  // sampling frequency → broken header → drop.
  let channels = builder.audio.as_ref().and_then(|a| a.channels).unwrap_or(0);
  let rate = builder.audio.as_ref().and_then(|a| a.sampling_frequency).unwrap_or(0.0);
  if channels == 0 || rate == 0.0 {
    builder.probe_failed = true;
    return Ok(());
  }

  // ALAC (r_qtmp4.cpp:3695-3696, 3705-3716): the ALAC magic cookie must be
  // present and large enough to carry the embedded ALACSpecificConfig.
  if is_alac(&codec) {
    if !alac_config_present(builder) {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // DTS (r_qtmp4.cpp:3698-3699, 3719-3731): a real DTS header must be findable
  // in the first frames, else the track is skipped.
  if is_dts(&codec) {
    let buf = read_first_bytes(src, deadline, builder, MAX_FIRST_BYTES)?;
    let has_header = buf
      .as_deref()
      .and_then(crate::media_metadata::audio::dts::first_header_params)
      .is_some();
    if !has_header {
      builder.probe_failed = true;
    }
  }

  Ok(())
}

/// Fill in any audio channel / sample-rate / bit-depth fields that are still
/// missing or zero, mirroring the `derive_track_params_from_*` assignments.
fn recover_audio_params(builder: &mut TrackBuilder, channels: u32, sample_rate: u32, bits: u32) {
  let audio = builder
    .audio
    .get_or_insert_with(crate::media_metadata::model::track_properties_audio::AudioTrackProperties::default);
  if audio.channels.unwrap_or(0) == 0 && channels != 0 {
    audio.channels = Some(channels);
  }
  if audio.sampling_frequency.unwrap_or(0.0) == 0.0 && sample_rate != 0 {
    audio.sampling_frequency = Some(sample_rate as f64);
  }
  if audio.bit_depth.is_none() && bits != 0 {
    audio.bit_depth = Some(bits);
  }
}

/// r_qtmp4.cpp:3705-3716 `verify_alac_audio_parameters`: the stsd must carry an
/// ALAC magic cookie whose embedded `codec_config_t` (24 bytes) is present.
/// mkvtoolnix checks `stsd->get_size() >= stsd_non_priv_struct_size + 12 +
/// sizeof(codec_config_t)`; the `+12` is the `alac` box header (size+type+
/// version/flags) inside the sample entry, and `create_audio_packetizer_alac`
/// (r_qtmp4.cpp:1833-1839) passes only the bytes after that `+12` as the magic
/// cookie.  Since PARSER-201 we store exactly that cookie (the
/// ALACSpecificConfig, FullBox header already stripped), so the codec_private
/// blob must be at least `sizeof(codec_config_t)` = 24 bytes.
fn alac_config_present(builder: &TrackBuilder) -> bool {
  const MIN_ALAC_PRIVATE_BYTES: usize = ALAC_CODEC_CONFIG_SIZE;
  builder
    .codec_private_hex
    .as_ref()
    .map(|hex| (hex.len() / 2) >= MIN_ALAC_PRIVATE_BYTES)
    .unwrap_or(false)
}

/// `sizeof(mtx::alac::codec_config_t)` — the ALACSpecificConfig payload.
const ALAC_CODEC_CONFIG_SIZE: usize = 24;

// ----- video --------------------------------------------------------------

/// r_qtmp4.cpp:3749-3832.  Drop on zero dimensions; require / derive the
/// decoder config for AVC / HEVC / MP4V.
fn verify_video(src: &mut FileSource, deadline: &Deadline, builder: &mut TrackBuilder) -> Result<(), ParseError> {
  // VIDEO general (r_qtmp4.cpp:3754-3757): missing dimensions → drop.
  let dims = builder.video.as_ref().and_then(|v| v.pixel_dimensions);
  let (width, height) = dims.map(|d| (d.width, d.height)).unwrap_or((0, 0));
  if width == 0 || height == 0 {
    builder.probe_failed = true;
    return Ok(());
  }

  let codec = identify::effective_codec_id(builder);

  // MP4V (r_qtmp4.cpp:3818-3832): require the esds decoder config.
  if is_mp4v(&codec) {
    if builder.esds_object_type.is_none() || builder.esds_decoder_specific_len.unwrap_or(0) == 0 {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // AVC (r_qtmp4.cpp:3771-3806): keep if an avcC was parsed (video codec
  // config from avcC, priv ≥ 4); else try to derive from the bitstream.
  if is_avc(&codec) {
    if has_decoder_config(builder, 4) {
      return Ok(());
    }
    if derive_avc_from_bitstream(src, deadline, builder)? {
      return Ok(());
    }
    builder.probe_failed = true;
    return Ok(());
  }

  // HEVC (r_qtmp4.cpp:3808-3816): require an hvcC (config ≥ 23 bytes); no
  // bitstream derivation.
  if is_hevc(&codec) {
    if !has_decoder_config(builder, 23) {
      builder.probe_failed = true;
    }
    return Ok(());
  }

  // Other video codecs (AV1, VP9, …): no extra gate beyond dimensions.
  Ok(())
}

/// True when the track carries a parsed decoder configuration record whose raw
/// bytes are at least `min_len` bytes long (mkvtoolnix's `priv[0]->get_size()`
/// check).  The avcC / hvcC parsers populate `video_codec_config.raw_hex`.
fn has_decoder_config(builder: &TrackBuilder, min_len: usize) -> bool {
  builder
    .video_codec_config
    .as_ref()
    .and_then(|c| c.raw_hex.as_ref())
    .map(|hex| (hex.len() / 2) >= min_len)
    .unwrap_or(false)
}

/// r_qtmp4.cpp:3771-3794 `derive_track_params_from_avc_bitstream`: read the
/// first 10 000 bytes, split NALs, and build an avcC from the SPS/PPS.  Sets
/// the derived `video_codec_config` + `codec_private` + pixel dims and returns
/// `true` on success.
fn derive_avc_from_bitstream(
  src: &mut FileSource,
  deadline: &Deadline,
  builder: &mut TrackBuilder,
) -> Result<bool, ParseError> {
  use crate::media_metadata::elementary::avc::nal::{self, NAL_UNIT_TYPE_PPS, NAL_UNIT_TYPE_SPS};
  use crate::media_metadata::elementary::avc::sps;

  let buf = match read_first_bytes(src, deadline, builder, 10_000)? {
    Some(b) => b,
    None => return Ok(false),
  };
  let units = nal::split_nal_units(&buf);
  let mut sps_unit = None;
  let mut pps_unit = None;
  let mut decoded_sps = None;
  for unit in &units {
    if unit.nal_unit_type == NAL_UNIT_TYPE_SPS && sps_unit.is_none() {
      let rbsp = nal::strip_emulation_prevention(unit.payload);
      if let Ok(parsed) = sps::parse(&rbsp) {
        decoded_sps = Some(parsed);
        sps_unit = Some(*unit);
      }
    } else if unit.nal_unit_type == NAL_UNIT_TYPE_PPS && pps_unit.is_none() {
      pps_unit = Some(*unit);
    }
  }
  let (sps_unit, sps) = match (sps_unit, decoded_sps) {
    (Some(u), Some(s)) => (u, s),
    _ => return Ok(false),
  };

  // Build a minimal avcC.  PPS is included when present (mirrors the es_parser
  // configuration record); without it we still emit a usable SPS-only record
  // (≥ 4 bytes), which mkvtoolnix accepts via the `4 <= priv[0]->get_size()`
  // check.
  let avcc = build_avcc(&sps, sps_unit, pps_unit);

  let cfg = VideoCodecConfig {
    profile_idc: Some(sps.profile_idc as u32),
    profile_name: Some(sps::format_profile(sps.profile_idc).to_string()),
    level_idc: Some(sps.level_idc as u32),
    level_name: Some(sps::format_level(sps.level_idc)),
    bit_depth_luma: Some(sps.bit_depth_luma as u32),
    bit_depth_chroma: Some(sps.bit_depth_chroma as u32),
    coded_dimensions: Some(Dimensions2D {
      width: sps.coded_width,
      height: sps.coded_height,
    }),
    raw_hex: Some(hex_encode(&avcc)),
    is_elementary_stream: Some(false),
    ..VideoCodecConfig::default()
  };
  builder.codec_private_hex = Some(hex_encode(&avcc));
  builder.video_codec_config = Some(cfg);
  // Refine pixel/display dims from the SPS (mkvtoolnix uses the derived
  // resolution when the sample entry's were unusable).
  if let Some(video) = builder.video.as_mut() {
    let display = Dimensions2D {
      width: sps.display_width,
      height: sps.display_height,
    };
    video.pixel_dimensions = Some(display);
    if video.display_dimensions.is_none() {
      video.display_dimensions = Some(display);
    }
    if let Some(cfg) = builder.video_codec_config.clone() {
      video.codec_config = Some(cfg);
    }
  }
  Ok(true)
}

/// Assemble an `avcC` configuration record from a decoded SPS and the raw
/// SPS/PPS NAL bytes.  `lengthSizeMinusOne` defaults to 3 (4-byte NAL length).
fn build_avcc(
  sps: &crate::media_metadata::elementary::avc::sps::AvcSps,
  sps_unit: crate::media_metadata::elementary::avc::nal::NalUnit<'_>,
  pps_unit: Option<crate::media_metadata::elementary::avc::nal::NalUnit<'_>>,
) -> Vec<u8> {
  let sps_bytes = nal_bytes(sps_unit);
  let mut out = Vec::new();
  out.push(1); // configurationVersion
  out.push(sps.profile_idc);
  // PARSER-259: preserve the SPS constraint-set / profile-compatibility byte
  // (`buffer[2] = sps.profile_compat`, `../mkvtoolnix/src/common/avc/avcc.cpp:134`).
  out.push(sps.profile_compat);
  out.push(sps.level_idc);
  out.push(0xff); // 6 reserved bits + lengthSizeMinusOne = 3
  out.push(0xe1); // 3 reserved bits + numOfSequenceParameterSets = 1
  out.extend_from_slice(&(sps_bytes.len() as u16).to_be_bytes());
  out.extend_from_slice(&sps_bytes);
  match pps_unit {
    Some(pps) => {
      let pps_bytes = nal_bytes(pps);
      out.push(1); // numOfPictureParameterSets
      out.extend_from_slice(&(pps_bytes.len() as u16).to_be_bytes());
      out.extend_from_slice(&pps_bytes);
    }
    None => {
      out.push(0); // numOfPictureParameterSets
    }
  }
  out
}

fn nal_bytes(unit: crate::media_metadata::elementary::avc::nal::NalUnit<'_>) -> Vec<u8> {
  let mut bytes = Vec::with_capacity(unit.payload.len() + 1);
  bytes.push((unit.nal_ref_idc << 5) | unit.nal_unit_type);
  bytes.extend_from_slice(unit.payload);
  bytes
}

// ----- subtitles ----------------------------------------------------------

/// r_qtmp4.cpp:3835-3853.  VobSub requires an esds decoder config ≥ 64 bytes;
/// tx3g/text always verify.  Image / unknown subtitle codecs whose private
/// data we preserved (PARSER-178) are kept — mkvtoolnix only has explicit
/// gates for S_VOBSUB and S_TX3G, and our `build_track` already emits the
/// remaining types as image subtitles.
fn verify_subtitles(builder: &mut TrackBuilder) {
  let codec = identify::effective_codec_id(builder);
  if codec == "S_VOBSUB" && builder.esds_decoder_specific_len.unwrap_or(0) < 64 {
    builder.probe_failed = true;
  }
}

// ----- bounded first-sample read ------------------------------------------

/// Port of `qtmp4_demuxer_c::read_first_bytes(num_bytes)`
/// (`r_qtmp4.cpp:2881-2906`): iterate the (bounded) sample index, seeking to
/// EACH sample's file offset and reading `min(remaining, sample.size)` bytes,
/// until `num_bytes` is collected.  `max` is clamped to [`MAX_FIRST_BYTES`].
/// Returns `None` when no samples can be located or a read comes up short.
///
/// PARSER-183: this now spans MULTIPLE samples — not just sample 0 — so AVC
/// salvage / DTS probing / first-frame derivation can collect the full window
/// across an index of small samples, exactly like mkvtoolnix.  When the
/// reconstructed index is empty we fall back to the single-sample fields so
/// older fixtures (and tracks with only `stco` + `stsz`) still resolve.
fn read_first_bytes(
  src: &mut FileSource,
  deadline: &Deadline,
  builder: &TrackBuilder,
  max: u64,
) -> Result<Option<Vec<u8>>, ParseError> {
  deadline.check("mp4::read_first_bytes")?;
  let want_total = max.min(MAX_FIRST_BYTES);
  if want_total == 0 {
    return Ok(None);
  }

  // Prefer the reconstructed multi-sample index; fall back to sample 0.
  let index: Vec<(u64, u64)> = if !builder.first_samples.is_empty() {
    builder.first_samples.clone()
  } else {
    match builder.first_sample_file_offset {
      Some(off) => vec![(off, builder.first_sample_size.unwrap_or(0))],
      None => return Ok(None),
    }
  };

  let len = src.length();
  let mut buf: Vec<u8> = Vec::with_capacity(want_total as usize);
  for (offset, size) in index {
    let remaining = want_total - buf.len() as u64;
    if remaining == 0 {
      break;
    }
    deadline.check("mp4::read_first_bytes")?;
    // mkvtoolnix reads min(remaining, sample.size); a zero size means the size
    // table ran short — read up to `remaining` to mirror the unbounded buffer.
    let mut want = remaining;
    if size > 0 {
      want = want.min(size);
    }
    if let Some(file_len) = len {
      if offset >= file_len {
        break;
      }
      want = want.min(file_len - offset);
    }
    if want == 0 {
      break;
    }
    src.seek_to(offset)?;
    let mut chunk = vec![0u8; want as usize];
    let read = src.read_at_most(&mut chunk)?;
    chunk.truncate(read);
    if chunk.is_empty() {
      break;
    }
    buf.extend_from_slice(&chunk);
  }

  if buf.is_empty() {
    return Ok(None);
  }
  Ok(Some(buf))
}

// ----- codec-family predicates --------------------------------------------

fn is_avc(codec: &str) -> bool {
  matches!(codec, "avc1" | "avc3" | "V_MPEG4/ISO/AVC")
}

fn is_hevc(codec: &str) -> bool {
  matches!(codec, "hvc1" | "hev1" | "V_MPEGH/ISO/HEVC")
}

fn is_mp4v(codec: &str) -> bool {
  matches!(codec, "mp4v" | "V_MPEG4/ISO/ASP")
}

fn is_dts(codec: &str) -> bool {
  matches!(codec, "A_DTS" | "dtsc" | "dtsh" | "dtse" | "dts ")
}

/// MP2 / MP3 — both the Matroska ids the esds object-type maps to and the
/// raw FOURCCs QuickTime uses (`.mp3`).
fn is_mp2_mp3(codec: &str) -> bool {
  matches!(codec, "A_MPEG/L3" | "A_MPEG/L2" | ".mp3" | "mp3 ")
}

/// AC-3 (and the QuickTime `ac-3` / `sac3` FOURCCs).
fn is_ac3(codec: &str) -> bool {
  matches!(codec, "A_AC3" | "ac-3" | "ac3 " | "sac3")
}

/// Apple Lossless — the `alac` FOURCC (and its Matroska id).
fn is_alac(codec: &str) -> bool {
  matches!(codec, "alac" | "A_ALAC")
}

/// Opus — the `Opus` sample-entry FOURCC (any case) or its Matroska id.
fn is_opus(codec: &str) -> bool {
  codec.eq_ignore_ascii_case("opus") || codec == "A_OPUS"
}

/// FLAC — the `fLaC` sample-entry FOURCC (any case) or its Matroska id.
fn is_flac(codec: &str) -> bool {
  codec.eq_ignore_ascii_case("flac") || codec == "A_FLAC"
}

/// Vorbis — recognised via the esds objectTypeIndication 0xDD → `A_VORBIS`.
fn is_vorbis(codec: &str) -> bool {
  codec == "A_VORBIS"
}

fn is_pcm(codec: &str) -> bool {
  matches!(codec, "A_PCM/INT/BIG" | "A_PCM/INT/LIT" | "A_PCM/FLOAT/IEEE")
}

/// Byte length of the audio codec-private blob (mkvtoolnix's `priv[0]->get_size()`).
fn codec_private_len(builder: &TrackBuilder) -> usize {
  builder.codec_private_hex.as_deref().map(|h| h.len() / 2).unwrap_or(0)
}

/// Port of `derive_track_params_from_vorbis_private_data` (r_qtmp4.cpp:3585-3623).
/// Unlace the esds DecoderSpecificInfo into the three Xiph-laced Vorbis headers
/// and read channels / sample rate from the identification header.  Returns
/// `false` (track invalid) unless exactly three packets are present and the
/// first parses as a Vorbis identification header (PARSER-230).
fn derive_vorbis_params(builder: &mut TrackBuilder) -> bool {
  let Some(data) = builder.esds_decoder_specific_data.clone() else {
    return false;
  };
  let Some(packets) = unlace_xiph(&data) else {
    return false;
  };
  if packets.len() != 3 {
    return false;
  }
  let Some(meta) = crate::media_metadata::ogg::codecs::vorbis::sniff(packets[0]) else {
    return false;
  };
  let Some(audio_info) = meta.audio else {
    return false;
  };
  let audio = builder
    .audio
    .get_or_insert_with(crate::media_metadata::model::track_properties_audio::AudioTrackProperties::default);
  if let Some(channels) = audio_info.channels {
    audio.channels = Some(channels);
  }
  if let Some(rate) = audio_info.sampling_frequency {
    audio.sampling_frequency = Some(rate);
  }
  true
}

/// Port of `unlace_memory_xiph` (`../mkvtoolnix/src/common/memory.cpp:95-135`).
/// Splits a Xiph-laced buffer (first byte = packet_count − 1, then per-packet
/// sizes as 255-summed bytes, then the packet payloads) into its packets.
/// Returns `None` on any structural inconsistency.
fn unlace_xiph(data: &[u8]) -> Option<Vec<&[u8]>> {
  if data.is_empty() {
    return None;
  }
  let num_blocks = data[0] as usize + 1;
  let mut pos = 1usize;
  let mut sizes: Vec<usize> = Vec::with_capacity(num_blocks);
  let mut last_size = data.len();
  for _ in 0..num_blocks.saturating_sub(1) {
    let mut size = 0usize;
    while pos < data.len() && data[pos] == 255 {
      size += 255;
      pos += 1;
    }
    if pos >= data.len() {
      return None;
    }
    size += data[pos] as usize;
    pos += 1;
    sizes.push(size);
    last_size = last_size.checked_sub(size)?;
  }
  // The final packet runs to the end of the buffer.
  sizes.push(last_size.checked_sub(pos)?);

  let mut blocks: Vec<&[u8]> = Vec::with_capacity(num_blocks);
  for size in sizes {
    let end = pos.checked_add(size)?;
    if end > data.len() {
      return None;
    }
    blocks.push(&data[pos..end]);
    pos = end;
  }
  Some(blocks)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::model::track_properties_audio::AudioTrackProperties;
  use crate::media_metadata::model::track_properties_video::VideoTrackProperties;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn source(bytes: Vec<u8>) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes))
  }

  fn video_builder(codec: &str, width: u32, height: u32) -> TrackBuilder {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"vide");
    b.codec_id_str = Some(codec.to_string());
    b.video = Some(VideoTrackProperties {
      pixel_dimensions: Some(Dimensions2D { width, height }),
      ..Default::default()
    });
    b
  }

  fn audio_builder(codec: &str, channels: u32, rate: f64) -> TrackBuilder {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"soun");
    b.codec_id_str = Some(codec.to_string());
    b.audio = Some(AudioTrackProperties {
      channels: if channels != 0 { Some(channels) } else { None },
      sampling_frequency: if rate != 0.0 { Some(rate) } else { None },
      ..Default::default()
    });
    b
  }

  fn avcc_raw_hex() -> String {
    // Minimal avcC ≥ 4 bytes.
    hex_encode(&crate::media_metadata::mp4::codec_specific::avcc::build_avcc_payload(
      66,
      30,
      3,
      &[&[0u8; 4]],
      &[&[0u8; 2]],
      None,
    ))
  }

  // r_qtmp4.cpp:3771-3799: avc1 with an avcC (priv ≥ 4) is kept.
  #[test]
  fn avc_with_avcc_kept() {
    let mut b = video_builder("avc1", 1920, 1080);
    b.video_codec_config = Some(VideoCodecConfig {
      raw_hex: Some(avcc_raw_hex()),
      ..Default::default()
    });
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3801 derive_track_params_from_avc_bitstream salvages an avc1
  // that has no avcC but a decodable SPS in the first frames.
  #[test]
  fn avc_without_avcc_salvaged_from_bitstream() {
    let mut b = video_builder("avc1", 1920, 1080);
    // mdat = an Annex B SPS + PPS the AVC SPS parser can decode.
    let es = build_avc_annex_b();
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(es.len() as u64);
    let mut src = source(es);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let cfg = b.video_codec_config.unwrap();
    assert_eq!(cfg.profile_idc, Some(66));
    assert!(b.codec_private_hex.is_some());
  }

  // PARSER-259: the derived avcC preserves the SPS constraint-set /
  // profile-compatibility byte (avcC byte 2), not a hard-coded zero.
  #[test]
  fn avc_salvage_preserves_profile_compat_byte() {
    let mut b = video_builder("avc1", 1920, 1080);
    // SPS NAL with constraints byte 0xC0 (constraint_set0/1 flags set).
    let mut es = vec![0x00, 0x00, 0x00, 0x01, 0x67, 66u8, 0xC0, 40u8];
    es.extend(build_baseline_sps_tail());
    es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE]); // PPS
    es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]); // AUD terminator
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(es.len() as u64);
    let mut src = source(es);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let hex = b.codec_private_hex.unwrap();
    // "01" configurationVersion, "42" profile_idc(66), "c0" profile_compat.
    assert!(hex.starts_with("0142c028"), "avcC prefix was {}", &hex[..8]);
  }

  // r_qtmp4.cpp:3804: avc1 with no avcC and junk first bytes is skipped.
  #[test]
  fn avc_without_avcc_and_junk_dropped() {
    let mut b = video_builder("avc1", 1920, 1080);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0xAAu8; 64]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3810: hev1 without an hvcC is skipped.
  #[test]
  fn hevc_without_hvcc_dropped() {
    let mut b = video_builder("hev1", 3840, 2160);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn hevc_with_hvcc_kept() {
    let mut b = video_builder("hev1", 3840, 2160);
    b.video_codec_config = Some(VideoCodecConfig {
      raw_hex: Some("00".repeat(23)),
      ..Default::default()
    });
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3754-3757: zero dimensions → drop.
  #[test]
  fn video_zero_dimensions_dropped() {
    let mut b = video_builder("av01", 0, 0);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3818-3832: mp4v without an esds decoder config → drop.
  #[test]
  fn mp4v_without_esds_decoder_config_dropped() {
    let mut b = video_builder("mp4v", 720, 480);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn mp4v_with_esds_decoder_config_kept() {
    let mut b = video_builder("mp4v", 720, 480);
    b.esds_object_type = Some(0x20);
    b.esds_decoder_specific_len = Some(10);
    let mut src = source(vec![]);
    verify_video(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // r_qtmp4.cpp:3719-3731: DTS with a real header in the first bytes is kept
  // and gets channels/rate; without a header it is dropped.
  #[test]
  fn dts_with_header_kept_and_specialised() {
    let mut b = audio_builder("A_DTS", 0, 0.0);
    let frame = build_dts_frame();
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(frame.len() as u64);
    let mut src = source(frame);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let a = b.audio.unwrap();
    assert!(a.channels.unwrap() > 0);
    assert!(a.sampling_frequency.unwrap() > 0.0);
  }

  #[test]
  fn dts_without_header_dropped() {
    let mut b = audio_builder("A_DTS", 0, 0.0);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0u8; 64]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // r_qtmp4.cpp:3687-3690: a zero-channel audio track is dropped.
  #[test]
  fn zero_channel_audio_dropped() {
    let mut b = audio_builder("mp4a", 0, 48_000.0);
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn good_audio_kept() {
    let mut b = audio_builder("mp4a", 2, 48_000.0);
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  #[test]
  fn in24_pcm_forces_24_bit_depth() {
    let mut b = audio_builder("in24", 2, 48_000.0);
    b.audio.as_mut().unwrap().bit_depth = Some(16);
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    assert_eq!(b.audio.unwrap().bit_depth, Some(24));
  }

  // ---- PARSER-184: MP2/MP3 + AC-3 first-frame parameter recovery -----------

  /// An mp4a entry whose esds objectTypeIndication is MP3 (0x6B) but whose
  /// sample entry left channels/rate at zero — mkvtoolnix recovers them from
  /// the first frame (r_qtmp4.cpp:3552-3565) instead of dropping the track.
  #[test]
  fn mp3_params_recovered_from_first_frame() {
    let mut b = audio_builder("mp4a", 0, 0.0);
    b.esds_object_type = Some(0x6B); // → A_MPEG/L3
    let frame = crate::media_metadata::audio::mp3::build_mp3_frame_v1(128, 44_100, false);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(frame.len() as u64);
    let mut src = source(frame);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  // mp4a/MP3 with junk first bytes recovers nothing; the generic gate drops it.
  #[test]
  fn mp3_without_frame_dropped_by_generic_gate() {
    let mut b = audio_builder("mp4a", 0, 0.0);
    b.esds_object_type = Some(0x6B);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0x00u8; 64]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // AC-3 (r_qtmp4.cpp:3526-3536): channels/rate recovered from the first frame.
  #[test]
  fn ac3_params_recovered_from_first_frame() {
    let mut b = audio_builder("ac-3", 0, 0.0);
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame(0, 8); // fscod0=48k, acmod2=2ch
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(frame.len() as u64);
    let mut src = source(frame);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48_000.0));
  }

  #[test]
  fn ac3_without_frame_dropped_by_generic_gate() {
    let mut b = audio_builder("ac-3", 0, 0.0);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(64);
    let mut src = source(vec![0x00u8; 64]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // Recovery never clobbers values the sample entry already carried.
  #[test]
  fn recovery_does_not_override_existing_params() {
    let mut b = audio_builder("ac-3", 6, 44_100.0);
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame(0, 8); // would say 2ch/48k
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(frame.len() as u64);
    let mut src = source(frame);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  // ---- PARSER-185: ALAC config payload verification ------------------------

  // r_qtmp4.cpp:3705-3716: a valid ALAC track carries the magic cookie
  // (≥ 24-byte ALACSpecificConfig of codec private, FullBox header already
  // stripped per PARSER-201) and is kept.
  #[test]
  fn alac_with_valid_config_kept() {
    let mut b = audio_builder("alac", 2, 44_100.0);
    // 24-byte ALACSpecificConfig (magic cookie), FullBox header stripped.
    b.codec_private_hex = Some(hex_encode(&vec![0u8; 24]));
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  // A truncated ALAC cookie (< 24-byte ALACSpecificConfig) is dropped even
  // though the sample entry's channels/rate look fine.
  #[test]
  fn alac_with_truncated_config_dropped() {
    let mut b = audio_builder("alac", 2, 44_100.0);
    b.codec_private_hex = Some(hex_encode(&vec![0u8; 20])); // too small
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // A missing ALAC cookie is dropped.
  #[test]
  fn alac_without_config_dropped() {
    let mut b = audio_builder("alac", 2, 44_100.0);
    b.codec_private_hex = None;
    let mut src = source(vec![]);
    verify_audio(&mut src, &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  // ---- PARSER-183: multi-sample first-bytes read ---------------------------

  // DTS header split so it only decodes once bytes from a SECOND sample are
  // collected — the multi-sample read must span both samples.
  #[test]
  fn read_first_bytes_spans_multiple_samples() {
    // File layout: [pad 8][half-frame A][half-frame B].  Two index samples
    // point at A and B; concatenating them yields a decodable DTS frame.
    let frame = build_dts_frame();
    let split = frame.len() / 2;
    let (a, c) = frame.split_at(split);
    let mut data = vec![0u8; 8];
    data.extend_from_slice(a);
    data.extend_from_slice(c);
    let off_a = 8u64;
    let off_b = 8u64 + a.len() as u64;

    let mut b = audio_builder("A_DTS", 0, 0.0);
    b.first_samples = vec![(off_a, a.len() as u64), (off_b, c.len() as u64)];
    let mut src = source(data);
    let got = read_first_bytes(&mut src, &dl(), &b, MAX_FIRST_BYTES).unwrap().unwrap();
    assert_eq!(got, frame);
  }

  // The multi-sample read stops once the requested window is filled.
  #[test]
  fn read_first_bytes_stops_at_requested_window() {
    let mut b = video_builder("avc1", 100, 100);
    // Three samples of 4 bytes each at 0/4/8; request only 6 bytes.
    b.first_samples = vec![(0, 4), (4, 4), (8, 4)];
    let mut src = source(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    let got = read_first_bytes(&mut src, &dl(), &b, 6).unwrap().unwrap();
    assert_eq!(got, vec![1, 2, 3, 4, 5, 6]);
  }

  // Falls back to the single-sample fields when no index was reconstructed.
  #[test]
  fn read_first_bytes_falls_back_to_single_sample() {
    let mut b = video_builder("avc1", 100, 100);
    b.first_sample_file_offset = Some(2);
    b.first_sample_size = Some(3);
    let mut src = source(vec![9, 8, 7, 6, 5]);
    let got = read_first_bytes(&mut src, &dl(), &b, 16).unwrap().unwrap();
    assert_eq!(got, vec![7, 6, 5]);
  }

  // r_qtmp4.cpp:3845-3853: S_VOBSUB needs an esds decoder config ≥ 64 bytes.
  #[test]
  fn vobsub_short_decoder_config_dropped() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"subp");
    b.codec_id_str = Some("S_VOBSUB".to_string());
    b.esds_decoder_specific_len = Some(16);
    verify_subtitles(&mut b);
    assert!(b.probe_failed);
  }

  #[test]
  fn vobsub_long_decoder_config_kept() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"subp");
    b.codec_id_str = Some("S_VOBSUB".to_string());
    b.esds_decoder_specific_len = Some(64);
    verify_subtitles(&mut b);
    assert!(!b.probe_failed);
  }

  #[test]
  fn tx3g_subtitle_kept() {
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"text");
    b.codec_id_str = Some("tx3g".to_string());
    verify_subtitles(&mut b);
    assert!(!b.probe_failed);
  }

  // read_first_bytes returns None when no chunk offset was recorded.
  #[test]
  fn read_first_bytes_none_without_offset() {
    let b = video_builder("avc1", 100, 100);
    let mut src = source(vec![1, 2, 3, 4]);
    assert!(read_first_bytes(&mut src, &dl(), &b, 16).unwrap().is_none());
  }

  #[test]
  fn read_first_bytes_clamps_to_sample_size_and_eof() {
    let mut b = video_builder("avc1", 100, 100);
    b.first_sample_file_offset = Some(0);
    b.first_sample_size = Some(3);
    let mut src = source(vec![9, 8, 7, 6, 5]);
    let got = read_first_bytes(&mut src, &dl(), &b, 16).unwrap().unwrap();
    assert_eq!(got, vec![9, 8, 7]);
  }

  #[test]
  fn verify_tracks_skips_metadata_handler() {
    let mut moov = MoovBuilder::default();
    let mut b = TrackBuilder::default();
    b.handler_type = Some(*b"meta");
    b.codec_id_str = Some("mdir".to_string());
    moov.tracks.push(b);
    let mut src = source(vec![]);
    verify_tracks(&mut src, &dl(), &mut moov).unwrap();
    // No gate ran (metadata handler) so it stays un-flagged here.
    assert!(!moov.tracks[0].probe_failed);
  }

  #[test]
  fn family_predicates() {
    assert!(is_avc("avc1"));
    assert!(is_avc("V_MPEG4/ISO/AVC"));
    assert!(is_hevc("hev1"));
    assert!(is_mp4v("mp4v"));
    assert!(is_dts("A_DTS"));
    assert!(is_dts("dtsc"));
    assert!(!is_avc("hev1"));
    assert!(is_mp2_mp3("A_MPEG/L3"));
    assert!(is_mp2_mp3("A_MPEG/L2"));
    assert!(is_mp2_mp3(".mp3"));
    assert!(is_ac3("A_AC3"));
    assert!(is_ac3("ac-3"));
    assert!(is_alac("alac"));
    assert!(is_alac("A_ALAC"));
    assert!(!is_alac("mp4a"));
    assert!(is_pcm("A_PCM/INT/LIT"));
  }

  // --- fixtures ---

  /// A tiny Annex B byte stream carrying a baseline 1920x1080 SPS + PPS.
  fn build_avc_annex_b() -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x67]; // SPS NAL header
    bytes.extend_from_slice(&[66u8, 0u8, 40u8]);
    bytes.extend(build_baseline_sps_tail());
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE]); // PPS NAL
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x09, 0xF0]); // AUD terminator
    bytes
  }

  fn build_baseline_sps_tail() -> Vec<u8> {
    let mut w = BitWriter::default();
    w.write_ue(0); // seq_parameter_set_id
    w.write_ue(0); // log2_max_frame_num_minus4
    w.write_ue(0); // pic_order_cnt_type
    w.write_ue(0); // log2_max_pic_order_cnt_lsb_minus4
    w.write_ue(0); // num_ref_frames
    w.write_bit(false); // gaps_in_frame_num_value_allowed_flag
    w.write_ue(119); // pic_width_in_mbs_minus1 (1920/16-1)
    w.write_ue(67); // pic_height_in_map_units_minus1
    w.write_bit(true); // frame_mbs_only_flag
    w.write_bit(false); // direct_8x8_inference_flag
    w.write_bit(true); // frame_cropping_flag
    w.write_ue(0); // crop_left
    w.write_ue(0); // crop_right
    w.write_ue(0); // crop_top
    w.write_ue(4); // crop_bottom
    w.write_bit(false); // vui_parameters_present_flag
    w.into_bytes()
  }

  #[derive(Default)]
  struct BitWriter {
    buf: Vec<u8>,
    bit_index: u8,
  }
  impl BitWriter {
    fn write_bit(&mut self, b: bool) {
      if self.bit_index == 0 {
        self.buf.push(0);
      }
      if b {
        let last = self.buf.len() - 1;
        self.buf[last] |= 1 << (7 - self.bit_index);
      }
      self.bit_index = (self.bit_index + 1) % 8;
    }
    fn write_bits(&mut self, value: u64, n: u32) {
      for i in 0..n {
        self.write_bit((value >> (n - 1 - i)) & 1 != 0);
      }
    }
    fn write_ue(&mut self, value: u32) {
      let codeword = value as u64 + 1;
      let nb = 64 - codeword.leading_zeros();
      for _ in 0..(nb - 1) {
        self.write_bit(false);
      }
      self.write_bits(codeword, nb);
    }
    fn into_bytes(mut self) -> Vec<u8> {
      self.write_bit(true);
      while self.bit_index != 0 {
        self.write_bit(false);
      }
      self.buf
    }
  }

  /// Build a single decodable DTS core frame via the audio::dts test helper.
  /// amode 6 = 3 channels, sfreq idx 13 = 48000 Hz.
  fn build_dts_frame() -> Vec<u8> {
    crate::media_metadata::audio::dts::build_dts_core_frame(6, 13)
  }

  // ---- PARSER-230 / PARSER-231: Opus / FLAC / Vorbis verification -------

  /// Xiph-lace `packets` (the inverse of [`unlace_xiph`]).
  fn lace_xiph(packets: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![(packets.len() - 1) as u8];
    for p in &packets[..packets.len() - 1] {
      let mut size = p.len();
      while size >= 255 {
        out.push(255);
        size -= 255;
      }
      out.push(size as u8);
    }
    for p in packets {
      out.extend_from_slice(p);
    }
    out
  }

  #[test]
  fn unlace_xiph_splits_three_packets_with_255_run() {
    let p0 = vec![1u8; 300]; // > 255 → exercises the 255-sum path
    let p1 = vec![2u8; 10];
    let p2 = vec![3u8; 50];
    let laced = lace_xiph(&[&p0, &p1, &p2]);
    let blocks = unlace_xiph(&laced).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].len(), 300);
    assert_eq!(blocks[1].len(), 10);
    assert_eq!(blocks[2].len(), 50);
  }

  #[test]
  fn vorbis_three_headers_derive_channels_and_rate() {
    let id = crate::media_metadata::ogg::codecs::vorbis::build_identification_packet(2, 44_100);
    let comment = b"\x03vorbis".to_vec();
    let setup = b"\x05vorbis".to_vec();
    let laced = lace_xiph(&[&id, &comment, &setup]);
    let mut b = audio_builder("mp4a", 0, 0.0); // channels / rate unset
    b.esds_object_type = Some(0xDD); // Vorbis OTI
    b.esds_decoder_specific_data = Some(laced);
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
    let a = b.audio.unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  #[test]
  fn vorbis_wrong_packet_count_dropped() {
    let id = crate::media_metadata::ogg::codecs::vorbis::build_identification_packet(2, 44_100);
    let laced = lace_xiph(&[&id, &id]); // only two packets
    let mut b = audio_builder("mp4a", 0, 0.0);
    b.esds_object_type = Some(0xDD);
    b.esds_decoder_specific_data = Some(laced);
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn vorbis_without_private_data_dropped() {
    let mut b = audio_builder("mp4a", 2, 44_100.0);
    b.esds_object_type = Some(0xDD);
    // No esds_decoder_specific_data.
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn opus_with_19_byte_private_kept() {
    let mut b = audio_builder("Opus", 2, 48_000.0);
    b.codec_private_hex = Some("00".repeat(19)); // exactly 19 bytes
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  #[test]
  fn opus_with_short_private_dropped() {
    let mut b = audio_builder("Opus", 2, 48_000.0);
    b.codec_private_hex = Some("00".repeat(18)); // 18 bytes < 19
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }

  #[test]
  fn flac_with_private_kept() {
    let mut b = audio_builder("fLaC", 2, 44_100.0);
    b.codec_private_hex = Some("664c6143".to_string()); // "fLaC"
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(!b.probe_failed);
  }

  #[test]
  fn flac_without_private_dropped() {
    let mut b = audio_builder("fLaC", 2, 44_100.0);
    // No codec_private_hex → no dfLa blob.
    verify_audio(&mut source(vec![]), &dl(), &mut b).unwrap();
    assert!(b.probe_failed);
  }
}
