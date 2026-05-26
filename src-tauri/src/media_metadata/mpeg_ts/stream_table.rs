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

//! Per-PID stream registry built from PMT entries + descriptors.

use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::codec::mpegts_stream_types;

use super::descriptors::DescriptorSummary;
use super::pmt::PmtStreamEntry;

#[derive(Debug, Clone)]
pub struct StreamRow {
  pub pid: u16,
  pub stream_type: u8,
  pub program_number: u16,
  pub language: Option<String>,
  pub teletext_page: Option<u32>,
  pub service_name: Option<String>,
  pub codec_id: String,
  pub codec_name: String,
  pub track_kind: TrackKind,
  /// PARSER-091: 5-byte DVB subtitling codec_private as written by
  /// mkvtoolnix's S_DVBSUB packetizer.
  pub codec_private: Option<Vec<u8>>,
  /// PARSER-092: hearing-impaired flag for teletext type 5 entries.
  pub hearing_impaired: Option<bool>,
  /// PARSER-173: Dolby Vision profile from the DV PMT descriptor (0xB0).
  pub dovi_profile: Option<u32>,
  /// PARSER-173: base-layer PID from the DV PMT descriptor when
  /// `bl_present_flag` is false (r_mpeg_ts.cpp:712-715).
  pub dovi_base_layer_pid: Option<u16>,
}

/// Canonical Matroska-style codec id string for known MPEG-TS stream types.
/// Mirrors mkvtoolnix's mapping in `r_mpeg_ts.cpp::create_packetizer`.
fn canonical_codec_id(stream_type: u8) -> Option<&'static str> {
  // PARSER-172: only the stream types mkvtoolnix actually supports resolve to a
  // real codec.  Anything else falls through `determine_codec_from_stream_type`
  // as Unknown (r_mpeg_ts.cpp:1012-1095, supported set r_mpeg_ts.h:55-87).  In
  // particular 0x20/0x21 (MVC / JPEG-2000) must NOT map to AVC, and 0x88 / 0xA0
  // are not recognised.
  Some(match stream_type {
    0x01 => "V_MPEG1",
    0x02 => "V_MPEG2",
    0x03 | 0x04 => "A_MPEG/L3",
    0x06 => "S_TX", // generic private; usually overridden by descriptors
    0x0F | 0x11 => "A_AAC",
    0x10 => "V_MPEG4/ISO/ASP",
    0x1B => "V_MPEG4/ISO/AVC",
    0x24 => "V_MPEGH/ISO/HEVC",
    0xEA => "V_VC1",
    // BD-style PMT stream types (ATSC / Blu-ray)
    0x80 => "A_PCM",
    0x81 => "A_AC3",
    0x82 | 0x85 | 0x86 => "A_DTS",
    0x83 => "A_TRUEHD",
    0x84 | 0x87 => "A_EAC3",
    0xA1 => "A_AC3",
    0xA2 => "A_DTS",
    0x90 => "S_HDMV/PGS",
    0x92 => "S_HDMV/TEXTST",
    _ => return None,
  })
}

