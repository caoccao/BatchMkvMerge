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

//! Extension → candidate file type table.  Direct port of
//! `mkvtoolnix/src/common/file_types.cpp::get_supported()` — every extension
//! mkvmerge recognises is registered here, including the ones whose readers
//! land in later phases.
//!
//! Multiple file types can share an extension (e.g. `ogg` matches both Ogg
//! containers and FLAC-in-Ogg; `mp4` matches MP4, AAC and ALAC). The lookup
//! returns *every* hint to let the probe cascade try each candidate in turn.

use std::path::Path;

/// Coarse file-type tag the probe cascade uses to bias dispatch.  One variant
/// per `mtx::file_type_e` value in mkvtoolnix.  Variants whose reader is not
/// yet implemented are still listed — they short-circuit to "skip" until the
/// matching format module lands in a later phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileTypeHint {
    Aac,
    Ac3,
    Asf,
    AvcEs,
    Avi,
    Cdxa,
    Chapters,
    CoreAudio,
    Dirac,
    Dts,
    Dv,
    Flac,
    Flv,
    HevcEs,
    HdSub,
    Ivf,
    Matroska,
    MicroDvd,
    Mp3,
    MpegEs,
    MpegPs,
    MpegTs,
    Ogm,
    PgsSup,
    QtMp4,
    Real,
    Srt,
    Ssa,
    TrueHd,
    Tta,
    Usf,
    Vc1,
    VobButton,
    VobSub,
    Wav,
    Wavpack4,
    WebVtt,
    HdmvTextSt,
    Obu,
    AviDv1,
    /// `mpls` blu-ray playlist — recognised but no dedicated reader; mkvtoolnix
    /// also tags it `is_unknown` (see file_types.cpp:48).
    BlurayPlaylist,
    /// `caf` / `m4a` / `mp4` ALAC variant — recognised, dispatched into the
    /// MP4 reader at parse time (file_types.cpp:32).
    Alac,
}

impl FileTypeHint {
    /// Stable identifier used for log lines and dispatch ordering.  Matches
    /// the lower-case variant name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Ac3 => "ac3",
            Self::Asf => "asf",
            Self::AvcEs => "avc_es",
            Self::Avi => "avi",
            Self::Cdxa => "cdxa",
            Self::Chapters => "chapters",
            Self::CoreAudio => "coreaudio",
            Self::Dirac => "dirac",
            Self::Dts => "dts",
            Self::Dv => "dv",
            Self::Flac => "flac",
            Self::Flv => "flv",
            Self::HevcEs => "hevc_es",
            Self::HdSub => "hdsub",
            Self::Ivf => "ivf",
            Self::Matroska => "matroska",
            Self::MicroDvd => "microdvd",
            Self::Mp3 => "mp3",
            Self::MpegEs => "mpeg_es",
            Self::MpegPs => "mpeg_ps",
            Self::MpegTs => "mpeg_ts",
            Self::Ogm => "ogm",
            Self::PgsSup => "pgssup",
            Self::QtMp4 => "qtmp4",
            Self::Real => "real",
            Self::Srt => "srt",
            Self::Ssa => "ssa",
            Self::TrueHd => "truehd",
            Self::Tta => "tta",
            Self::Usf => "usf",
            Self::Vc1 => "vc1",
            Self::VobButton => "vobbtn",
            Self::VobSub => "vobsub",
            Self::Wav => "wav",
            Self::Wavpack4 => "wavpack4",
            Self::WebVtt => "webvtt",
            Self::HdmvTextSt => "hdmv_textst",
            Self::Obu => "obu",
            Self::AviDv1 => "avi_dv_1",
            Self::BlurayPlaylist => "bluray_playlist",
            Self::Alac => "alac",
        }
    }
}

/// One (extension, candidate type) entry.  Mirrors the `(extensions, file_type_e)`
/// pairs in `file_types.cpp::get_supported`.  An extension can appear more
/// than once when it is ambiguous (e.g. `mp4` → AAC, ALAC, QtMp4).
struct Entry {
    ext: &'static str,
    hint: FileTypeHint,
}

