# VobSub IDX Parser

Implementation progress: 90%

## Purpose

The VobSub parser recognises `.idx` manifests, records the sibling `.sub` file when present, and reports one image subtitle track per language entry.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/vobsub.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_vobsub.cpp`, `../mkvtoolnix/src/input/r_vobsub.h`, `../mkvtoolnix/src/common/vobsub.cpp`, `../mkvtoolnix/src/common/vobsub.h`

The parser is resolved by *path* before the content cascade: both `.idx` and `.sub` inputs map to the canonical `.idx` (mirroring mkvtoolnix's `idx_and_sub_file_names`), so dragging a `.sub` produces the same listing as its `.idx`. It checks the VobSub index-file banner, parses the manifest into per-`id:` track entry lists, resolves the sibling `.sub` data file, records it under `container.properties.other_files`, and emits one `S_VOBSUB` track per non-empty entry list. The `.sub` MPEG-PS payload is never demuxed — only located and recorded.

## Data Structures

```mermaid
flowchart TD
  P["input path (.idx / .sub)"] --> R["resolve_idx_path → .idx"]
  R --> B["looks_like_vobsub_idx (banner)"]
  R --> C["parse_idx"]
  C --> D["VobSubTrack + entries"]
  C --> CP["filtered codec_private"]
  D --> E["Subtitle tracks"]
  CP --> E
  R --> F["sibling_sub_path"]
  F --> G["otherFiles"]
  E --> H["MediaMetadata"]
  G --> H
```

`parse_idx` returns a list of `VobSubTrack` (language + `VobSubEntry` list of `{position, timestamp}`) plus the shared `codec_private` text. `parse_idx` is a direct port of `vobsub_reader_c::parse_headers` (`r_vobsub.cpp:193-352`): it accumulates `delay:` per track, parses `timestamp: HH:MM:SS:mmm, filepos: 0xNN` entries (the third colon is the millisecond separator, matching `parse_timestamp` in `parsing.cpp:154-155`), applies negative-delay clamp-forward correction, skips entries that stay negative, flags out-of-order tracks for a stable sort by timestamp, and drops tracks that end up with zero entries.

## Path resolution and dispatch

VobSub is intercepted by path in `media_metadata::parse_with_extension_fallback` *before* the content cascade. `is_vobsub_candidate_path` matches `.idx` and `.sub` extensions; `subtitles::vobsub::try_open_by_path` then resolves the `.idx` (`resolve_idx_path`), and only claims the file when that `.idx` exists and carries the banner. A `.sub` with no banner-bearing sibling `.idx` (e.g. a MicroDVD `.sub`) declines and the normal cascade runs, so no other reader's inputs are stolen. `VobSubReader` remains in the registry for content-based `.idx` probing as a fallback. The `.sub` data file is located and recorded under `container.properties.other_files` but never demuxed.

## Codec private

Codec private is built from the filtered `idx_data`: the per-track control lines `id:`, `timestamp:`, `delay:`, `alt:` and `langidx:` are removed, and `#` comment / blank lines are skipped, leaving the global settings lines (`size:`, `palette:`, ...) shared across every track — matching mkvtoolnix's `idx_data`.

## Gaps and Handling

Header-only: the `.sub` MPEG-PS payload is never demuxed, so per-entry SPU durations and `spu_size`/`overhead` accounting from `extract_one_spu_packet` are not computed. The `.idx` manifest is decoded up to a bounded 64 KiB prefix (these manifests are tiny in practice). These are intentional header-only departures and do not affect the track listing, languages, entry counts, or codec-private parity.
