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

//! Top-level `MpegTsReader`. Pure-Rust port of `r_mpeg_ts.cpp`.
//!
//! - Packet size is detected with alignment + several sync confirmations
//!   (PARSER-053).
//! - PSI sections (PAT / PMT / SDT) are reassembled across packets using the
//!   declared `section_length` (PARSER-054); SDT (PID 0x0011) supplies program
//!   service provider/name (PARSER-055).
//! - Transport-error packets are dropped and continuity-counter discontinuities
//!   reset the in-progress section (PARSER-057).
//! - PIDs carrying PES that no PMT lists are content-sniffed as a fallback when
//!   no listed streams are found (PARSER-056).

use std::collections::HashMap;

use crate::media_metadata::audio::{aac, ac3, dts, mp3, truehd};
use crate::media_metadata::codec::TrackKind;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::{avc, hevc, mpeg_video, vc1};
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::reader::Reader;

use super::descriptors::DescriptorSummary;
use super::descriptors::service;
use super::identify::{self, EsEnrichment, RowProbe};
use super::packet::{self, PACKET_SIZE_BD_M2TS, detect_packet_size_aligned};
use super::pat;
use super::pmt;
use super::stream_table::{self, StreamRow};

const MAX_PACKETS: usize = 64 * 1024;
const PROBE_BYTES: usize = 16 * 1024;
const SDT_PID: u16 = 0x0011;
const NULL_PID: u16 = 0x1FFF;
/// PARSER-158: bounded per-PID PES accumulation for codec probing.  We hold up
/// to 64 KiB of each candidate stream's payload — enough for an AVC/HEVC SPS or
/// the first audio frame header — across at most this many PIDs.
const PES_PAYLOAD_CAP: usize = 64 * 1024;
const MAX_PES_PIDS: usize = 32;
/// Packet budget scanned (after all PMTs are seen) before stopping, so the
/// per-PID PES buffers have a chance to fill for the header probe.
///
/// PARSER-169: mkvtoolnix probes at least 5 MiB before declaring a content
/// probe successful (`min_size_to_probe`, r_mpeg_ts.cpp:1347-1376).  Because
/// tracks now survive only when their bounded PES probe succeeds, we must read
/// far enough that the per-PID buffers can fill or we would under-report
/// relative to mkvmerge.  5 MiB of 188-byte packets ≈ 27 900 packets; keep it
/// well under MAX_PACKETS (64 Ki packets) and still bounded by the deadline +
/// EOF + the per-PID PES cap.
const PES_PROBE_PACKET_BUDGET: usize = 28 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct MpegTsReader;

/// Per-PID PSI section reassembler (PARSER-054).
#[derive(Default)]
struct SectionAssembler {
  buf: Vec<u8>,
  last_cc: Option<u8>,
  in_progress: bool,
}

impl SectionAssembler {
  /// Feed one packet's payload; return a complete section when one finishes.
  fn feed(&mut self, header: &packet::PacketHeader, payload: &[u8]) -> Option<Vec<u8>> {
    // Continuity check (PARSER-057): a gap means we lost packets, so any
    // partially assembled section must be discarded.
    if let Some(last) = self.last_cc {
      let expected = (last + 1) & 0x0F;
      if header.continuity_counter != expected && header.continuity_counter != last {
        self.buf.clear();
        self.in_progress = false;
      }
    }
    self.last_cc = Some(header.continuity_counter);

    if header.payload_unit_start {
      if payload.is_empty() {
        return None;
      }
      let pointer = payload[0] as usize;
      let start = 1 + pointer;
      if start > payload.len() {
        return None;
      }
      self.buf.clear();
      self.buf.extend_from_slice(&payload[start..]);
      self.in_progress = true;
    } else if self.in_progress {
      self.buf.extend_from_slice(payload);
    } else {
      return None;
    }

    if self.buf.len() >= 3 {
      let section_length = (((self.buf[1] as usize) & 0x0F) << 8) | self.buf[2] as usize;
      let total = 3 + section_length;
      if self.buf.len() >= total {
        let section = self.buf[..total].to_vec();
        self.buf.clear();
        self.in_progress = false;
        return Some(section);
      }
    }
    None
  }
}

impl Reader for MpegTsReader {
  fn name(&self) -> &'static str {
    "mpeg_ts"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let mut head = vec![0u8; PROBE_BYTES];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    if read < 188 {
      return Ok(false);
    }
    Ok(detect_packet_size_aligned(&head[..read]).is_some())
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let mut probe = vec![0u8; PROBE_BYTES];
    src.seek_to(0)?;
    let probe_len = src.read_at_most(&mut probe)?;
    let (packet_size, start) = detect_packet_size_aligned(&probe[..probe_len]).ok_or(ParseError::Unrecognised)?;
    let header_offset = if packet_size == PACKET_SIZE_BD_M2TS { 4 } else { 0 };

    src.seek_to(start as u64)?;
    let mut pmt_pids: HashMap<u16, u16> = HashMap::new(); // pmt_pid → program_number
    let mut rows: Vec<StreamRow> = Vec::new();
    let mut seen_pmt_pids: std::collections::HashSet<u16> = Default::default();
    let mut sdt_map: HashMap<u16, (String, String)> = HashMap::new();
    let mut assemblers: HashMap<u16, SectionAssembler> = HashMap::new();
    let mut pmt_stream_pids: std::collections::HashSet<u16> = Default::default();
    // PARSER-056 / PARSER-158: bounded per-PID PES payload accumulation, used
    // both to sniff unlisted PIDs and to probe codec parameters for listed
    // tracks.
    let mut pes_payloads: HashMap<u16, Vec<u8>> = HashMap::new();

