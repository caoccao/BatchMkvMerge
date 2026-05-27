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
//! Mirrors mkvtoolnix's `probe_file_format` staged fallthrough: unsupported
//! signatures, unambiguous formats, extension-hinted formats, text subtitles,
//! strict elementary streams, raw-audio phases, ambiguous MPEG containers, and
//! late loose elementary/raw phases. The first reader that claims the file is
//! asked to `read_headers` and the result is returned to the caller.

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

/// Like [`dispatch`], but additionally runs the extension-hinted readers in
/// mkvtoolnix's extension phase, after the unambiguous content probes and
/// before text-subtitle probing (`reader_detection_and_creation.cpp:302-310`).
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
  for reader_name in staged_reader_names(hints) {
    if let Some(outcome) = try_reader_by_name(src, deadline, out, reader_name)? {
      return Ok(outcome);
    }
  }
  Err(ParseError::Unrecognised)
}

fn try_reader_by_name(
  src: &mut FileSource,
  deadline: &Deadline,
  out: &mut MediaMetadata,
  reader_name: &'static str,
) -> Result<Option<DispatchOutcome>, ParseError> {
  let Some(reader) = registered_readers().iter().copied().find(|r| r.name() == reader_name) else {
    return Ok(None);
  };
  // Each probe call must see a freshly-positioned cursor; the trait contract
  // requires probes to rewind on return, but re-seeking here prevents a
  // misbehaving probe from leaking position into the next phase.
  src.seek_to(0)?;
  deadline.check("probe")?;
  if !reader.probe(src)? {
    return Ok(None);
  }
  src.seek_to(0)?;
  reader.read_headers(src, deadline, out)?;
  Ok(Some(DispatchOutcome::Claimed(reader.name())))
}

fn staged_reader_names(hints: &[super::extension_hint::FileTypeHint]) -> Vec<&'static str> {
  let phases: [&[&str]; 8] = [
    UNAMBIGUOUS_READERS,
    TEXT_READERS,
    STRICT_ELEMENTARY_READERS,
    RAW_AUDIO_EIGHT_FRAME_READERS,
    AMBIGUOUS_CONTAINER_READERS,
    LATE_AMBIGUOUS_READERS,
    ONE_FRAME_START_READERS,
    LOOSE_ELEMENTARY_READERS,
  ];
  let mut names = Vec::new();
  names.extend_from_slice(UNAMBIGUOUS_READERS);
  names.extend(hints_to_reader_names(hints));
  for phase in phases.iter().skip(1) {
    names.extend_from_slice(phase);
  }
  names.extend_from_slice(RAW_AUDIO_TWENTY_FRAME_READERS);
  names.extend_from_slice(FINAL_UNSUPPORTED_BUT_LOCAL_READERS);
  names
}

const UNAMBIGUOUS_READERS: &[&str] = &[
  "avi",
  "flv",
  "matroska",
  "wav",
  "ogg",
  "hdmv_textst",
  "flac",
  "pgs",
  "realmedia",
  "mp4",
  "tta",
  "vc1",
  "wavpack",
  "ivf",
  "coreaudio",
  "dirac",
];
const TEXT_READERS: &[&str] = &["webvtt", "srt", "ssa", "vobsub", "usf", "microdvd"];
const STRICT_ELEMENTARY_READERS: &[&str] = &["avc", "hevc"];
const RAW_AUDIO_EIGHT_FRAME_READERS: &[&str] = &["mp3", "ac3", "aac"];
const AMBIGUOUS_CONTAINER_READERS: &[&str] = &["dts", "mpeg_ts", "mpeg_ps", "obu"];
const LATE_AMBIGUOUS_READERS: &[&str] = &["truehd", "dts", "vobbtn"];
const ONE_FRAME_START_READERS: &[&str] = &["mp3", "ac3", "aac"];
const LOOSE_ELEMENTARY_READERS: &[&str] = &["mpeg_video", "avc", "hevc"];
const RAW_AUDIO_TWENTY_FRAME_READERS: &[&str] = &["mp3", "ac3", "aac"];
const FINAL_UNSUPPORTED_BUT_LOCAL_READERS: &[&str] = &["dv"];

/// Translate [`FileTypeHint`] values into the reader names registered in
/// [`registered_readers`].  A hint may resolve to multiple reader names when
/// the extension is genuinely ambiguous (e.g. `mp4` → mp4 + aac, `ogg` → ogg
/// + flac).
fn hints_to_reader_names(hints: &[super::extension_hint::FileTypeHint]) -> Vec<&'static str> {
  use super::extension_hint::FileTypeHint;
  let mut names = Vec::new();
  for hint in hints {
    let resolved: &[&'static str] = match hint {
      FileTypeHint::Aac => &[],
      FileTypeHint::Ac3 => &[],
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
      FileTypeHint::MicroDvd => &[],
      FileTypeHint::Mp3 => &[],
      FileTypeHint::MpegEs => &["mpeg_video"],
      FileTypeHint::MpegPs => &["mpeg_ps"],
      FileTypeHint::MpegTs => &["mpeg_ts"],
      FileTypeHint::Ogm => &["ogg"],
      FileTypeHint::PgsSup => &["pgs"],
      FileTypeHint::QtMp4 => &["mp4"],
      FileTypeHint::Real => &["realmedia"],
      FileTypeHint::Srt => &[],
      FileTypeHint::Ssa => &[],
      FileTypeHint::TrueHd => &["truehd"],
      FileTypeHint::Tta => &["tta"],
      FileTypeHint::Usf => &[],
      FileTypeHint::Vc1 => &["vc1"],
      FileTypeHint::VobButton => &["vobbtn"],
      FileTypeHint::VobSub => &[],
      FileTypeHint::Wav => &["wav"],
      FileTypeHint::Wavpack4 => &["wavpack"],
      FileTypeHint::WebVtt => &[],
      FileTypeHint::HdmvTextSt => &["hdmv_textst"],
      FileTypeHint::Obu => &["obu"],
      FileTypeHint::Alac => &[],
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

  #[test]
  fn extension_hints_do_not_frontload_raw_audio_or_text_readers() {
    use super::super::extension_hint::hints_for_extension;
    assert_eq!(hints_to_reader_names(&hints_for_extension("mp4")), vec!["mp4"]);
    assert!(hints_to_reader_names(&hints_for_extension("mp3")).is_empty());
    assert!(hints_to_reader_names(&hints_for_extension("srt")).is_empty());
  }

  #[test]
  fn staged_cascade_places_unambiguous_before_extension_and_ts_before_ps_without_hint() {
    use super::super::extension_hint::hints_for_extension;
    let hinted = staged_reader_names(&hints_for_extension("mpg"));
    let dirac = hinted.iter().position(|n| *n == "dirac").unwrap();
    let hinted_ps = hinted.iter().position(|n| *n == "mpeg_ps").unwrap();
    assert!(dirac < hinted_ps);

    let unhinted = staged_reader_names(&[]);
    let ts = unhinted.iter().position(|n| *n == "mpeg_ts").unwrap();
    let ps = unhinted.iter().position(|n| *n == "mpeg_ps").unwrap();
    assert!(ts < ps);
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