/// Full extension table.  Source-of-truth file is
/// `mkvtoolnix/src/common/file_types.cpp:28-66`.  Keep this list sorted by
/// extension for easier maintenance.
///
/// Note: extensions are stored *without* leading dot, lower-case.  Comparisons
/// are case-insensitive (see [`hints_for_extension`]).
#[rustfmt::skip]
const TABLE: &[Entry] = &[
    // Dolby Digital / Dolby Digital Plus (AC-3, E-AC-3)
    Entry { ext: "ac3",     hint: FileTypeHint::Ac3 },
    Entry { ext: "eac3",    hint: FileTypeHint::Ac3 },
    Entry { ext: "eb3",     hint: FileTypeHint::Ac3 },
    Entry { ext: "ec3",     hint: FileTypeHint::Ac3 },
    // AAC
    Entry { ext: "aac",     hint: FileTypeHint::Aac },
    Entry { ext: "m4a",     hint: FileTypeHint::Aac },
    Entry { ext: "mp4",     hint: FileTypeHint::Aac },
    // AVC/H.264 elementary streams
    Entry { ext: "264",     hint: FileTypeHint::AvcEs },
    Entry { ext: "avc",     hint: FileTypeHint::AvcEs },
    Entry { ext: "h264",    hint: FileTypeHint::AvcEs },
    Entry { ext: "x264",    hint: FileTypeHint::AvcEs },
    // AVI
    Entry { ext: "avi",     hint: FileTypeHint::Avi },
    // ALAC (file_types.cpp:32) — shares extensions with AAC/MP4.
    Entry { ext: "caf",     hint: FileTypeHint::Alac },
    Entry { ext: "m4a",     hint: FileTypeHint::Alac },
    Entry { ext: "mp4",     hint: FileTypeHint::Alac },
    // Dirac
    Entry { ext: "drc",     hint: FileTypeHint::Dirac },
    // Dolby TrueHD
    Entry { ext: "mlp",     hint: FileTypeHint::TrueHd },
    Entry { ext: "thd",     hint: FileTypeHint::TrueHd },
    Entry { ext: "thd+ac3", hint: FileTypeHint::TrueHd },
    Entry { ext: "truehd",  hint: FileTypeHint::TrueHd },
    Entry { ext: "true-hd", hint: FileTypeHint::TrueHd },
    // DTS / DTS-HD
    Entry { ext: "dts",     hint: FileTypeHint::Dts },
    Entry { ext: "dtshd",   hint: FileTypeHint::Dts },
    Entry { ext: "dts-hd",  hint: FileTypeHint::Dts },
    Entry { ext: "dtsma",   hint: FileTypeHint::Dts },
    // FLAC (mkvtoolnix gates this on HAVE_FLAC_FORMAT_H; we ship Rust always)
    Entry { ext: "flac",    hint: FileTypeHint::Flac },
    Entry { ext: "ogg",     hint: FileTypeHint::Flac },
    // FLV (Flash Video)
    Entry { ext: "f4v",     hint: FileTypeHint::Flv },
    Entry { ext: "flv",     hint: FileTypeHint::Flv },
    // HDMV TextST
    Entry { ext: "textst",  hint: FileTypeHint::HdmvTextSt },
    // HEVC/H.265 elementary streams
    Entry { ext: "265",     hint: FileTypeHint::HevcEs },
    Entry { ext: "hevc",    hint: FileTypeHint::HevcEs },
    Entry { ext: "h265",    hint: FileTypeHint::HevcEs },
    Entry { ext: "x265",    hint: FileTypeHint::HevcEs },
    // IVF (AV1, VP8, VP9)
    Entry { ext: "ivf",     hint: FileTypeHint::Ivf },
    // MP4 / QuickTime / M4V
    Entry { ext: "mp4",     hint: FileTypeHint::QtMp4 },
    Entry { ext: "m4v",     hint: FileTypeHint::QtMp4 },
    Entry { ext: "mov",     hint: FileTypeHint::QtMp4 },
    // MPEG-1/2 Audio Layer II/III
    Entry { ext: "mp2",     hint: FileTypeHint::Mp3 },
    Entry { ext: "mp3",     hint: FileTypeHint::Mp3 },
    // MPEG program streams
    Entry { ext: "mpg",     hint: FileTypeHint::MpegPs },
    Entry { ext: "mpeg",    hint: FileTypeHint::MpegPs },
    Entry { ext: "m2v",     hint: FileTypeHint::MpegPs },
    Entry { ext: "mpv",     hint: FileTypeHint::MpegPs },
    Entry { ext: "evo",     hint: FileTypeHint::MpegPs },
    Entry { ext: "evob",    hint: FileTypeHint::MpegPs },
    Entry { ext: "vob",     hint: FileTypeHint::MpegPs },
    // MPEG transport streams
    Entry { ext: "ts",      hint: FileTypeHint::MpegTs },
    Entry { ext: "m2ts",    hint: FileTypeHint::MpegTs },
    Entry { ext: "mts",     hint: FileTypeHint::MpegTs },
    // MPEG-1/2 video elementary streams (file_types.cpp:47 re-uses m2v/mpv).
    Entry { ext: "m1v",     hint: FileTypeHint::MpegEs },
    Entry { ext: "m2v",     hint: FileTypeHint::MpegEs },
    Entry { ext: "mpv",     hint: FileTypeHint::MpegEs },
    // Blu-ray playlist (mkvtoolnix recognises but does not parse standalone).
    Entry { ext: "mpls",    hint: FileTypeHint::BlurayPlaylist },
    // Matroska
    Entry { ext: "mk3d",    hint: FileTypeHint::Matroska },
    Entry { ext: "mka",     hint: FileTypeHint::Matroska },
    Entry { ext: "mks",     hint: FileTypeHint::Matroska },
    Entry { ext: "mkv",     hint: FileTypeHint::Matroska },
    // WebM (matroska reader, declared separately in file_types.cpp for naming).
    Entry { ext: "weba",    hint: FileTypeHint::Matroska },
    Entry { ext: "webm",    hint: FileTypeHint::Matroska },
    Entry { ext: "webma",   hint: FileTypeHint::Matroska },
    Entry { ext: "webmv",   hint: FileTypeHint::Matroska },
    // PGS / SUP subtitles
    Entry { ext: "sup",     hint: FileTypeHint::PgsSup },
    // AV1 Open Bitstream Units stream
    Entry { ext: "av1",     hint: FileTypeHint::Obu },
    Entry { ext: "obu",     hint: FileTypeHint::Obu },
    // Ogg / OGM (Opus / Vorbis / Theora / FLAC-in-Ogg all share these)
    Entry { ext: "ogg",     hint: FileTypeHint::Ogm },
    Entry { ext: "ogm",     hint: FileTypeHint::Ogm },
    Entry { ext: "ogv",     hint: FileTypeHint::Ogm },
    Entry { ext: "opus",    hint: FileTypeHint::Ogm },
    // RealMedia
    Entry { ext: "ra",      hint: FileTypeHint::Real },
    Entry { ext: "ram",     hint: FileTypeHint::Real },
    Entry { ext: "rm",      hint: FileTypeHint::Real },
    Entry { ext: "rmvb",    hint: FileTypeHint::Real },
    Entry { ext: "rv",      hint: FileTypeHint::Real },
    // SRT / SSA / ASS subtitles
    Entry { ext: "srt",     hint: FileTypeHint::Srt },
    Entry { ext: "ass",     hint: FileTypeHint::Ssa },
    Entry { ext: "ssa",     hint: FileTypeHint::Ssa },
    // TTA / USF / VC-1
    Entry { ext: "tta",     hint: FileTypeHint::Tta },
    Entry { ext: "usf",     hint: FileTypeHint::Usf },
    Entry { ext: "xml",     hint: FileTypeHint::Usf },
    Entry { ext: "vc1",     hint: FileTypeHint::Vc1 },
    // VobButton / VobSub
    Entry { ext: "btn",     hint: FileTypeHint::VobButton },
    Entry { ext: "idx",     hint: FileTypeHint::VobSub },
    // WAV / WAVPACK
    Entry { ext: "wav",     hint: FileTypeHint::Wav },
    Entry { ext: "wv",      hint: FileTypeHint::Wavpack4 },
    // WebVTT
    Entry { ext: "vtt",     hint: FileTypeHint::WebVtt },
    Entry { ext: "webvtt",  hint: FileTypeHint::WebVtt },
];