    let mut packet_buf = vec![0u8; packet_size];
    let mut packet_count = 0usize;
    loop {
      deadline.check("mpeg_ts::reader")?;
      if packet_count >= MAX_PACKETS {
        break;
      }
      let read = src.read_at_most(&mut packet_buf)?;
      if read < packet_size {
        break;
      }
      packet_count += 1;
      let pkt = &packet_buf[header_offset..];
      if pkt.len() < 4 || pkt[0] != packet::TS_SYNC_BYTE {
        continue;
      }
      let header = match packet::decode_header(pkt) {
        Ok(h) => h,
        Err(_) => continue,
      };
      // Drop transport-error packets (PARSER-057).
      if header.transport_error || !header.has_payload() {
        continue;
      }
      let payload = packet::payload_slice(pkt, &header);
      if payload.is_empty() {
        continue;
      }

      if packet::is_pat_pid(header.pid) {
        if let Some(section) = assemblers.entry(0).or_default().feed(&header, payload) {
          handle_pat(&section, &mut pmt_pids);
        }
      } else if header.pid == SDT_PID {
        if let Some(section) = assemblers.entry(SDT_PID).or_default().feed(&header, payload) {
          handle_sdt(&section, &mut sdt_map);
        }
      } else if let Some(&prog) = pmt_pids.get(&header.pid) {
        let asm = assemblers.entry(header.pid).or_default();
        if let Some(section) = asm.feed(&header, payload) {
          if seen_pmt_pids.insert(header.pid) {
            handle_pmt(&section, prog, &mut rows, &mut pmt_stream_pids);
          }
        }
      } else if header.pid != NULL_PID {
        // Candidate PES PID (PARSER-056 / PARSER-158).  Start a buffer at the
        // first PES-start packet, then keep appending continuation packets up
        // to the cap so the codec probe has enough of the elementary stream.
        //
        // PARSER-271: accumulate only *elementary* bytes.  mkvtoolnix finalises
        // the previous PES at every new payload-unit start, strips that PES
        // header, and appends only the elementary payload
        // (`r_mpeg_ts.cpp:2147-2195`, `2394-2427`).  We mirror that by stripping
        // the PES header on every PES-start packet and appending continuation
        // packets verbatim, so a later PES header is never injected into the
        // probe buffer where it would interrupt an AAC / AC-3 / video header
        // search that spans PES boundaries.
        let is_pes_start =
          header.payload_unit_start && payload.len() >= 4 && payload[0] == 0 && payload[1] == 0 && payload[2] == 1;
        let known = pes_payloads.contains_key(&header.pid);
        if known || (is_pes_start && pes_payloads.len() < MAX_PES_PIDS) {
          let elementary: &[u8] = if is_pes_start { strip_pes_header(payload) } else { payload };
          let buf = pes_payloads.entry(header.pid).or_default();
          if buf.len() < PES_PAYLOAD_CAP {
            let take = (PES_PAYLOAD_CAP - buf.len()).min(elementary.len());
            buf.extend_from_slice(&elementary[..take]);
          }
        }
      }

      // Stop once every known PMT is processed and the PES probe budget is
      // spent (PARSER-158: scan far enough to fill the per-PID buffers rather
      // than bailing the moment the SDT arrives).
      if !pmt_pids.is_empty() && seen_pmt_pids.len() == pmt_pids.len() && packet_count > PES_PROBE_PACKET_BUDGET {
        break;
      }
    }

    // Fallback content detection for unlisted PES PIDs (PARSER-056): only
    // when no PMT-listed streams were found, and only on a confident sniff.
    if rows.is_empty() {
      let mut pids: Vec<u16> = pes_payloads.keys().copied().collect();
      pids.sort_unstable();
      for pid in pids {
        if pmt_stream_pids.contains(&pid) {
          continue;
        }
        // PARSER-271: the buffer already holds PES-header-stripped elementary
        // bytes, so it is sniffed directly.
        if let Some((kind, id, name)) = sniff_codec(&pes_payloads[&pid]) {
          rows.push(StreamRow {
            pid,
            stream_type: 0,
            program_number: 0,
            language: None,
            teletext_page: None,
            service_name: None,
            codec_id: id.to_string(),
            codec_name: name.to_string(),
            track_kind: kind,
            codec_private: None,
            hearing_impaired: None,
            dovi_profile: None,
            dovi_base_layer_pid: None,
          });
        }
      }
    }

    // PARSER-158 / PARSER-169 / PARSER-170: probe each row's PES payload for
    // the codec parameters the PMT alone cannot supply (audio channels /
    // sampling rate / bit depth, video dimensions, TextST codec_private) and
    // decide whether the row is `probed_ok`.  Rows that need no content probe
    // (PGS / DVBSUB / Teletext) are kept unconditionally.
    let probes = compute_probes(&rows, &pes_payloads);

    identify::finalise_with_probes(rows, &sdt_map, &probes, out);
    Ok(())
  }
}

/// PARSER-158: strip the PES packet header so codec probes see the elementary
/// stream bytes.  Streams carrying the full optional header (`PTS_DTS_flags`
/// etc.) start their payload at `9 + PES_header_data_length`; the few stream
/// ids that never carry it (program stream map / padding / private-2 / ECM /
/// EMM / directory / DSMCC / H.222.1-E) start right after the 6-byte prefix.
fn strip_pes_header(bytes: &[u8]) -> &[u8] {
  if bytes.len() < 6 || bytes[0..3] != [0x00, 0x00, 0x01] {
    return bytes;
  }
  match bytes[3] {
    0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF => bytes.get(6..).unwrap_or(&[]),
    _ => {
      if bytes.len() < 9 {
        return &[];
      }
      let header_data_len = bytes[8] as usize;
      bytes.get(9 + header_data_len..).unwrap_or(&[])
    }
  }
}

/// PARSER-169 / PARSER-170: build a per-row probe list parallel to `rows`.  Each
/// `RowProbe.keep` mirrors mkvtoolnix's `probed_ok` flag
/// (r_mpeg_ts.cpp:1547-1572):
///
/// * Audio/video content codecs are `keep` only when their bounded PES header
///   decode succeeded.
/// * PGS subtitles (r_mpeg_ts.cpp:1080-1084), DVBSUB (r_mpeg_ts.cpp:527-533)
///   and Teletext (r_mpeg_ts.cpp:820-843) are always `keep`.
/// * TextST is `keep` only when the dialog-style segment is present
///   (r_mpeg_ts.cpp:501-525).
/// * A stream_type 0x83 TrueHD primary is `keep` once a TrueHD sync is found;
///   its coupled `A_AC3` row is `keep` only when an embedded AC-3 frame is also
///   found (r_mpeg_ts.cpp:454-499, 1554-1567).
fn compute_probes(rows: &[StreamRow], pes: &HashMap<u16, Vec<u8>>) -> Vec<RowProbe> {
  let mut probes: Vec<RowProbe> = Vec::with_capacity(rows.len());
  for row in rows.iter() {
    // stream_type 0x83 produces two coupled rows (TrueHD + AC-3) on one PID;
    // probe the shared payload once and gate each independently.
    if row.stream_type == 0x83 {
      // PARSER-271: buffers already contain PES-header-stripped elementary bytes.
      let (truehd, ac3) = probe_truehd_pair(pes.get(&row.pid).map(|v| v.as_slice()));
      // The primary TrueHD row is emitted first, then the coupled AC-3 row.
      let is_primary = row.codec_id == "A_TRUEHD";
      let probe = if is_primary {
        match truehd {
          Some(e) => RowProbe { keep: true, enrichment: e },
          None => RowProbe::default(),
        }
      } else {
        match ac3 {
          Some(e) => RowProbe { keep: true, enrichment: e },
          None => RowProbe::default(),
        }
      };
      probes.push(probe);
      continue;
    }

    // Subtitle tracks that need no content probe are always probed_ok.
    if matches!(row.codec_id.as_str(), "S_HDMV/PGS" | "S_DVBSUB" | "S_TELETEXT") {
      probes.push(RowProbe {
        keep: true,
        enrichment: EsEnrichment::default(),
      });
      continue;
    }

    let es = pes.get(&row.pid).map(|v| v.as_slice()).unwrap_or(&[]);
    match enrich_for_codec(&row.codec_id, row.stream_type, es) {
      Some(enrichment) => probes.push(RowProbe { keep: true, enrichment }),
      None => probes.push(RowProbe::default()),
    }
  }
  probes
}

