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

//! specta-driven TypeScript generation for `media_metadata` types.
//!
//! Drift between Rust and TypeScript is caught at test time.  Set
//! `BMM_REGEN_PROTOCOL_TS=1` to refresh the checked-in file.

use batch_mkvmerge_lib::media_metadata::language::Language;
use batch_mkvmerge_lib::media_metadata::model::{
    AlphaMode, Attachment, AudioCodecConfig, AudioEmphasis, AudioTrackProperties, ChannelLayout,
    ChannelLayoutKind, ChapterSummary, ChromaFormat, ChromaSiting, ChromaSubsampling,
    Chromaticity, CodecInfo, CodecPrivate, ColorMetadata, ColorRange, CommonTrackProperties,
    Container, ContainerFormat, ContainerProperties, CropRect, Dimensions2D, DisplayUnit,
    DurationValue, FieldOrder, HevcTier, InterlaceFlag, MasterMetadata, MediaMetadata,
    PlaylistInfo, Program, ProjectionMetadata, ProjectionPose, ProjectionType, SampleAspectRatio,
    StereoMode,
    SubtitleTrackProperties, TagEntry, TagsBundle, Track, TrackFlag, TrackProperties, TrackType,
    VideoCodecConfig, VideoTrackProperties, Warning, WarningCategory,
};
use specta::Types;
use specta_typescript::Typescript;

const HEADER: &str = "\
// THIS FILE IS GENERATED — DO NOT EDIT BY HAND.
// Regenerate by running:
//     BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript
// from src-tauri/, then commit the updated file.
//
// Source of truth: media_metadata::model in src-tauri/src/media_metadata/model/.

";

fn generate() -> String {
    // Every named type that is part of the on-the-wire shape must be
    // registered so specta emits an `export interface ...` for it.
    let types = Types::default()
        .register::<MediaMetadata>()
        .register::<Container>()
        .register::<ContainerFormat>()
        .register::<ContainerProperties>()
        .register::<Track>()
        .register::<TrackType>()
        .register::<TrackProperties>()
        .register::<CommonTrackProperties>()
        .register::<TrackFlag>()
        .register::<VideoTrackProperties>()
        .register::<AudioTrackProperties>()
        .register::<SubtitleTrackProperties>()
        .register::<CodecInfo>()
        .register::<CodecPrivate>()
        .register::<Dimensions2D>()
        .register::<CropRect>()
        .register::<DisplayUnit>()
        .register::<ColorMetadata>()
        .register::<ColorRange>()
        .register::<ChromaSubsampling>()
        .register::<ChromaSiting>()
        .register::<Chromaticity>()
        .register::<MasterMetadata>()
        .register::<ProjectionMetadata>()
        .register::<ProjectionPose>()
        .register::<ProjectionType>()
        .register::<StereoMode>()
        .register::<AlphaMode>()
        .register::<FieldOrder>()
        .register::<InterlaceFlag>()
        .register::<VideoCodecConfig>()
        .register::<HevcTier>()
        .register::<ChromaFormat>()
        .register::<SampleAspectRatio>()
        .register::<ChannelLayout>()
        .register::<ChannelLayoutKind>()
        .register::<AudioEmphasis>()
        .register::<AudioCodecConfig>()
        .register::<Attachment>()
        .register::<ChapterSummary>()
        .register::<TagEntry>()
        .register::<TagsBundle>()
        .register::<Program>()
        .register::<PlaylistInfo>()
        .register::<DurationValue>()
        .register::<Warning>()
        .register::<WarningCategory>()
        .register::<Language>();

    Typescript::new()
        .header(HEADER)
        .export(&types, specta_serde::Format)
        .expect("specta export failed")
}

fn normalise(s: &str) -> String {
    // Tolerate CRLF/LF differences and trailing whitespace so checked-in
    // file stays comparable across editors and OSes.
    s.replace("\r\n", "\n")
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

fn target_path() -> std::path::PathBuf {
    // Tests run with the manifest dir as cwd, so the frontend's src/ is one
    // level up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("src")
        .join("protocol.generated.ts")
}

#[test]
fn protocol_generated_ts_is_up_to_date() {
    let generated = generate();
    let path = target_path();

    if std::env::var("BMM_REGEN_PROTOCOL_TS").is_ok() {
        std::fs::write(&path, &generated).expect("write protocol.generated.ts");
        eprintln!("regenerated {}", path.display());
        return;
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "{} not found ({err}). \
             First run: BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript",
            path.display()
        )
    });

    if normalise(&existing) != normalise(&generated) {
        let diff_preview = diff_preview(&existing, &generated);
        panic!(
            "protocol.generated.ts is stale.\n{diff_preview}\n\n\
             Refresh with: BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript"
        );
    }
}

#[test]
fn generated_typescript_is_non_empty_and_exports_root() {
    let generated = generate();
    assert!(generated.contains("export type MediaMetadata"));
    assert!(generated.contains("export type Track"));
    assert!(generated.contains("export type Container"));
}

#[test]
fn generated_typescript_preserves_nested_track_properties() {
    let generated = generate();
    // The whole point of this protocol: video / audio / subtitle live as
    // nested sub-objects on TrackProperties, never flattened.
    assert!(generated.contains("export type TrackProperties"));
    assert!(generated.contains("export type VideoTrackProperties"));
    assert!(generated.contains("export type AudioTrackProperties"));
    assert!(generated.contains("export type SubtitleTrackProperties"));
}

fn diff_preview(expected: &str, actual: &str) -> String {
    let exp_lines: Vec<&str> = expected.lines().collect();
    let act_lines: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    let max_lines = exp_lines.len().max(act_lines.len());
    let mut shown = 0;
    for i in 0..max_lines {
        let a = exp_lines.get(i).copied().unwrap_or("");
        let b = act_lines.get(i).copied().unwrap_or("");
        if a != b {
            out.push_str(&format!("line {:>4}: -{}\n", i + 1, a));
            out.push_str(&format!("line {:>4}: +{}\n", i + 1, b));
            shown += 1;
            if shown >= 20 {
                out.push_str("... (further differences truncated)\n");
                break;
            }
        }
    }
    if out.is_empty() {
        out.push_str("(diff was found by normalised comparison only — whitespace/newlines)");
    }
    out
}
