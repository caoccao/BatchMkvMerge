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

//! Reader registry + probe cascade.
//!
//! Mirrors mkvtoolnix's `probe_file_format` — a six-phase fallthrough:
//! unambiguous magics → extension hints → text subtitles → strict elementary
//! streams → frame-scan audio → ambiguous formats. The cascade walks every
//! registered reader in priority order
//! and asks it to `probe`. The first reader that claims the file is asked to
//! `read_headers` and the result is returned to the caller.
//!
//! Phase 3 ships with the Matroska reader as the only registered entry —
//! every subsequent format reader slots in here without changing the
//! cascade's shape. The probe outcome is reported via [`DispatchOutcome`] so
//! the public `parse` entry point can distinguish "no reader claimed" from
//! "a reader claimed but then failed mid-parse".

use crate::media_metadata::audio::{
  AacReader, Ac3Reader, DtsReader, FlacReader, Mp3Reader, TrueHdReader, TtaReader, WavReader, WavpackReader,
};
use crate::media_metadata::avi::AviReader;
use crate::media_metadata::coreaudio::CoreAudioReader;
use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::elementary::{
  AvcReader, DiracReader, DvReader, HevcReader, MpegVideoReader, ObuReader, Vc1Reader,
};
use crate::media_metadata::error::ParseError;
use crate::media_metadata::flv::FlvReader;
use crate::media_metadata::io::FileSource;
use crate::media_metadata::ivf::IvfReader;
use crate::media_metadata::matroska::MatroskaReader;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::mp4::Mp4Reader;
use crate::media_metadata::mpeg_ps::MpegPsReader;
use crate::media_metadata::mpeg_ts::MpegTsReader;
use crate::media_metadata::ogg::OggReader;
use crate::media_metadata::reader::Reader;
use crate::media_metadata::realmedia::RealMediaReader;
use crate::media_metadata::subtitles::{
  HdmvTextStReader, MicroDvdReader, PgsReader, SrtReader, SsaReader, UsfReader, VobButtonReader, VobSubReader,
  WebVttReader,
};

/// Describes what the cascade did and why. `Claimed` means a reader's
/// `probe()` returned `true` — independent of whether `read_headers` then
/// succeeded. `NoMatch` means every registered reader rejected the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
  /// A reader claimed the file and `read_headers` was attempted. The
  /// `&'static str` is the reader name (e.g. `"matroska"`).
  Claimed(&'static str),
  /// No registered reader recognised the file.
  NoMatch,
}