/// PARSER-170: probe the shared PES payload of a stream_type 0x83 TrueHD track.
/// Returns `(truehd_enrichment, ac3_enrichment)` — the first non-AC-3 sync
/// frame's params (TrueHD/MLP) and, when present, the first embedded AC-3
/// frame's params.  Mirrors `new_stream_a_truehd` (r_mpeg_ts.cpp:454-499).
fn probe_truehd_pair(es: Option<&[u8]>) -> (Option<EsEnrichment>, Option<EsEnrichment>) {
  let Some(es) = es else {
    return (None, None);
  };
  let frames = truehd::parse_frames(es);
  let mut thd: Option<EsEnrichment> = None;
  let mut ac3: Option<EsEnrichment> = None;
  for frame in frames {
    if frame.codec == truehd::Codec::Ac3 {
      if ac3.is_none() {
        ac3 = Some(EsEnrichment {
          channels: opt_pos(frame.channels),
          sampling_frequency: opt_rate(frame.sampling_rate),
          ..EsEnrichment::default()
        });
      }
    } else if thd.is_none() {
      thd = Some(EsEnrichment {
        channels: opt_pos(frame.channels),
        sampling_frequency: opt_rate(frame.sampling_rate),
        ..EsEnrichment::default()
      });
    }
    if thd.is_some() && ac3.is_some() {
      break;
    }
  }
  (thd, ac3)
}

fn opt_pos(v: u32) -> Option<u32> {
  if v > 0 { Some(v) } else { None }
}

fn opt_rate(v: u32) -> Option<f64> {
  if v > 0 { Some(v as f64) } else { None }
}

/// Decode the codec-specific header at the start of an elementary stream into
/// the parameters mkvmerge recovers from its bounded probe.  Returns `None`
/// (probe failed → drop the track) when no decodable header is found.
fn enrich_for_codec(codec_id: &str, stream_type: u8, es: &[u8]) -> Option<EsEnrichment> {
  match codec_id {
    "V_MPEG4/ISO/AVC" => {
      for nal in avc::nal::split_nal_units(es) {
        if nal.nal_unit_type == 7 {
          let rbsp = avc::nal::strip_emulation_prevention(nal.payload);
          if let Ok(sps) = avc::sps::parse(&rbsp) {
            return Some(EsEnrichment {
              pixel_dimensions: Some((sps.display_width, sps.display_height)),
              ..EsEnrichment::default()
            });
          }
        }
      }
      None
    }
    "V_MPEGH/ISO/HEVC" => {
      for nal in hevc::nal::split_nal_units(es) {
        // HEVC SPS_NUT == 33.
        if nal.nal_unit_type == 33 {
          let rbsp = hevc::nal::strip_emulation_prevention(nal.payload);
          if let Ok(sps) = hevc::sps::parse(&rbsp) {
            return Some(EsEnrichment {
              pixel_dimensions: Some((sps.display_width, sps.display_height)),
              ..EsEnrichment::default()
            });
          }
        }
      }
      None
    }
    "V_MPEG1" | "V_MPEG2" => mpeg_video::decode_sequence_header(es).map(|h| EsEnrichment {
      pixel_dimensions: Some((h.horizontal_size, h.vertical_size)),
      ..EsEnrichment::default()
    }),
    "V_VC1" => vc1::decode_sequence_header(es).map(|h| EsEnrichment {
      pixel_dimensions: Some((h.max_coded_width, h.max_coded_height)),
      ..EsEnrichment::default()
    }),
    "A_AC3" | "A_EAC3" => {
      // The PMT already tells us this is AC-3, so probe the first decodable
      // frame at a sync word rather than requiring several consecutive frames
      // (a bounded PES sample may carry only one).
      let frame = first_ac3_frame(es)?;
      Some(EsEnrichment {
        channels: Some(frame.channels),
        sampling_frequency: Some(frame.sample_rate as f64),
        ..EsEnrichment::default()
      })
    }
    "A_MPEG/L3" => {
      // PARSER-250: the PMT defaults stream types 0x03/0x04 to A_MPEG/L3, but
      // mkvtoolnix's `new_stream_a_mpeg` decodes the first frame header and
      // replaces the codec with `header.get_codec()` (r_mpeg_ts.cpp:354-357),
      // so Layer I / II audio is labelled correctly rather than as MP3.
      let (_, header) = mp3::find_consecutive_mp3_headers(es, 1)?;
      let (codec_id, codec_name) = mp3::codec_for_layer(header.layer);
      Some(EsEnrichment {
        channels: Some(header.channels),
        sampling_frequency: Some(header.sampling_frequency as f64),
        codec_override: Some((codec_id.to_string(), codec_name.to_string())),
        ..EsEnrichment::default()
      })
    }
    "A_AAC" => {
      // PARSER-206: require five consecutive AAC frames (ADTS *or* LOAS/LATM)
      // before trusting the header, mirroring `new_stream_a_aac`'s
      // `find_consecutive_frames(buffer, size, 5)` (r_mpeg_ts.cpp:367-378). This
      // recognises the LOAS/LATM multiplex that stream type 0x11 commonly
      // carries and rejects a lone accidental ADTS-looking sync.
      let header = first_aac_header(es)?;
      Some(EsEnrichment {
        channels: opt_pos(header.channels),
        sampling_frequency: opt_rate(header.sample_rate),
        ..EsEnrichment::default()
      })
    }
    // PARSER-170: DTS — decode the first DTS header for channels + sampling
    // frequency (r_mpeg_ts.cpp:411-425).
    "A_DTS" => {
      let header = dts::first_header_params(es)?;
      Some(EsEnrichment {
        channels: opt_pos(header.0),
        sampling_frequency: opt_rate(header.1),
        bits_per_sample: opt_pos(header.2),
        ..EsEnrichment::default()
      })
    }
    // PARSER-170: Blu-ray LPCM (stream_type 0x80) — the 4-byte BD LPCM header
    // sits at the very start of the PES elementary payload
    // (r_mpeg_ts.cpp:427-452).
    "A_PCM" if stream_type == 0x80 => decode_bd_lpcm_header(es),
    // PARSER-170: TextST — capture the dialog style segment as codec_private
    // (r_mpeg_ts.cpp:501-525).
    "S_HDMV/TEXTST" => decode_textst_codec_private(es),
    _ => None,
  }
}

