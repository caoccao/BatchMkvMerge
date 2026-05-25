# USF Parser

Implementation progress: 45%

## Purpose

The USF parser recognises Universal Subtitle Format XML files and reports one or more text subtitle tracks with basic language and name metadata.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/usf.rs`
- Encoding helper: `src-tauri/src/media_metadata/subtitles/encoding.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_usf.cpp`, `../mkvtoolnix/src/input/r_usf.h`

The reader performs lightweight text probing for a `<USFSubtitles` root, scans `<subtitles>` elements, extracts simple inline attributes, and emits one `S_TEXT/USF` track per discovered subtitle element.

## Data Structures

```mermaid
flowchart TD
  A["USF XML text"] --> B["root probe"]
  A --> C["parse_usf_tracks"]
  C --> D["UsfTrackInfo"]
  D --> E["Subtitle tracks"]
  E --> F["MediaMetadata"]
```

`UsfTrackInfo` holds the local track name and language found during the lightweight scan.

## Gaps and Handling

Unlike upstream, Rust does not use a full XML parser or validate the full document. It misses default language metadata from `<metadata>`, child language elements, and the upstream codec-private shape, which should be the full XML minus subtitle payloads. This parser is therefore a skeleton that identifies USF files and track count but does not yet match mkvmerge's richer USF metadata behavior.