/// Build one or more rows per `PmtStreamEntry`, applying descriptor overrides
/// where applicable.  Returns multiple rows when a teletext or DVB subtitling
/// descriptor declares multiple per-language subtitle pages (PARSER-091..092).
pub fn build_rows(
  pid: u16,
  program_number: u16,
  entry: &PmtStreamEntry,
  _program_descriptors: &DescriptorSummary,
) -> Vec<StreamRow> {
  let stream_desc = super::descriptors::walk(&entry.descriptors);
  let stream_language = stream_desc.language_iso_639_2.clone();
  let language = stream_language.clone();
  let service_name: Option<String> = None;

  let from_table = mpegts_stream_types::lookup(entry.stream_type);
  let mut codec_id = canonical_codec_id(entry.stream_type)
    .map(str::to_owned)
    .unwrap_or_else(|| format!("0x{:02X}", entry.stream_type));
  let mut codec_name = from_table
    .map(|e| e.name.to_string())
    .unwrap_or_else(|| "Unknown".to_string());
  let mut kind = from_table.map(|e| e.kind).unwrap_or(TrackKind::Unknown);

  // PARSER-090: registration descriptor (0x05) overrides codec on private
  // PES streams (stream_type 0x06).  `HDMV` carries an embedded Blu-ray
  // stream_coding_type byte; every other FourCC is looked up directly.
  if entry.stream_type == 0x06 {
    if let Some(reg) = &stream_desc.registration {
      if reg.format_identifier == "HDMV" {
        if let Some(sct) = reg.hdmv_stream_coding_type {
          if let Some((cid, cname, ck)) = super::descriptors::registration::hdmv_codec(sct) {
            codec_id = cid.to_string();
            codec_name = cname.to_string();
            kind = ck;
          }
        }
      } else if let Some((cid, cname, ck)) =
        super::descriptors::registration::codec_for_fourcc(reg.format_identifier.as_str())
      {
        codec_id = cid.to_string();
        codec_name = cname.to_string();
        kind = ck;
      }
    }
  }

  // Descriptor-driven overrides — handle the common "stream_type = 0x06
  // private data + a descriptor that disambiguates the codec" pattern.
  if entry.stream_type == 0x06 {
    if stream_desc.is_ac3 {
      codec_id = "A_AC3".to_string();
      codec_name = "AC-3".to_string();
      kind = TrackKind::Audio;
    } else if stream_desc.is_eac3 {
      codec_id = "A_EAC3".to_string();
      codec_name = "E-AC-3".to_string();
      kind = TrackKind::Audio;
    } else if stream_desc.is_dts {
      codec_id = "A_DTS".to_string();
      codec_name = "DTS".to_string();
      kind = TrackKind::Audio;
    }
  }
  // PARSER-251: mkvtoolnix's PMT descriptor switch (r_mpeg_ts.cpp:1864-1887)
  // handles only registration / ISO-639 / teletext / subtitling / AC-3 /
  // E-AC-3 / DTS / Dolby Vision tags — there is no HEVC-descriptor (0x38)
  // handler.  A private PES stream signalled only by an HEVC descriptor stays
  // `unknown` and is dropped; HEVC video is recognised solely by stream_type
  // 0x24 (→ the canonical `V_MPEGH/ISO/HEVC`).  We therefore do not promote on
  // the HEVC descriptor (and emit no noncanonical `V_HEVC`).

  // PARSER-091: DVB subtitling descriptor (0x59) on a private PES stream
  // creates one S_DVBSUB track per language entry.
  if entry.stream_type == 0x06 && !stream_desc.subtitling_entries.is_empty() {
    return stream_desc
      .subtitling_entries
      .iter()
      .map(|sub| StreamRow {
        pid,
        stream_type: entry.stream_type,
        program_number,
        language: Some(sub.language_iso_639_2.clone()),
        teletext_page: None,
        service_name: service_name.clone(),
        codec_id: "S_DVBSUB".to_string(),
        codec_name: "DVB Subtitles".to_string(),
        track_kind: TrackKind::Subtitle,
        codec_private: Some(sub.codec_private().to_vec()),
        hearing_impaired: None,
        dovi_profile: None,
        dovi_base_layer_pid: None,
      })
      .collect();
  }

  // PARSER-092: teletext descriptor (0x56) — emit one S_TELETEXT track
  // per type=2 or type=5 entry; track hearing-impaired flag for type 5.
  if entry.stream_type == 0x06 && !stream_desc.teletext_entries.is_empty() {
    let subtitle_entries: Vec<_> = stream_desc
      .teletext_entries
      .iter()
      .filter(|e| e.is_subtitle())
      .cloned()
      .collect();
    if !subtitle_entries.is_empty() {
      return subtitle_entries
        .iter()
        .map(|e| StreamRow {
          pid,
          stream_type: entry.stream_type,
          program_number,
          language: Some(e.language_iso_639_2.clone()),
          teletext_page: Some(e.page),
          service_name: service_name.clone(),
          codec_id: "S_TELETEXT".to_string(),
          codec_name: "DVB Teletext".to_string(),
          track_kind: TrackKind::Subtitle,
          codec_private: None,
          hearing_impaired: Some(e.is_hearing_impaired()),
          dovi_profile: None,
          dovi_base_layer_pid: None,
        })
        .collect();
    }
    // All teletext entries are non-subtitle (initial pages etc).
    // Drop the track — mkvtoolnix records the pages in
    // `m_ttx_known_non_subtitle_pages` and creates no demuxed track.
    kind = TrackKind::Unknown;
  }

  // PARSER-093: stream_type 0x06 with no disambiguating descriptor defaults
  // to AC-3 audio (mkvtoolnix `r_mpeg_ts.cpp:1890-1895`).
  if entry.stream_type == 0x06
    && kind != TrackKind::Audio
    && kind != TrackKind::Video
    && kind != TrackKind::Subtitle
    && !stream_desc.has_disambiguating_tag
  {
    codec_id = "A_AC3".to_string();
    codec_name = "AC-3".to_string();
    kind = TrackKind::Audio;
  }

  // PARSER-173: carry the Dolby Vision descriptor (profile + optional
  // base-layer PID) onto the row so the reader can pair base/enhancement
  // layers.  mkvtoolnix only stores the DV config for private-PES streams
  // (r_mpeg_ts.cpp:694-720); for those the enhancement layer is typically a
  // video stream promoted by a co-located HEVC/registration descriptor.
  let (dovi_profile, dovi_base_layer_pid) = match stream_desc.dovi {
    Some(d) if kind == TrackKind::Video => (Some(d.profile), d.base_layer_pid),
    _ => (None, None),
  };

  let primary = StreamRow {
    pid,
    stream_type: entry.stream_type,
    program_number,
    language: language.clone(),
    teletext_page: stream_desc.teletext_page,
    service_name: service_name.clone(),
    codec_id,
    codec_name,
    track_kind: kind,
    codec_private: None,
    hearing_impaired: None,
    dovi_profile,
    dovi_base_layer_pid,
  };

  // PARSER-159: a Blu-ray TrueHD stream (stream_type 0x83) carries an embedded
  // AC-3 compatibility sub-stream.  mkvtoolnix creates a coupled `A_AC3` track
  // on the same PID right after the primary `A_TRUEHD` track
  // (`r_mpeg_ts.cpp:1050-1062, 1897-1903`); we mirror that so track counts and
  // codec lists match.
  if entry.stream_type == 0x83 {
    let coupled = StreamRow {
      pid,
      stream_type: entry.stream_type,
      program_number,
      language,
      teletext_page: None,
      service_name,
      codec_id: "A_AC3".to_string(),
      codec_name: "AC-3".to_string(),
      track_kind: TrackKind::Audio,
      codec_private: None,
      hearing_impaired: None,
      dovi_profile: None,
      dovi_base_layer_pid: None,
    };
    return vec![primary, coupled];
  }

  vec![primary]
}