/// PARSER-170: decode the 4-byte Blu-ray LPCM header that prefixes the PES
/// elementary payload.  Port of `new_stream_a_pcm` (r_mpeg_ts.cpp:427-452):
///
/// * `channels`         = `s_channels[buffer[2] >> 4]`
/// * `bits_per_sample`  = `{0,16,20,24}[buffer[3] >> 6]`
/// * `sample_rate`      from `buffer[2] & 0x0f`: 1→48000, 4→96000, 5→192000.
fn decode_bd_lpcm_header(es: &[u8]) -> Option<EsEnrichment> {
  const CHANNELS: [u32; 16] = [0, 1, 0, 2, 3, 3, 4, 4, 5, 6, 7, 8, 0, 0, 0, 0];
  const BITS: [u32; 4] = [0, 16, 20, 24];
  if es.len() < 4 {
    return None;
  }
  let channels = CHANNELS[(es[2] >> 4) as usize];
  let bits = BITS[(es[3] >> 6) as usize];
  let sample_rate = match es[2] & 0x0f {
    1 => 48_000,
    4 => 96_000,
    5 => 192_000,
    _ => 0,
  };
  // A header with no recoverable parameter is treated as a failed probe so the
  // track is dropped, matching mkvtoolnix's FILE_STATUS_MOREDATA path.
  if channels == 0 && sample_rate == 0 && bits == 0 {
    return None;
  }
  Some(EsEnrichment {
    channels: opt_pos(channels),
    sampling_frequency: opt_rate(sample_rate),
    bits_per_sample: opt_pos(bits),
    ..EsEnrichment::default()
  })
}

/// PARSER-170: build the TextST codec_private from the dialog style segment.
/// Port of `new_stream_s_hdmv_textst` (r_mpeg_ts.cpp:501-525): the segment
/// descriptor is `[type(1)=0x81][size(2 BE)]` followed by the segment data; the
/// codec_private is the whole `size + 3` byte block.
fn decode_textst_codec_private(es: &[u8]) -> Option<EsEnrichment> {
  if es.len() < 3 || es[0] != 0x81 {
    return None;
  }
  let dialog_segment_size = u16::from_be_bytes([es[1], es[2]]) as usize;
  let total = dialog_segment_size + 3;
  if total > es.len() {
    return None;
  }
  Some(EsEnrichment {
    codec_private: Some(es[..total].to_vec()),
    ..EsEnrichment::default()
  })
}

/// Number of consecutive AAC frames mkvtoolnix's `new_stream_a_aac` requires
/// before it trusts the stream (`find_consecutive_frames(..., 5)`,
/// r_mpeg_ts.cpp:367).
const AAC_REQUIRED_FRAMES: usize = 5;

/// PARSER-206: decode the first AAC header in `es`, requiring five consecutive
/// frames so a lone accidental ADTS-looking sync is rejected and so LOAS/LATM
/// multiplexed AAC (stream type 0x11) is recognised, not only ADTS. Port of
/// `new_stream_a_aac` (r_mpeg_ts.cpp:364-385) which calls
/// `aac::parser_c::find_consecutive_frames(buffer, size, 5)` and then reads the
/// first decoded frame's header (`parser.get_frame()`).
fn first_aac_header(es: &[u8]) -> Option<aac::AacHeader> {
  aac::find_first_header_with_frames(es, AAC_REQUIRED_FRAMES).map(|(_offset, header)| header)
}

/// Decode the first AC-3 frame found at a `0x0B77` sync word in `es`.  Unlike
/// [`ac3::find_frame_sync`], this does not require several confirming frames —
/// the PMT stream type already identified the codec.
fn first_ac3_frame(es: &[u8]) -> Option<ac3::Ac3Frame> {
  let mut i = 0;
  while i + 2 <= es.len() {
    if es[i] == 0x0B && es[i + 1] == 0x77 {
      if let Some(frame) = ac3::decode_frame(&es[i..]) {
        return Some(frame);
      }
    }
    i += 1;
  }
  None
}

fn handle_pat(section: &[u8], pmt_pids: &mut HashMap<u16, u16>) {
  if let Ok(pat) = pat::parse(section) {
    for entry in pat.entries {
      pmt_pids.insert(entry.pmt_pid, entry.program_number);
    }
  }
}

fn handle_pmt(
  section: &[u8],
  program_number: u16,
  rows: &mut Vec<StreamRow>,
  stream_pids: &mut std::collections::HashSet<u16>,
) {
  let Ok(pmt) = pmt::parse(section) else { return };
  let program_descriptors: DescriptorSummary = super::descriptors::walk(&pmt.program_descriptors);
  for entry in pmt.streams {
    stream_pids.insert(entry.elementary_pid);
    let new_rows = stream_table::build_rows(entry.elementary_pid, program_number, &entry, &program_descriptors);
    rows.extend(new_rows);
  }
  let _ = (pmt.program_number, pmt.pcr_pid);
}