/// Walk the registered readers in priority order. On the first `probe()` that
/// returns `Ok(true)`, hand off to `read_headers` and propagate its result.
/// If every reader rejects the file, return `Err(ParseError::Unrecognised)`.
///
/// The cursor is rewound between probes so each reader gets a fresh view of
/// the start of the file.
pub fn dispatch(
  src: &mut FileSource,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<DispatchOutcome, ParseError> {
  dispatch_with_hints(src, deadline, out, &[])
}

/// Like [`dispatch`], but additionally biases the reader cascade so that
/// readers whose names match any of `hints` are tried first.  Mirrors
/// mkvtoolnix's "extension-hinted" phase
/// (`reader_detection_and_creation.cpp:302-310`).  PARSER-062.
pub fn dispatch_with_hints(
  src: &mut FileSource,
  deadline: &Deadline,
  out: &mut MediaMetadata,
  hints: &[super::extension_hint::FileTypeHint],
) -> Result<DispatchOutcome, ParseError> {
  // PARSER-063: unsupported-signature prober runs first.  mkvtoolnix calls
  // this at the top of `probe_file_format`
  // (`reader_detection_and_creation.cpp:266`) so that ADIF AAC / ASF /
  // CDXA / HD-Sub / IVR / WinTV DVR files are reported as recognised but
  // unsupported instead of unrecognised.
  src.seek_to(0)?;
  deadline.check("probe")?;
  if let Some(format) = super::unsupported::probe(src)? {
    out.container.format = format;
    out.container.recognized = true;
    out.container.supported = false;
    return Ok(DispatchOutcome::Claimed("unsupported"));
  }
  let mut order: Vec<&'static (dyn Reader + Send + Sync)> = Vec::new();
  let hinted_names = hints_to_reader_names(hints);
  // PARSER-062: front-load every reader whose name matches an extension hint
  // so ambiguous formats (`.mp4` → MP4 / AAC / ALAC; `.ogg` → Ogg / FLAC)
  // are tried in the order the extension implies.  The remaining readers run
  // in their original priority order.
  for reader in registered_readers() {
    if hinted_names.iter().any(|n| *n == reader.name()) {
      order.push(*reader);
    }
  }
  for reader in registered_readers() {
    if !order.iter().any(|r| std::ptr::eq(*r, *reader)) {
      order.push(*reader);
    }
  }
  for reader in order {
    // Each probe call must see a freshly-positioned cursor; the trait
    // contract requires probes to rewind on return, but we re-seek defensively
    // so a misbehaving probe can't leak position across registry entries.
    src.seek_to(0)?;
    deadline.check("probe")?;
    let claimed = reader.probe(src)?;
    if !claimed {
      continue;
    }
    src.seek_to(0)?;
    reader.read_headers(src, deadline, out)?;
    return Ok(DispatchOutcome::Claimed(reader.name()));
  }
  Err(ParseError::Unrecognised)
}

/// Translate [`FileTypeHint`] values into the reader names registered in
/// [`registered_readers`].  A hint may resolve to multiple reader names when
/// the extension is genuinely ambiguous (e.g. `mp4` → mp4 + aac, `ogg` → ogg
/// + flac).
fn hints_to_reader_names(hints: &[super::extension_hint::FileTypeHint]) -> Vec<&'static str> {
  use super::extension_hint::FileTypeHint;
  let mut names = Vec::new();
  for hint in hints {
    let resolved: &[&'static str] = match hint {
      FileTypeHint::Aac => &["aac"],
      FileTypeHint::Ac3 => &["ac3"],
      FileTypeHint::AvcEs => &["avc"],
      FileTypeHint::Avi => &["avi"],
      FileTypeHint::CoreAudio => &["coreaudio"],
      FileTypeHint::Dirac => &["dirac"],
      FileTypeHint::Dts => &["dts"],
      FileTypeHint::Dv => &["dv"],
      FileTypeHint::Flac => &["flac"],
      FileTypeHint::Flv => &["flv"],
      FileTypeHint::HevcEs => &["hevc"],
      FileTypeHint::Ivf => &["ivf"],
      FileTypeHint::Matroska => &["matroska"],
      FileTypeHint::MicroDvd => &["microdvd"],
      FileTypeHint::Mp3 => &["mp3"],
      FileTypeHint::MpegEs => &["mpeg_video"],
      FileTypeHint::MpegPs => &["mpeg_ps"],
      FileTypeHint::MpegTs => &["mpeg_ts"],
      FileTypeHint::Ogm => &["ogg"],
      FileTypeHint::PgsSup => &["pgs"],
      FileTypeHint::QtMp4 => &["mp4"],
      FileTypeHint::Real => &["realmedia"],
      FileTypeHint::Srt => &["srt"],
      FileTypeHint::Ssa => &["ssa"],
      FileTypeHint::TrueHd => &["truehd"],
      FileTypeHint::Tta => &["tta"],
      FileTypeHint::Usf => &["usf"],
      FileTypeHint::Vc1 => &["vc1"],
      FileTypeHint::VobButton => &["vobbtn"],
      FileTypeHint::VobSub => &["vobsub"],
      FileTypeHint::Wav => &["wav"],
      FileTypeHint::Wavpack4 => &["wavpack"],
      FileTypeHint::WebVtt => &["webvtt"],
      FileTypeHint::HdmvTextSt => &["hdmv_textst"],
      FileTypeHint::Obu => &["obu"],
      FileTypeHint::Alac => &["mp4", "coreaudio"],
      // Hint values without dedicated readers — no-op.
      FileTypeHint::Asf
      | FileTypeHint::Cdxa
      | FileTypeHint::Chapters
      | FileTypeHint::HdSub
      | FileTypeHint::AviDv1
      | FileTypeHint::BlurayPlaylist => &[],
    };
    for n in resolved {
      if !names.contains(n) {
        names.push(*n);
      }
    }
  }
  names
}