/// Thin back-compat wrapper that returns the *first* row produced by
/// [`build_rows`].  Old callers/tests rely on a single-row signature; new
/// callers should use `build_rows` to surface DVB subtitling and teletext
/// multi-entry tracks.
pub fn build_row(
  pid: u16,
  program_number: u16,
  entry: &PmtStreamEntry,
  program_descriptors: &DescriptorSummary,
) -> StreamRow {
  let mut rows = build_rows(pid, program_number, entry, program_descriptors);
  rows.swap_remove(0)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::mpeg_ts::descriptors::{
    TAG_AC3, TAG_DOVI, TAG_DTS, TAG_EAC3, TAG_HEVC, TAG_ISO_639_LANGUAGE, TAG_TELETEXT, build_descriptor, walk,
  };

  /// Build a Dolby Vision descriptor body (bit-packed) with profile 7 and a
  /// base-layer PID (bl_present_flag = false).
  fn dovi_body(profile: u32, base_layer_pid: u16) -> Vec<u8> {
    let mut bits: Vec<u8> = Vec::new();
    let mut push = |value: u64, n: u32| {
      for i in (0..n).rev() {
        bits.push(((value >> i) & 1) as u8);
      }
    };
    push(1, 8); // dv_version_major
    push(0, 8); // dv_version_minor
    push(profile as u64, 7);
    push(0, 6); // dv_level
    push(0, 1); // rpu_present_flag
    push(0, 1); // bl_present_flag = false
    push(0, 1); // el_present_flag
    push(base_layer_pid as u64, 13);
    push(0, 3);
    push(0, 4); // dv_bl_signal_compatibility_id
    push(0, 4);
    let mut out = Vec::new();
    for chunk in bits.chunks(8) {
      let mut byte = 0u8;
      for (i, &b) in chunk.iter().enumerate() {
        byte |= b << (7 - i);
      }
      out.push(byte);
    }
    out
  }

  fn entry(stream_type: u8, descriptors: Vec<u8>) -> PmtStreamEntry {
    PmtStreamEntry {
      stream_type,
      elementary_pid: 0x1234,
      descriptors,
    }
  }

  #[test]
  fn dovi_descriptor_on_video_carries_profile_and_base_layer_pid() {
    // PARSER-173 / PARSER-251: an HEVC video stream (stream_type 0x24 → the
    // canonical V_MPEGH/ISO/HEVC) carrying a Dolby Vision descriptor exposes the
    // DV profile + base-layer PID.  HEVC video is recognised by its stream_type,
    // not by the HEVC PMT descriptor (which mkvtoolnix ignores).
    let descs = build_descriptor(TAG_DOVI, &dovi_body(7, 0x1010));
    let row = build_row(0x1234, 1, &entry(0x24, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "V_MPEGH/ISO/HEVC");
    assert_eq!(row.track_kind, TrackKind::Video);
    assert_eq!(row.dovi_profile, Some(7));
    assert_eq!(row.dovi_base_layer_pid, Some(0x1010));
  }

  #[test]
  fn dovi_descriptor_on_non_video_is_ignored() {
    // A DV descriptor on a stream that does not resolve to video carries no DV
    // pairing data (mkvtoolnix only stores the config for video EL streams).
    let descs = build_descriptor(TAG_DOVI, &dovi_body(7, 0x1010));
    // stream_type 0x0F (AAC) → audio kind.
    let row = build_row(0x1234, 1, &entry(0x0F, descs), &DescriptorSummary::default());
    assert_eq!(row.track_kind, TrackKind::Audio);
    assert_eq!(row.dovi_profile, None);
    assert_eq!(row.dovi_base_layer_pid, None);
  }

  #[test]
  fn truehd_stream_type_emits_coupled_ac3_track() {
    // PARSER-159: stream_type 0x83 yields the primary TrueHD track plus a
    // coupled AC-3 compatibility track on the same PID.
    let rows = build_rows(0x1100, 1, &entry(0x83, vec![]), &DescriptorSummary::default());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].codec_id, "A_TRUEHD");
    assert_eq!(rows[0].track_kind, TrackKind::Audio);
    assert_eq!(rows[1].codec_id, "A_AC3");
    assert_eq!(rows[1].track_kind, TrackKind::Audio);
    assert_eq!(rows[1].pid, 0x1100);
  }

  #[test]
  fn truehd_coupled_ac3_inherits_language() {
    let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"eng\x00");
    let rows = build_rows(0x1100, 1, &entry(0x83, descs), &DescriptorSummary::default());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].language.as_deref(), Some("eng"));
    assert_eq!(rows[1].language.as_deref(), Some("eng"));
  }

  #[test]
  fn known_stream_type_resolved_via_catalogue() {
    let e = entry(0x1B, vec![]); // H.264
    let row = build_row(0x1234, 1, &e, &DescriptorSummary::default());
    assert_eq!(row.codec_id, "V_MPEG4/ISO/AVC");
    assert_eq!(row.track_kind, TrackKind::Video);
  }

  #[test]
  fn iso_639_descriptor_populates_language() {
    let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"fra\x00");
    let row = build_row(0x1234, 1, &entry(0x0F, descs), &DescriptorSummary::default());
    assert_eq!(row.language.as_deref(), Some("fra"));
  }

  #[test]
  fn ac3_descriptor_on_private_data_promotes_to_ac3() {
    let descs = build_descriptor(TAG_AC3, &[]);
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_AC3");
    assert_eq!(row.track_kind, TrackKind::Audio);
  }

  #[test]
  fn eac3_descriptor_on_private_data_promotes_to_eac3() {
    let descs = build_descriptor(TAG_EAC3, &[]);
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_EAC3");
  }

  #[test]
  fn dts_descriptor_on_private_data_promotes_to_dts() {
    let descs = build_descriptor(TAG_DTS, &[]);
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_DTS");
  }

  #[test]
  fn teletext_descriptor_on_private_data_promotes_to_subtitle() {
    // Use teletext type 2 (subtitle page) in the top 5 bits — PARSER-092
    // gates subtitle promotion on types 2 / 5 only.
    let descs = build_descriptor(TAG_TELETEXT, &[b'e', b'n', b'g', (2 << 3), 0x88]);
    let rows = build_rows(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].codec_id, "S_TELETEXT");
    assert_eq!(rows[0].track_kind, TrackKind::Subtitle);
    assert_eq!(rows[0].teletext_page, Some(888));
  }

  // ---- PARSER-090: registration descriptor ----------------------------

  #[test]
  fn hdmv_registration_promotes_to_ac3() {
    // HDMV + stuffing byte 0xFF + stream_coding_type 0x81 (AC-3).
    let descs = build_descriptor(
      super::super::descriptors::TAG_REGISTRATION,
      &[b'H', b'D', b'M', b'V', 0xFF, 0x81, 0x00, 0x00],
    );
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_AC3");
    assert_eq!(row.track_kind, TrackKind::Audio);
  }

  #[test]
  fn registration_vc1_promotes_to_vc1_video() {
    let descs = build_descriptor(super::super::descriptors::TAG_REGISTRATION, &[b'V', b'C', b'-', b'1']);
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "V_VC1");
    assert_eq!(row.track_kind, TrackKind::Video);
  }

  // ---- PARSER-091: DVB subtitling descriptor --------------------------

  #[test]
  fn subtitling_descriptor_creates_one_track_per_language() {
    let mut body = Vec::new();
    body.extend_from_slice(&[b'e', b'n', b'g', 0x10, 0x00, 0x01, 0x00, 0x02]);
    body.extend_from_slice(&[b'd', b'e', b'u', 0x20, 0x00, 0x03, 0x00, 0x04]);
    let descs = build_descriptor(super::super::descriptors::TAG_SUBTITLING, &body);
    let rows = build_rows(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].language.as_deref(), Some("eng"));
    assert_eq!(rows[0].codec_id, "S_DVBSUB");
    assert_eq!(
      rows[0].codec_private.as_deref(),
      Some(&[0x00u8, 0x01, 0x00, 0x02, 0x10][..])
    );
    assert_eq!(rows[1].language.as_deref(), Some("deu"));
    assert_eq!(
      rows[1].codec_private.as_deref(),
      Some(&[0x00u8, 0x03, 0x00, 0x04, 0x20][..])
    );
  }

  // ---- PARSER-092: multi-page teletext --------------------------------

  #[test]
  fn teletext_descriptor_emits_one_track_per_subtitle_page() {
    let mut body = Vec::new();
    body.extend_from_slice(&[b'e', b'n', b'g', (2 << 3) | 0x01, 0x50]); // subtitle 150
    body.extend_from_slice(&[b'd', b'e', b'u', (5 << 3) | 0x02, 0x12]); // hearing impaired 212
    body.extend_from_slice(&[b'f', b'r', b'a', 0x00, 0x88]); // initial page (filtered)
    let descs = build_descriptor(TAG_TELETEXT, &body);
    let rows = build_rows(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].language.as_deref(), Some("eng"));
    assert_eq!(rows[0].teletext_page, Some(150));
    assert_eq!(rows[0].hearing_impaired, Some(false));
    assert_eq!(rows[1].language.as_deref(), Some("deu"));
    assert_eq!(rows[1].teletext_page, Some(212));
    assert_eq!(rows[1].hearing_impaired, Some(true));
  }

  #[test]
  fn teletext_with_only_non_subtitle_types_is_dropped() {
    // Type 0 (reserved) and type 1 (initial page) — neither is a subtitle.
    let mut body = Vec::new();
    body.extend_from_slice(&[b'e', b'n', b'g', 0x00, 0x10]);
    body.extend_from_slice(&[b'e', b'n', b'g', (1 << 3), 0x88]);
    let descs = build_descriptor(TAG_TELETEXT, &body);
    let rows = build_rows(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].track_kind, TrackKind::Unknown);
  }

  // ---- PARSER-093: default 0x06 to AC-3 ------------------------------

  #[test]
  fn stream_type_06_with_no_descriptor_defaults_to_ac3() {
    let row = build_row(0x1234, 1, &entry(0x06, vec![]), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_AC3");
    assert_eq!(row.track_kind, TrackKind::Audio);
  }

  #[test]
  fn stream_type_06_with_only_iso_639_still_defaults_to_ac3() {
    // ISO-639 (0x0A) does *not* clear the missing-tag bit, so the
    // AC-3 fallback still kicks in (mkvtoolnix `r_mpeg_ts.cpp:1861-1862`).
    let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"fra\x00");
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "A_AC3");
    assert_eq!(row.language.as_deref(), Some("fra"));
  }

  #[test]
  fn hevc_descriptor_does_not_promote_unknown_stream() {
    // PARSER-251: mkvtoolnix has no HEVC PMT descriptor handler, so an unknown
    // stream type carrying only an HEVC descriptor (0x38) is not promoted — it
    // stays unknown and is later dropped.  HEVC video must arrive via
    // stream_type 0x24.
    let descs = build_descriptor(TAG_HEVC, &[]);
    let row = build_row(0x1234, 1, &entry(0xFA, descs), &DescriptorSummary::default());
    assert_eq!(row.track_kind, TrackKind::Unknown);
    assert_ne!(row.codec_id, "V_HEVC");
  }

  #[test]
  fn private_stream_with_only_hevc_descriptor_stays_unknown() {
    // PARSER-251: a private PES stream (0x06) signalled solely by an HEVC
    // descriptor does not become AC-3 (the descriptor clears `missing_tag`) and
    // is not promoted to HEVC — it stays unknown, exactly as mkvtoolnix leaves
    // it (r_mpeg_ts.cpp:1861-1895).
    let descs = build_descriptor(TAG_HEVC, &[]);
    let row = build_row(0x1234, 1, &entry(0x06, descs), &DescriptorSummary::default());
    assert_eq!(row.track_kind, TrackKind::Unknown);
  }

  #[test]
  fn stream_type_24_resolves_to_canonical_hevc() {
    // PARSER-251: HEVC video uses the canonical Matroska codec id.
    let row = build_row(0x1234, 1, &entry(0x24, vec![]), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "V_MPEGH/ISO/HEVC");
    assert_eq!(row.track_kind, TrackKind::Video);
  }

  #[test]
  fn program_level_language_is_not_used_when_stream_lacks_one() {
    let mut prog = DescriptorSummary::default();
    prog.language_iso_639_2 = Some("jpn".to_string());
    let row = build_row(0x1234, 1, &entry(0x0F, vec![]), &prog);
    assert!(row.language.is_none());
  }

  #[test]
  fn program_level_service_name_is_not_copied_to_stream() {
    let mut prog = DescriptorSummary::default();
    prog.service_name = Some("Program Service".to_string());
    let row = build_row(0x1234, 1, &entry(0x0F, vec![]), &prog);
    assert!(row.service_name.is_none());
  }

  #[test]
  fn stream_level_language_is_used() {
    let mut prog = DescriptorSummary::default();
    prog.language_iso_639_2 = Some("jpn".to_string());
    let descs = build_descriptor(TAG_ISO_639_LANGUAGE, b"fra\x00");
    let row = build_row(0x1234, 1, &entry(0x0F, descs), &prog);
    assert_eq!(row.language.as_deref(), Some("fra"));
  }

  #[test]
  fn unknown_stream_type_falls_back_to_hex_id() {
    let row = build_row(0x1234, 1, &entry(0xEE, vec![]), &DescriptorSummary::default());
    assert_eq!(row.codec_id, "0xEE");
    assert_eq!(row.track_kind, TrackKind::Unknown);
  }

  #[test]
  fn descriptor_walk_compiles_into_summary() {
    let mut descs = Vec::new();
    descs.extend(build_descriptor(TAG_ISO_639_LANGUAGE, b"eng\x00"));
    descs.extend(build_descriptor(TAG_AC3, &[]));
    let s = walk(&descs);
    assert_eq!(s.language_iso_639_2.as_deref(), Some("eng"));
    assert!(s.is_ac3);
  }

  #[test]
  fn canonical_codec_id_covers_known_stream_types() {
    // PARSER-172: only mkvtoolnix-supported stream types map to a real codec.
    let cases = [
      (0x01u8, "V_MPEG1"),
      (0x02, "V_MPEG2"),
      (0x03, "A_MPEG/L3"),
      (0x04, "A_MPEG/L3"),
      (0x0F, "A_AAC"),
      (0x10, "V_MPEG4/ISO/ASP"),
      (0x11, "A_AAC"),
      (0x1B, "V_MPEG4/ISO/AVC"),
      (0x24, "V_MPEGH/ISO/HEVC"),
      (0xEA, "V_VC1"),
      (0x80, "A_PCM"),
      (0x81, "A_AC3"),
      (0x82, "A_DTS"),
      (0x83, "A_TRUEHD"),
      (0x84, "A_EAC3"),
      (0x85, "A_DTS"),
      (0x86, "A_DTS"),
      (0x87, "A_EAC3"),
      (0x90, "S_HDMV/PGS"),
      (0x92, "S_HDMV/TEXTST"),
      (0xA1, "A_AC3"),
      (0xA2, "A_DTS"),
    ];
    for (st, expected) in cases {
      assert_eq!(canonical_codec_id(st), Some(expected), "stream_type 0x{st:02X}");
    }
    assert_eq!(canonical_codec_id(0xEE), None);
    // PARSER-172: removed mappings now fall through.
    assert_eq!(canonical_codec_id(0x20), None);
    assert_eq!(canonical_codec_id(0x21), None);
    assert_eq!(canonical_codec_id(0x88), None);
    assert_eq!(canonical_codec_id(0xA0), None);
  }
}