/// Parse a DVB Service Description Table section (PARSER-055), recording each
/// service id → (provider, name).
fn handle_sdt(section: &[u8], sdt: &mut HashMap<u16, (String, String)>) {
  // table_id 0x42 = actual TS SDT.
  if section.first() != Some(&0x42) || section.len() < 12 {
    return;
  }
  let section_length = (((section[1] as usize) & 0x0F) << 8) | section[2] as usize;
  let end = (3 + section_length).min(section.len()).saturating_sub(4); // strip CRC
  let mut pos = 11usize;
  while pos + 5 <= end {
    let service_id = u16::from_be_bytes([section[pos], section[pos + 1]]);
    let loop_len = (((section[pos + 3] as usize) & 0x0F) << 8) | section[pos + 4] as usize;
    let desc_start = pos + 5;
    let desc_end = (desc_start + loop_len).min(end);
    let mut d = desc_start;
    while d + 2 <= desc_end {
      let tag = section[d];
      let len = section[d + 1] as usize;
      let body_start = d + 2;
      let body_end = (body_start + len).min(desc_end);
      if tag == 0x48 {
        if let Some((provider, name)) = service::decode_full(&section[body_start..body_end]) {
          sdt.entry(service_id).or_insert((provider, name));
        }
      }
      d = body_end;
    }
    pos = desc_end;
  }
}