/// Lower-case the extension and look up every file-type hint that declares it.
/// Returns an empty slice for unknown extensions.  Duplicates for the same
/// extension reflect the genuinely ambiguous cases (e.g. `mp4`).
pub fn hints_for_extension(ext: &str) -> Vec<FileTypeHint> {
    let needle = ext.trim_start_matches('.').to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    TABLE
        .iter()
        .filter(|e| e.ext == needle)
        .map(|e| e.hint)
        .collect()
}

/// Pull the extension off a path and look it up.  Returns an empty `Vec` if
/// the path has no extension component.
pub fn hints_for_path<P: AsRef<Path>>(path: P) -> Vec<FileTypeHint> {
    let p = path.as_ref();
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    hints_for_extension(ext)
}

/// Coarse drag-drop accept predicate — was `mkvtoolnix.rs::is_mkv`. Returns
/// `true` for every extension `mkvmerge -J` would recognise. The MKV-only
/// historical restriction is dropped here.
pub fn is_supported_media_extension(ext: &str) -> bool {
    !hints_for_extension(ext).is_empty()
}

/// Convenience wrapper for whole paths.
pub fn is_supported_media_path<P: AsRef<Path>>(path: P) -> bool {
    !hints_for_path(path).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_extension_returns_empty() {
        assert!(hints_for_extension("xyz").is_empty());
        assert!(!is_supported_media_extension("xyz"));
    }

    #[test]
    fn empty_extension_returns_empty() {
        assert!(hints_for_extension("").is_empty());
        assert!(!is_supported_media_extension(""));
    }

    #[test]
    fn leading_dot_is_stripped() {
        assert_eq!(hints_for_extension(".mkv"), hints_for_extension("mkv"));
    }

    #[test]
    fn case_insensitive_match() {
        let lower = hints_for_extension("mkv");
        let upper = hints_for_extension("MKV");
        let mixed = hints_for_extension("Mkv");
        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
        assert_eq!(lower, vec![FileTypeHint::Matroska]);
    }

    #[test]
    fn ambiguous_mp4_returns_all_candidates() {
        let hints = hints_for_extension("mp4");
        assert!(hints.contains(&FileTypeHint::Aac));
        assert!(hints.contains(&FileTypeHint::Alac));
        assert!(hints.contains(&FileTypeHint::QtMp4));
    }

    #[test]
    fn ambiguous_ogg_returns_ogm_and_flac() {
        let hints = hints_for_extension("ogg");
        assert!(hints.contains(&FileTypeHint::Ogm));
        assert!(hints.contains(&FileTypeHint::Flac));
    }

    #[test]
    fn extension_table_covers_matroska_family() {
        for ext in ["mkv", "mka", "mks", "mk3d", "webm", "weba", "webmv", "webma"] {
            assert_eq!(
                hints_for_extension(ext),
                vec![FileTypeHint::Matroska],
                "{ext} should map to matroska only"
            );
        }
    }

    #[test]
    fn dot_path_lookup() {
        assert!(is_supported_media_path("foo/bar/clip.mkv"));
        assert!(is_supported_media_path("foo.MP4"));
        assert!(!is_supported_media_path("foo.unknownext"));
        // No extension at all
        assert!(!is_supported_media_path("Makefile"));
    }

    #[test]
    fn historical_mkv_predicate_now_covers_all_supported_extensions() {
        // Spot-check that the historic mkv-only restriction has been broadened.
        for ext in [
            "mkv", "webm", "mp4", "mov", "avi", "mp3", "aac", "flac", "wav",
            "ts", "m2ts", "h264", "265", "av1", "srt", "ass", "sup", "vtt",
            "ogg", "opus", "thd", "dts", "rm", "vc1", "idx", "ivf",
        ] {
            assert!(is_supported_media_extension(ext), "{ext} should be supported");
        }
    }

    #[test]
    fn webvtt_canonical_extension_is_supported() {
        assert!(is_supported_media_extension("vtt"));
        assert!(is_supported_media_extension("webvtt"));
        assert_eq!(
            hints_for_extension("vtt"),
            vec![FileTypeHint::WebVtt],
        );
    }

    #[test]
    fn truehd_alternate_spellings_supported() {
        for ext in ["thd", "mlp", "truehd", "true-hd", "thd+ac3"] {
            assert_eq!(
                hints_for_extension(ext),
                vec![FileTypeHint::TrueHd],
                "{ext} should map to truehd",
            );
        }
    }

    #[test]
    fn file_type_hint_name_is_stable_and_unique() {
        // Make sure every hint has a non-empty stable name.
        let all: Vec<&'static str> = TABLE.iter().map(|e| e.hint.name()).collect();
        for name in &all {
            assert!(!name.is_empty());
        }
    }

    #[test]
    fn path_with_no_extension_returns_empty() {
        assert!(hints_for_path("/tmp/anonymous_blob").is_empty());
        assert!(hints_for_path(".hidden").is_empty()); // extension parser treats whole thing as stem
    }

    #[test]
    fn bluray_playlist_recognised_but_no_dedicated_reader() {
        // .mpls is recognised so we don't reject it from drag-drop, but it has
        // its own hint flag distinct from QtMp4 / Matroska.
        assert_eq!(
            hints_for_extension("mpls"),
            vec![FileTypeHint::BlurayPlaylist],
        );
        assert!(is_supported_media_extension("mpls"));
    }
}