/// The active reader registry. Order matches mkvtoolnix's probe cascade so
/// adding a reader is a one-line insert at the right priority level.
pub fn registered_readers() -> &'static [&'static (dyn Reader + Send + Sync)] {
  // Static dispatch table.  The lifetime is `'static` because every entry
  // is a zero-sized unit struct; no allocation involved.  `Send + Sync`
  // bounds let the static live in a multi-threaded process.
  static MATROSKA: MatroskaReader = MatroskaReader;
  static MP4: Mp4Reader = Mp4Reader;
  static AVI: AviReader = AviReader;
  static OGG: OggReader = OggReader;
  static MPEG_PS: MpegPsReader = MpegPsReader;
  static MPEG_TS: MpegTsReader = MpegTsReader;
  static FLV: FlvReader = FlvReader;
  static REALMEDIA: RealMediaReader = RealMediaReader;
  static IVF: IvfReader = IvfReader;

  // Magic-byte audio formats (single-FOURCC probes) — these go before the
  // frame-sync probes because their magic bytes don't collide.
  static FLAC: FlacReader = FlacReader;
  static WAV: WavReader = WavReader;
  static WAVPACK: WavpackReader = WavpackReader;
  static TTA: TtaReader = TtaReader;
  static CORE_AUDIO: CoreAudioReader = CoreAudioReader;
  static TRUEHD: TrueHdReader = TrueHdReader;

  // Frame-sync audio formats — order mirrors mkvtoolnix's probe cascade:
  // AC-3 before MP3 (MPEG sync `0xFFE` and AC-3 sync `0x0B77` don't
  // collide, but MP3 must beat AAC because of the shared `0xFFF` prefix).
  static AC3: Ac3Reader = Ac3Reader;
  static DTS: DtsReader = DtsReader;
  static MP3: Mp3Reader = Mp3Reader;
  static AAC: AacReader = AacReader;

  // Elementary video streams.  AVC + HEVC require an SPS NAL within the
  // first ~64 KB; MPEG / VC-1 / Dirac / DV need their fixed start code at
  // offset 0; AV1 OBU needs a sequence_header or temporal_delimiter as
  // the first byte.
  static AVC: AvcReader = AvcReader;
  static HEVC: HevcReader = HevcReader;
  static MPEG_VIDEO: MpegVideoReader = MpegVideoReader;
  static VC1: Vc1Reader = Vc1Reader;
  static DIRAC: DiracReader = DiracReader;
  static DV: DvReader = DvReader;
  static OBU: ObuReader = ObuReader;

  // Subtitle readers.  Image / segment-based formats (PGS, TextST, VobSub,
  // VobButton) probe first because their magic bytes are unambiguous; the
  // text-based formats (SRT, SSA, WebVTT, USF, MicroDVD) probe last so they
  // don't claim binary streams whose decoded UTF-8 happens to look like a
  // valid timecode line.
  static PGS: PgsReader = PgsReader;
  static HDMV_TEXTST: HdmvTextStReader = HdmvTextStReader;
  static VOBBTN: VobButtonReader = VobButtonReader;
  static VOBSUB: VobSubReader = VobSubReader;
  static WEBVTT: WebVttReader = WebVttReader;
  static USF: UsfReader = UsfReader;
  static SSA: SsaReader = SsaReader;
  static SRT: SrtReader = SrtReader;
  static MICRODVD: MicroDvdReader = MicroDvdReader;

  static REGISTRY: &[&'static (dyn Reader + Send + Sync)] = &[
    &MATROSKA,
    &AVI,
    &OGG,
    &MP4,
    &MPEG_PS,
    &MPEG_TS,
    &FLV,
    &REALMEDIA,
    &IVF,
    &FLAC,
    &WAV,
    &WAVPACK,
    &TTA,
    &CORE_AUDIO,
    &TRUEHD,
    &MPEG_VIDEO,
    &VC1,
    &DIRAC,
    &DV,
    &AVC,
    &HEVC,
    &OBU,
    &PGS,
    &HDMV_TEXTST,
    &VOBBTN,
    &VOBSUB,
    &WEBVTT,
    &USF,
    &SSA,
    &SRT,
    &MICRODVD,
    &AC3,
    &DTS,
    &MP3,
    &AAC,
  ];
  REGISTRY
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  fn src_for(bytes: &[u8]) -> FileSource {
    FileSource::from_reader_for_test(Cursor::new(bytes.to_vec()))
  }

  #[test]
  fn registry_contains_at_least_matroska_in_phase_3() {
    let names: Vec<&'static str> = registered_readers().iter().map(|r| r.name()).collect();
    assert!(
      names.contains(&"matroska"),
      "expected matroska in registry, got {names:?}"
    );
  }

  #[test]
  fn dispatch_returns_unrecognised_on_garbage() {
    // 16 bytes of nothing recognisable.
    let mut src = src_for(&[0xAB; 16]);
    let deadline = Deadline::new(60_000);
    let mut out = MediaMetadata::new("garbage", 16);
    let err = dispatch(&mut src, &deadline, &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn dispatch_returns_unrecognised_on_empty_input() {
    let mut src = src_for(&[]);
    let deadline = Deadline::new(60_000);
    let mut out = MediaMetadata::new("empty", 0);
    let err = dispatch(&mut src, &deadline, &mut out).unwrap_err();
    assert!(matches!(err, ParseError::Unrecognised));
  }

  #[test]
  fn dispatch_does_not_consume_budget_per_reader_check() {
    // The probe check itself bumps the deadline-check counter once; verify
    // we don't blow the budget when the registry is short and probes are
    // cheap.  We use a 1 s budget and assert the call returns quickly.
    let mut src = src_for(&[0; 16]);
    let deadline = Deadline::new(1_000);
    let mut out = MediaMetadata::new("garbage", 16);
    let _ = dispatch(&mut src, &deadline, &mut out);
    assert!(deadline.check("post-dispatch").is_ok());
  }

  // ---- PARSER-063: unsupported signatures ----------------------------

  #[test]
  fn dispatch_marks_asf_signature_recognised_but_unsupported() {
    let bytes = vec![0x30, 0x26, 0xB2, 0x75, 0x00, 0x00, 0x00, 0x00];
    let mut src = src_for(&bytes);
    let deadline = Deadline::new(60_000);
    let mut out = MediaMetadata::new("clip.wma", bytes.len() as u64);
    let outcome = dispatch(&mut src, &deadline, &mut out).unwrap();
    assert!(matches!(outcome, DispatchOutcome::Claimed("unsupported")));
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert_eq!(
      out.container.format,
      crate::media_metadata::model::container::ContainerFormat::Asf
    );
  }

  #[test]
  fn dispatch_marks_adif_aac_signature_recognised_but_unsupported() {
    let bytes = b"ADIF\x00\x00\x00\x00".to_vec();
    let mut src = src_for(&bytes);
    let deadline = Deadline::new(60_000);
    let mut out = MediaMetadata::new("clip.aac", bytes.len() as u64);
    dispatch(&mut src, &deadline, &mut out).unwrap();
    assert!(out.container.recognized);
    assert!(!out.container.supported);
    assert_eq!(
      out.container.format,
      crate::media_metadata::model::container::ContainerFormat::Aac
    );
  }

  #[test]
  fn dispatch_outcome_is_claimed_for_matroska_signature() {
    // Minimal byte sequence that matches the EBML signature.  The actual
    // matroska reader will then attempt full header parse and likely fail
    // on incomplete data, so we expect a Malformed / UnexpectedEof here —
    // *not* Unrecognised.  This proves the registry routed us to matroska.
    let mut head: Vec<u8> = vec![0x1A, 0x45, 0xDF, 0xA3]; // EBML id
    // Followed by an obviously-truncated payload size
    head.extend_from_slice(&[0x80]); // size = 0
    let mut src = src_for(&head);
    let deadline = Deadline::new(60_000);
    let mut out = MediaMetadata::new("matroska-stub", head.len() as u64);
    let result = dispatch(&mut src, &deadline, &mut out);
    // Either it succeeds (unlikely on this stub) or it errors with
    // something other than Unrecognised — both prove dispatch picked
    // matroska.
    match result {
      Ok(DispatchOutcome::Claimed(name)) => assert_eq!(name, "matroska"),
      Err(ParseError::Unrecognised) => {
        panic!("matroska reader should have claimed EBML-prefixed input")
      }
      Err(_) | Ok(DispatchOutcome::NoMatch) => {
        // Any other error means the parser claimed the file but the
        // synthetic input was too short — fine for this assertion.
      }
    }
  }
}