/// Confidently sniff an elementary payload's codec for the unlisted-PID
/// fallback. Returns `None` unless a real codec header is found.
fn sniff_codec(payload: &[u8]) -> Option<(TrackKind, &'static str, &'static str)> {
  for nal in avc::nal::split_nal_units(payload) {
    if nal.nal_unit_type == 7 {
      let rbsp = avc::nal::strip_emulation_prevention(nal.payload);
      if avc::sps::parse(&rbsp).is_ok() {
        return Some((TrackKind::Video, "V_MPEG4/ISO/AVC", "AVC/H.264"));
      }
    }
  }
  if mpeg_video::decode_sequence_header(payload).is_some() {
    return Some((TrackKind::Video, "V_MPEG2", "MPEG-2 Video"));
  }
  if ac3::find_frame_sync(payload).is_some() {
    return Some((TrackKind::Audio, "A_AC3", "AC-3"));
  }
  if mp3::find_consecutive_mp3_headers(payload, 4).is_some() {
    return Some((TrackKind::Audio, "A_MPEG/L3", "MPEG-1/2 Audio"));
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::deadline::Deadline;
  use crate::media_metadata::mpeg_ts::packet::build_packet_with_pointer;
  use crate::media_metadata::mpeg_ts::pat::build_section as build_pat_section;
  use crate::media_metadata::mpeg_ts::pmt::build_section as build_pmt_section;
  use std::io::Cursor;

  fn dl() -> Deadline {
    Deadline::new(60_000)
  }

  fn padding() -> Vec<u8> {
    crate::media_metadata::mpeg_ts::packet::build_packet(0x1FFF, false, &[])
  }

  /// Wrap an elementary-stream payload in a minimal PES packet (extended-header
  /// form, no PTS/DTS) and emit it as a single TS packet on `pid`.  The ES must
  /// be small enough to fit one 188-byte TS packet (≤ ~175 bytes).
  fn pes_packet(pid: u16, es: &[u8]) -> Vec<u8> {
    let mut pes = vec![0x00, 0x00, 0x01, 0xE0]; // video-range stream id
    let pes_len = (3 + es.len()) as u16; // flags(2) + header_data_length(1) + ES
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80); // '10' marker + flags
    pes.push(0x00); // no PTS/DTS
    pes.push(0x00); // PES_header_data_length = 0
    pes.extend_from_slice(es);
    packet::build_packet(pid, true, &pes)
  }

  /// PARSER-169: a PMT-listed track now survives only when its bounded PES
  /// header probe succeeds, so `assemble_ts` embeds a real MPEG-2 sequence
  /// header (video PID 0x110) and a real ADTS frame (audio PID 0x111).
  fn assemble_ts(pmt_pid: u16) -> Vec<u8> {
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(
      1,
      pmt_pid,
      &[],
      &[
        (0x02, 0x110, vec![0x0A, 0x04, b'e', b'n', b'g', 0x00]), // MPEG-2 video
        (0x0F, 0x111, vec![]),                                   // AAC
      ],
    );
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    let video_es = mpeg_video::build_sequence_header(1280, 720, 4);
    // PARSER-206: AAC enrichment now requires five consecutive frames, so the
    // PES payload carries a multi-frame ADTS stream (sr_index 3 = 48 kHz, ch 2).
    let audio_es = aac::build_adts_stream(6, 1, 3, 2);
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(0x110, &video_es));
    bytes.extend(pes_packet(0x111, &audio_es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    bytes
  }

  #[test]
  fn probe_accepts_standard_188_byte_ts_stream() {
    let bytes = assemble_ts(0x100);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegTsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn probe_rejects_garbage() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 4096]));
    assert!(!MpegTsReader.probe(&mut s).unwrap());
  }

  // ---- PARSER-053: alignment / leading garbage --------------------------

  #[test]
  fn probe_accepts_stream_with_leading_garbage() {
    let mut bytes = vec![0xAAu8; 100]; // not a multiple of 188
    bytes.extend(assemble_ts(0x100));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(MpegTsReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_extracts_video_and_aac_tracks() {
    // PARSER-169: both PMT rows carry a decodable PES header, so both survive
    // the `probed_ok` filter.  The video row probes to its MPEG-2 dimensions
    // and the audio row to its AAC channels / sampling frequency.
    let bytes = assemble_ts(0x100);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(
      out.container.format,
      crate::media_metadata::model::container::ContainerFormat::MpegTs
    );
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(out.tracks[0].codec.id, "V_MPEG2");
    assert_eq!(out.tracks[1].codec.id, "A_AAC");
    // PARSER-171: `number` equals the elementary PID.
    assert_eq!(out.tracks[0].properties.common.number, Some(0x110));
    assert_eq!(out.tracks[1].properties.common.number, Some(0x111));
    let v = out.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(v.pixel_dimensions.as_ref().map(|d| (d.width, d.height)), Some((1280, 720)));
    let a = out.tracks[1].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
    let lang = out.tracks[0].properties.common.language.as_ref().unwrap();
    assert_eq!(lang.iso639_2, "eng");
    assert_eq!(out.container.properties.programs.len(), 1);
  }

  #[test]
  fn read_headers_drops_pmt_row_without_pes_payload() {
    // PARSER-169: a PMT entry advertised but never carrying a decodable PES
    // header must be dropped (mkvtoolnix `probed_ok` filter,
    // r_mpeg_ts.cpp:1547-1572).  Here only the video PID gets a PES payload;
    // the AAC PID is silent, so only the video track survives.
    let pmt_pid = 0x100u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x02, 0x110, vec![]), (0x0F, 0x111, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    let video_es = mpeg_video::build_sequence_header(1920, 1080, 4);
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(0x110, &video_es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "V_MPEG2");
    assert_eq!(out.tracks[0].id, 0);
  }

  // ---- PARSER-054: PMT split across packets -----------------------------

  #[test]
  fn read_headers_reassembles_pmt_across_packets() {
    let pmt_pid = 0x100u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);

    let mut streams = Vec::new();
    for i in 0..40u16 {
      if i % 2 == 0 {
        streams.push((0x1Bu8, 0x200 + i, vec![]));
      } else {
        streams.push((0x0Fu8, 0x200 + i, vec![]));
      }
    }
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &streams);
    assert!(pmt_section.len() > 184, "section must exceed one packet payload");

    // Split into TS packets: head carries the pointer + start.
    let mut full = vec![0u8]; // pointer_field = 0
    full.extend_from_slice(&pmt_section);
    let payload_len = 184usize;
    let mut bytes = pat_pkt;
    let mut cc = 0u8;
    let mut idx = 0usize;
    while idx < full.len() {
      let chunk_end = (idx + payload_len).min(full.len());
      let chunk = &full[idx..chunk_end];
      let mut p = vec![packet::TS_SYNC_BYTE];
      let pusi = if idx == 0 { 0x40 } else { 0x00 };
      p.push(pusi | ((pmt_pid >> 8) as u8 & 0x1F));
      p.push((pmt_pid & 0xFF) as u8);
      p.push(0x10 | (cc & 0x0F));
      p.extend_from_slice(chunk);
      while p.len() < 188 {
        p.push(0xFF);
      }
      bytes.extend(p);
      cc = (cc + 1) & 0x0F;
      idx = chunk_end;
    }
    // PARSER-169: PMT rows now survive only when their PES header probes
    // successfully.  Give the second stream (an AAC PID, 0x201) a real ADTS
    // PES payload.  The unlisted-PID fallback sniffer recognises only
    // AVC/MPEG-2/AC-3/MP3 — never AAC — so an A_AAC track on 0x201 can only
    // appear if the PMT (split across two packets) was correctly reassembled
    // and classified that PID as AAC.
    bytes.extend(pes_packet(0x201, &aac::build_adts_stream(6, 1, 3, 2)));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    // Only the AAC PID carried a decodable PES payload; the other 39 silent
    // rows are dropped by the probed_ok filter.
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    assert_eq!(out.tracks[0].properties.common.number, Some(0x201));
  }

  // ---- PARSER-055: SDT service name -------------------------------------

  #[test]
  fn read_headers_applies_sdt_service_name() {
    let provider = b"ACME";
    let name = b"Channel One";
    let mut desc_body = vec![0x01u8]; // service_type
    desc_body.push(provider.len() as u8);
    desc_body.extend_from_slice(provider);
    desc_body.push(name.len() as u8);
    desc_body.extend_from_slice(name);
    let mut descriptor = vec![0x48u8, desc_body.len() as u8];
    descriptor.extend_from_slice(&desc_body);

    let mut svc = Vec::new();
    svc.extend_from_slice(&1u16.to_be_bytes()); // service_id = 1
    svc.push(0x00); // EIT flags
    let loop_len = descriptor.len();
    svc.push(((loop_len >> 8) as u8) & 0x0F);
    svc.push((loop_len & 0xFF) as u8);
    svc.extend_from_slice(&descriptor);

    let mut body = Vec::new();
    body.extend_from_slice(&1u16.to_be_bytes()); // transport_stream_id
    body.push(0xC1); // version/current_next
    body.push(0); // section_number
    body.push(0); // last_section_number
    body.extend_from_slice(&1u16.to_be_bytes()); // original_network_id
    body.push(0xFF); // reserved
    body.extend_from_slice(&svc);
    body.extend_from_slice(&0u32.to_be_bytes()); // CRC placeholder

    let mut section = vec![0x42u8];
    let section_length = body.len();
    section.push(0x80 | ((section_length >> 8) as u8 & 0x0F));
    section.push((section_length & 0xFF) as u8);
    section.extend_from_slice(&body);

    let sdt_pkt = build_packet_with_pointer(SDT_PID, &section);

    let mut bytes = assemble_ts(0x100);
    bytes.extend(sdt_pkt);
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    let prog = &out.container.properties.programs[0];
    assert_eq!(prog.service_name.as_deref(), Some("Channel One"));
    assert_eq!(prog.service_provider.as_deref(), Some("ACME"));
  }

  #[test]
  fn read_headers_enriches_ac3_audio_from_pes_payload() {
    // PARSER-158: a Blu-ray AC-3 stream (stream_type 0x81) carrying a real
    // AC-3 frame in its PES payload must surface channels + sampling frequency
    // probed from that frame, not an empty audio struct.
    let pmt_pid = 0x100u16;
    let ac3_pid = 0x111u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x81, ac3_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);

    // fscod=0 (48 kHz), bsid=8 (AC-3), acmod=7 (3/2) + LFE → 6 channels.
    let frame = crate::media_metadata::audio::ac3::build_ac3_frame_full(0, 0, 8, 7, true);
    let mut pes = vec![0x00, 0x00, 0x01, 0xBD]; // private_stream_1
    let pes_len = (3 + frame.len()) as u16; // flags(2) + header_data_length(1) + ES
    pes.extend_from_slice(&pes_len.to_be_bytes());
    pes.push(0x80); // '10' marker + flags
    pes.push(0x00); // no PTS/DTS
    pes.push(0x00); // PES_header_data_length = 0
    pes.extend_from_slice(&frame);
    let pes_pkt = packet::build_packet(ac3_pid, true, &pes);

    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_pkt);
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_AC3");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(6));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn strip_pes_header_handles_both_stream_forms() {
    // Extended-header stream (video 0xE0): ES starts at 9 + header_data_len.
    let mut pes = vec![0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x80, 0x00, 0x02, 0xAA, 0xBB];
    pes.extend_from_slice(&[0x11, 0x22, 0x33]); // ES
    assert_eq!(strip_pes_header(&pes), &[0x11, 0x22, 0x33]);

    // Padding stream (0xBE) carries no optional header — ES right after byte 6.
    let pad = vec![0x00, 0x00, 0x01, 0xBE, 0x00, 0x00, 0x44, 0x55];
    assert_eq!(strip_pes_header(&pad), &[0x44, 0x55]);

    // Non-PES bytes pass through unchanged.
    assert_eq!(strip_pes_header(&[0x01, 0x02]), &[0x01, 0x02]);
  }

  #[test]
  fn enrich_for_codec_decodes_each_supported_codec() {
    use crate::media_metadata::audio::{aac, ac3, mp3};
    use crate::media_metadata::elementary::{mpeg_video, vc1};

    // MPEG-2 video → pixel dimensions.
    let es = mpeg_video::build_sequence_header(1280, 720, 4);
    assert_eq!(enrich_for_codec("V_MPEG2", 0x02, &es).unwrap().pixel_dimensions, Some((1280, 720)));

    // VC-1 → pixel dimensions.
    let es = vc1::build_sequence_header(1920, 1080);
    assert_eq!(enrich_for_codec("V_VC1", 0xEA, &es).unwrap().pixel_dimensions, Some((1920, 1080)));

    // MP3 → channels + sampling frequency + Layer III codec override.
    let es = mp3::build_mp3_frame_v1(128, 44100, false);
    let m = enrich_for_codec("A_MPEG/L3", 0x03, &es).unwrap();
    assert_eq!(m.channels, Some(2));
    assert_eq!(m.sampling_frequency, Some(44100.0));
    assert_eq!(
      m.codec_override,
      Some(("A_MPEG/L3".to_string(), "MP3".to_string()))
    );

    // AAC (ADTS, sr_index 3 = 48 kHz, channel_config 2) — five consecutive
    // frames are required (PARSER-206).
    let es = aac::build_adts_stream(6, 1, 3, 2);
    let m = enrich_for_codec("A_AAC", 0x0F, &es).unwrap();
    assert_eq!(m.channels, Some(2));
    assert_eq!(m.sampling_frequency, Some(48000.0));

    // AC-3 (acmod 7 + LFE = 6 channels, 48 kHz).
    let es = ac3::build_ac3_frame_full(0, 0, 8, 7, true);
    let m = enrich_for_codec("A_AC3", 0x81, &es).unwrap();
    assert_eq!(m.channels, Some(6));
    assert_eq!(m.sampling_frequency, Some(48000.0));

    // Unknown / unprobeable codec yields nothing.
    assert!(enrich_for_codec("S_HDMV/PGS", 0x90, &es).is_none());
    assert!(enrich_for_codec("A_AC3", 0x81, &[0u8; 4]).is_none());
  }

  #[test]
  fn mpeg_audio_layer_two_specialises_codec() {
    // PARSER-250: a stream type 0x04 carrying Layer II audio is relabelled from
    // the table default (A_MPEG/L3) to A_MPEG/L2 once the frame header decodes,
    // mirroring `new_stream_a_mpeg`'s `header.get_codec()`.
    use crate::media_metadata::audio::mp3;
    let es = mp3::build_mp3_frame(1, 2, 128, 44100, false);
    let m = enrich_for_codec("A_MPEG/L3", 0x04, &es).unwrap();
    assert_eq!(
      m.codec_override,
      Some(("A_MPEG/L2".to_string(), "MP2".to_string()))
    );
    assert_eq!(m.channels, Some(2));
  }

  // ---- PARSER-206: AAC requires five consecutive frames + LOAS/LATM ------

  #[test]
  fn enrich_aac_rejects_single_isolated_adts_header() {
    // A lone ADTS-looking header followed by garbage must NOT enrich — the
    // five-consecutive-frame requirement (find_consecutive_frames(.., 5))
    // rejects an accidental sync.
    let mut es = aac::build_adts_frame(1, 3, 2);
    es.extend(vec![0u8; 64]);
    assert!(enrich_for_codec("A_AAC", 0x0F, &es).is_none());
  }

  #[test]
  fn enrich_aac_rejects_two_frames_when_five_required() {
    let es = aac::build_adts_stream(2, 1, 3, 2);
    assert!(enrich_for_codec("A_AAC", 0x0F, &es).is_none());
  }

  #[test]
  fn enrich_aac_accepts_loas_latm_on_stream_type_0x11() {
    // PARSER-206: stream type 0x11 commonly carries LOAS/LATM-framed AAC. Five
    // consecutive LOAS frames (44.1 kHz, mono) must be recognised as A_AAC and
    // surface channels + sampling frequency, not just ADTS.
    let es = aac::build_loas_latm_stream(6, 4, 1); // sr_index 4 = 44.1 kHz, ch 1
    let m = enrich_for_codec("A_AAC", 0x11, &es).unwrap();
    assert_eq!(m.channels, Some(1));
    assert_eq!(m.sampling_frequency, Some(44100.0));
  }

  #[test]
  fn read_headers_extracts_loas_latm_aac_track() {
    // End-to-end: a PMT advertising stream type 0x11 whose PES payload carries
    // LOAS/LATM AAC must survive the probed_ok filter and be classified A_AAC.
    let pmt_pid = 0x100u16;
    let aac_pid = 0x111u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x11, aac_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    let audio_es = aac::build_loas_latm_stream(6, 3, 2); // 48 kHz, stereo
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(aac_pid, &audio_es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  #[test]
  fn read_headers_aac_spanning_two_pes_packets_survives() {
    // PARSER-271: four ADTS frames per PES packet, with each PES exactly filling
    // its 188-byte TS packet (175-byte ES + 9-byte PES header → 184-byte
    // payload, no stuffing) so the frame groups are contiguous after the PES
    // headers are stripped.  Five consecutive frames are required, so the AAC
    // probe must span the PES boundary: each PES header has to be stripped at
    // its own packet so the second `00 00 01 E0 …` header is never injected
    // between frames 4 and 5.  Only the PMT can classify this PID as AAC (the
    // unlisted-PID fallback never recognises AAC), so the surviving A_AAC track
    // proves both reassembly and the per-PES header strip.
    let pmt_pid = 0x100u16;
    let aac_pid = 0x111u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x0F, aac_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    // 4 frames per PES (44+44+44+43 = 175 bytes), all 48 kHz stereo.
    let mut es = Vec::new();
    for _ in 0..3 {
      es.extend(aac::build_adts_frame_with_len(1, 3, 2, 44));
    }
    es.extend(aac::build_adts_frame_with_len(1, 3, 2, 43));
    assert_eq!(es.len(), 175, "ES must fill the TS payload exactly");
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(aac_pid, &es));
    bytes.extend(pes_packet(aac_pid, &es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_AAC");
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.sampling_frequency, Some(48000.0));
  }

  // ---- PARSER-170: DTS / Blu-ray LPCM / TrueHD / TextST enrichment ------

  #[test]
  fn enrich_for_codec_decodes_dts() {
    // amode 6 = 3 channels, sfreq idx 13 = 48 kHz, source_pcm_resolution 16.
    let es = crate::media_metadata::audio::dts::build_dts_core_frame(6, 13);
    let m = enrich_for_codec("A_DTS", 0x82, &es).unwrap();
    assert_eq!(m.channels, Some(3));
    assert_eq!(m.sampling_frequency, Some(48000.0));
    assert_eq!(m.bits_per_sample, Some(16));
    // Garbage DTS payload → failed probe.
    assert!(enrich_for_codec("A_DTS", 0x82, &[0u8; 32]).is_none());
  }

  #[test]
  fn decode_bd_lpcm_header_decodes_channels_rate_bits() {
    // buffer[2] >> 4 = channels index, buffer[2] & 0x0f = rate code,
    // buffer[3] >> 6 = bits index.  Pick: ch index 9 → 6 channels, rate 4 →
    // 96 kHz, bits index 3 → 24 bits.
    let es = [0x00u8, 0x00, (9 << 4) | 0x04, 3 << 6];
    let m = decode_bd_lpcm_header(&es).unwrap();
    assert_eq!(m.channels, Some(6));
    assert_eq!(m.sampling_frequency, Some(96000.0));
    assert_eq!(m.bits_per_sample, Some(24));
    // Routed through enrich_for_codec only for stream_type 0x80.
    let m = enrich_for_codec("A_PCM", 0x80, &es).unwrap();
    assert_eq!(m.channels, Some(6));
    // All-zero header carries no parameters → failed probe.
    assert!(decode_bd_lpcm_header(&[0u8; 4]).is_none());
    assert!(decode_bd_lpcm_header(&[0u8; 3]).is_none());
  }

  #[test]
  fn decode_textst_codec_private_captures_dialog_style_segment() {
    // [0x81][size=4 BE][4 bytes of data] → codec_private = 7 bytes.
    let es = [0x81u8, 0x00, 0x04, 0xDE, 0xAD, 0xBE, 0xEF, 0xFF, 0xFF];
    let m = decode_textst_codec_private(&es).unwrap();
    assert_eq!(m.codec_private.as_deref(), Some(&[0x81u8, 0x00, 0x04, 0xDE, 0xAD, 0xBE, 0xEF][..]));
    // Wrong segment type or truncation → failed probe.
    assert!(decode_textst_codec_private(&[0x80u8, 0x00, 0x04, 0, 0, 0, 0]).is_none());
    assert!(decode_textst_codec_private(&[0x81u8, 0x00, 0x10, 0xDE]).is_none());
    assert!(decode_textst_codec_private(&[0x81u8]).is_none());
  }

  #[test]
  fn probe_truehd_pair_finds_truehd_and_coupled_ac3() {
    use crate::media_metadata::audio::ac3::build_ac3_frame;
    use crate::media_metadata::audio::truehd::build_truehd_frame;
    let mut data = build_truehd_frame(1, 0b1111); // 96 kHz, 6 channels
    data.extend(build_truehd_frame(1, 0b1111));
    data.extend(build_ac3_frame(0, 8)); // AC-3 48 kHz
    let (thd, ac3) = probe_truehd_pair(Some(&data));
    let thd = thd.unwrap();
    assert_eq!(thd.channels, Some(6));
    assert_eq!(thd.sampling_frequency, Some(96000.0));
    let ac3 = ac3.unwrap();
    assert_eq!(ac3.sampling_frequency, Some(48000.0));

    // TrueHD with no embedded AC-3 → the coupled probe is None.
    let mut only_thd = build_truehd_frame(1, 0b1111);
    only_thd.extend(build_truehd_frame(1, 0b1111));
    let (thd, ac3) = probe_truehd_pair(Some(&only_thd));
    assert!(thd.is_some());
    assert!(ac3.is_none());

    // No payload → both None.
    assert_eq!(probe_truehd_pair(None).0.is_none(), true);
  }

  #[test]
  fn read_headers_truehd_with_coupled_ac3_keeps_both() {
    // PARSER-169 / PARSER-170: stream_type 0x83 emits a TrueHD primary + an
    // AC-3 coupled row.  When the PES carries both a TrueHD sync and an
    // embedded AC-3 frame, both survive the probed_ok filter.
    use crate::media_metadata::audio::ac3::build_ac3_frame;
    use crate::media_metadata::audio::truehd::build_truehd_frame;
    let pmt_pid = 0x100u16;
    let thd_pid = 0x120u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x83, thd_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    // Keep the ES small enough to fit one 188-byte TS packet: one 32-byte
    // TrueHD sync frame + one 128-byte AC-3 frame (frmsizecod 0 @ 48 kHz).
    let mut es = build_truehd_frame(1, 0b1111);
    es.extend(build_ac3_frame(0, 0));
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(thd_pid, &es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 2);
    assert_eq!(out.tracks[0].codec.id, "A_TRUEHD");
    assert_eq!(out.tracks[1].codec.id, "A_AC3");
    let thd = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(thd.channels, Some(6));
    assert_eq!(thd.sampling_frequency, Some(96000.0));
  }

  #[test]
  fn read_headers_truehd_without_coupled_ac3_drops_ac3_row() {
    // PARSER-169: when only a TrueHD sync (no embedded AC-3) is present, the
    // primary survives but the coupled AC-3 row is dropped
    // (r_mpeg_ts.cpp:1554-1567).
    use crate::media_metadata::audio::truehd::build_truehd_frame;
    let pmt_pid = 0x100u16;
    let thd_pid = 0x120u16;
    let pat_section = build_pat_section(1, &[(1, pmt_pid)]);
    let pat_pkt = build_packet_with_pointer(0, &pat_section);
    let pmt_section = build_pmt_section(1, pmt_pid, &[], &[(0x83, thd_pid, vec![])]);
    let pmt_pkt = build_packet_with_pointer(pmt_pid, &pmt_section);
    let mut es = build_truehd_frame(1, 0b1111);
    es.extend(build_truehd_frame(1, 0b1111));
    let mut bytes = pat_pkt;
    bytes.extend(pmt_pkt);
    bytes.extend(pes_packet(thd_pid, &es));
    for _ in 0..6 {
      bytes.extend(padding());
    }
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.ts", 0);
    MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap();
    assert_eq!(out.tracks.len(), 1);
    assert_eq!(out.tracks[0].codec.id, "A_TRUEHD");
  }

  #[test]
  fn short_input_returns_unrecognised() {
    let mut s = FileSource::from_reader_for_test(Cursor::new(vec![0u8; 64]));
    let mut out = MediaMetadata::new("clip.ts", 0);
    let err = MpegTsReader.read_headers(&mut s, &dl(), &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }
}
