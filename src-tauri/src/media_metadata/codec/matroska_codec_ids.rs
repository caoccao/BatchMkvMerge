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

//! Matroska / WebM CodecID → human-readable codec name.
//!
//! Sourced from the Matroska codec registry
//! (<https://www.matroska.org/technical/codec_specs.html>) and from
//! `mkvtoolnix/src/common/codec.cpp`.
//!
//! Lookup is case-sensitive (Matroska CodecIDs are upper-case ASCII) but
//! prefix matches are supported so `A_AAC/MPEG4/LC/SBR` resolves to `A_AAC`
//! without us enumerating every legacy sub-form.

use super::TrackKind;

/// Look up a Matroska CodecID.  Returns the catalogue entry on match.
pub fn lookup(codec_id: &str) -> Option<MatroskaCodec> {
    // Exact match wins.
    if let Some(entry) = TABLE.iter().copied().find(|e| e.id == codec_id) {
        return Some(entry);
    }
    // Otherwise prefix-match against entries flagged as prefix-able.
    TABLE
        .iter()
        .copied()
        .find(|e| e.prefix && codec_id.starts_with(e.id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatroskaCodec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: TrackKind,
    /// `true` if `lookup` should also resolve longer codec IDs that start
    /// with this `id` (e.g. `A_AAC/MPEG4/LC` matches the bare `A_AAC` entry).
    pub prefix: bool,
}

const TABLE: &[MatroskaCodec] = &[
    // Video
    MatroskaCodec { id: "V_AV1",                       name: "AV1",                                 kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_CINEPAK",                   name: "Cinepak",                             kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_DIRAC",                     name: "Dirac",                               kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_FFV1",                      name: "FFV1",                                kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG1",                     name: "MPEG-1",                              kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG2",                     name: "MPEG-2",                              kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG4/ISO/AP",              name: "MPEG-4p2 advanced profile",           kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG4/ISO/ASP",             name: "MPEG-4p2 advanced simple profile",    kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG4/ISO/AVC",             name: "AVC/H.264/MPEG-4p10",                 kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG4/ISO/SP",              name: "MPEG-4p2 simple profile",             kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEG4/MS/V3",               name: "Microsoft MPEG-4 v3",                 kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MPEGH/ISO/HEVC",            name: "HEVC/H.265/MPEG-H p2",                kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_MS/VFW/FOURCC",             name: "VfW compatibility (FourCC)",          kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_PRORES",                    name: "Apple ProRes",                        kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_QUICKTIME",                 name: "QuickTime video",                     kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_REAL/RV10",                 name: "RealVideo 1",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_REAL/RV20",                 name: "RealVideo 2",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_REAL/RV30",                 name: "RealVideo 3",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_REAL/RV40",                 name: "RealVideo 4",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_SVQ1",                      name: "Sorenson v1",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_SVQ3",                      name: "Sorenson v3",                         kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_THEORA",                    name: "Theora",                              kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_UNCOMPRESSED",              name: "Uncompressed video",                  kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_VP8",                       name: "VP8",                                 kind: TrackKind::Video,    prefix: false },
    MatroskaCodec { id: "V_VP9",                       name: "VP9",                                 kind: TrackKind::Video,    prefix: false },

    // Audio — order matters: more-specific IDs before their prefix-resolving fallbacks.
    MatroskaCodec { id: "A_AAC/MPEG2/LC/SBR",          name: "AAC LC SBR (MPEG-2)",                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG2/LC",              name: "AAC LC (MPEG-2)",                     kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG2/MAIN",            name: "AAC Main (MPEG-2)",                   kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG2/SSR",             name: "AAC SSR (MPEG-2)",                    kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG4/LC/SBR",          name: "AAC LC SBR (MPEG-4)",                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG4/LC",              name: "AAC LC (MPEG-4)",                     kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG4/LTP",             name: "AAC LTP (MPEG-4)",                    kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG4/MAIN",            name: "AAC Main (MPEG-4)",                   kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC/MPEG4/SSR",             name: "AAC SSR (MPEG-4)",                    kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_AAC",                       name: "AAC",                                 kind: TrackKind::Audio,    prefix: true  },
    MatroskaCodec { id: "A_AC3",                       name: "AC-3",                                kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_ALAC",                      name: "Apple Lossless (ALAC)",               kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_DTS/EXPRESS",               name: "DTS Express",                         kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_DTS/LOSSLESS",              name: "DTS-HD Master Audio",                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_DTS",                       name: "DTS",                                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_EAC3",                      name: "E-AC-3",                              kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_FLAC",                      name: "FLAC",                                kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_MLP",                       name: "MLP",                                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_MPEG/L1",                   name: "MP1",                                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_MPEG/L2",                   name: "MP2",                                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_MPEG/L3",                   name: "MP3",                                 kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_MS/ACM",                    name: "ACM compatibility",                   kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_OPUS",                      name: "Opus",                                kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_PCM/FLOAT/IEEE",            name: "PCM (IEEE float)",                    kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_PCM/INT/BIG",               name: "PCM (signed integer, big-endian)",    kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_PCM/INT/LIT",               name: "PCM (signed integer, little-endian)", kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_QUICKTIME/QDM2",            name: "QDesign Music 2",                     kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_QUICKTIME/QDMC",            name: "QDesign Music",                       kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_QUICKTIME",                 name: "QuickTime audio",                     kind: TrackKind::Audio,    prefix: true  },
    MatroskaCodec { id: "A_REAL/14_4",                 name: "RealAudio 14.4",                      kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_REAL/28_8",                 name: "RealAudio 28.8",                      kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_REAL/ATRC",                 name: "Sony ATRAC3",                         kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_REAL/COOK",                 name: "RealAudio Cook",                      kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_REAL/RALF",                 name: "RealAudio Lossless",                  kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_REAL/SIPR",                 name: "RealAudio SIPR",                      kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_TRUEHD",                    name: "TrueHD",                              kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_TTA1",                      name: "TrueAudio",                           kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_VORBIS",                    name: "Vorbis",                              kind: TrackKind::Audio,    prefix: false },
    MatroskaCodec { id: "A_WAVPACK4",                  name: "WavPack4",                            kind: TrackKind::Audio,    prefix: false },

    // Subtitles
    MatroskaCodec { id: "S_DVBSUB",                    name: "DVB subtitles",                       kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_HDMV/PGS",                  name: "HDMV PGS",                            kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_HDMV/TEXTST",               name: "HDMV TextST",                         kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_IMAGE/BMP",                 name: "Bitmap subtitles",                    kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_KATE",                      name: "Kate",                                kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/ASCII",                name: "ASCII text subtitles",                kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/ASS",                  name: "SubStationAlpha (ASS)",               kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/SSA",                  name: "SubStationAlpha (SSA)",               kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/USF",                  name: "Universal Subtitle Format",           kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/UTF8",                 name: "SRT (UTF-8 text)",                    kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TEXT/WEBVTT",               name: "WebVTT",                              kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_TX3G",                      name: "Timed Text (3GPP)",                   kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_VOBSUB",                    name: "VobSub",                              kind: TrackKind::Subtitle, prefix: false },
    MatroskaCodec { id: "S_VOBSUB/ZLIB",               name: "VobSub (zlib-compressed)",            kind: TrackKind::Subtitle, prefix: false },

    // Buttons / metadata
    MatroskaCodec { id: "B_VOBBTN",                    name: "VobButton",                           kind: TrackKind::Button,   prefix: false },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_common_video_ids() {
        let m = lookup("V_MPEG4/ISO/AVC").unwrap();
        assert_eq!(m.name, "AVC/H.264/MPEG-4p10");
        assert_eq!(m.kind, TrackKind::Video);
        let m = lookup("V_AV1").unwrap();
        assert_eq!(m.name, "AV1");
    }

    #[test]
    fn looks_up_common_audio_ids() {
        let m = lookup("A_AC3").unwrap();
        assert_eq!(m.name, "AC-3");
        let m = lookup("A_OPUS").unwrap();
        assert_eq!(m.name, "Opus");
    }

    #[test]
    fn looks_up_subtitle_ids() {
        let m = lookup("S_TEXT/UTF8").unwrap();
        assert_eq!(m.name, "SRT (UTF-8 text)");
        let m = lookup("S_HDMV/PGS").unwrap();
        assert_eq!(m.name, "HDMV PGS");
    }

    #[test]
    fn unknown_codec_id_is_none() {
        assert!(lookup("V_NEVER_SHIPPED").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn aac_prefix_match_resolves() {
        // Exact CodecIDs win over the prefix entry.
        let m = lookup("A_AAC/MPEG4/LC/SBR").unwrap();
        assert_eq!(m.id, "A_AAC/MPEG4/LC/SBR");
        // Bare A_AAC resolves the catch-all entry.
        let m = lookup("A_AAC").unwrap();
        assert_eq!(m.id, "A_AAC");
        // A novel suffix not in the table falls through to the bare A_AAC
        // prefix entry — better to surface "AAC" than nothing at all.
        let m = lookup("A_AAC/CUSTOM_FUTURE").unwrap();
        assert_eq!(m.id, "A_AAC");
        let m = lookup("A_AAC/MPEG4/LC/SBR/future").unwrap();
        assert_eq!(m.id, "A_AAC");
    }

    #[test]
    fn case_sensitive() {
        // Matroska CodecIDs are upper-case ASCII.  We don't lowercase here.
        assert!(lookup("v_mpeg4/iso/avc").is_none());
    }

    #[test]
    fn vobsub_zlib_variant_is_distinct_from_plain_vobsub() {
        // Cross-checked against mkvtoolnix codec.h MKV_S_VOBSUBZLIB.
        let plain = lookup("S_VOBSUB").unwrap();
        let zlib = lookup("S_VOBSUB/ZLIB").unwrap();
        assert_eq!(plain.id, "S_VOBSUB");
        assert_eq!(zlib.id, "S_VOBSUB/ZLIB");
        assert_ne!(plain.name, zlib.name);
    }

    #[test]
    fn newer_video_codec_ids_present() {
        // Cross-checked against mkvtoolnix codec.cpp s_codecs registrations.
        assert_eq!(lookup("V_CINEPAK").unwrap().name, "Cinepak");
        assert_eq!(lookup("V_SVQ1").unwrap().name, "Sorenson v1");
        assert_eq!(lookup("V_SVQ3").unwrap().name, "Sorenson v3");
    }

    #[test]
    fn tx3g_subtitles_recognised() {
        assert_eq!(lookup("S_TX3G").unwrap().name, "Timed Text (3GPP)");
    }

    #[test]
    fn kind_classification_consistent_with_prefix() {
        for entry in TABLE {
            match entry.id.chars().next().unwrap() {
                'V' => assert_eq!(entry.kind, TrackKind::Video, "{}", entry.id),
                'A' => assert_eq!(entry.kind, TrackKind::Audio, "{}", entry.id),
                'S' => assert_eq!(entry.kind, TrackKind::Subtitle, "{}", entry.id),
                'B' => assert_eq!(entry.kind, TrackKind::Button, "{}", entry.id),
                other => panic!("unexpected codec ID prefix '{}': {}", other, entry.id),
            }
        }
    }
}
