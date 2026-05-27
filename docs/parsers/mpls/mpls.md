# Blu-ray MPLS Playlist Parser

Implementation progress: 86%

## Purpose

The MPLS parser recognises Blu-ray playlist files, resolves referenced stream clips, applies playlist language metadata, and delegates each segment to the MPEG-TS parser so playlist inputs produce combined media metadata.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/mpls/mod.rs`
- Binary parser: `src-tauri/src/media_metadata/mpls/parser.rs`
- Upstream basis: `../mkvtoolnix/src/common/bluray/mpls.cpp`, `../mkvtoolnix/src/common/bluray/mpls.h`, `../mkvtoolnix/src/common/mm_mpls_multi_file_io.cpp`, MPEG-TS playlist hooks in `../mkvtoolnix/src/input/r_mpeg_ts.cpp`

`parser.rs` validates the MPLS header, version, playlist offsets, play items, sub paths, sub play items, STN stream entries, and chapter marks. `mod.rs` resolves `STREAM/*.m2ts` files, parses available segments through the MPEG-TS reader, merges tracks by PID, records playlist metadata, mirrors the playlist chapter count into the standard `MediaMetadata.chapters` summary, and applies STN languages to matching tracks (PARSER-353).

## Data Structures

```mermaid
flowchart TD
  A[".mpls bytes"] --> B["Playlist"]
  B --> C["PlayItem list"]
  B --> D["SubPath list"]
  B --> E["STN streams"]
  C --> F["STREAM clip paths"]
  F --> G["MPEG-TS parser"]
  G --> H["Merged MediaMetadata"]
  E --> H
```

Key structures are `Playlist`, `PlayItem`, `SubPath`, `SubPlayItem`, `SubPlayItemClip`, and `StnStream`.

## Gaps and Handling

Rust does not use CLPI metadata, does not implement true multi-file packet IO or timestamp continuity, and does not fully surface chapter names or angle/multiclip details. If referenced segment files are missing, only playlist metadata that can be read from the MPLS file is available. Track merging is PID-based and intentionally scoped to metadata listing, but playlists with chapter marks now expose both playlist-specific chapter metadata and the standard chapter summary that mkvtoolnix reports during identification.

## Open Issues

- `PARSER-369` - MPLS playlist opening is gated on the `.mpls` extension hint. mkvtoolnix calls `mm_mpls_multi_file_io_c::open_multi(in)` for every input before the normal probe cascade, and that path validates the MPLS header/content rather than the filename extension. A valid Blu-ray playlist renamed without a `.mpls` extension, with resolvable `STREAM/*.m2ts` clips, is therefore handled by mkvtoolnix but falls through to the normal local dispatcher.
