# CoreAudio CAF Parser

Implementation progress: 95%

## Purpose

The CoreAudio parser recognises CAF files and reports audio metadata, with full supported-track handling for ALAC. Non-ALAC CAF files are recognised but exposed as unsupported, matching mkvtoolnix's container-level identification behavior.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/coreaudio/reader.rs`
- CAF helpers: `src-tauri/src/media_metadata/coreaudio/caf.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_coreaudio.cpp`, `../mkvtoolnix/src/input/r_coreaudio.h`

The reader checks the `caff` magic case-insensitively, scans CAF chunks while preserving each declared chunk size, requires `desc`, `pakt`, and `data`, uses `pakt` for duration, and converts `kuki` ALAC magic cookies into the codec-private form used by Matroska-oriented metadata. Required chunk bodies are read exactly: zero-sized chunks, chunks over the bounded read cap, and chunks whose declared body extends past EOF fail header parsing instead of being repaired. When a present ALAC `kuki` chunk is too short or carries a truncated old-style `frmaalac` wrapper, header parsing fails as malformed instead of silently dropping codec private data. `caf.rs` contains the chunk-level structures and ALAC cookie conversion.

## Data Structures

```mermaid
flowchart TD
  A["CAF chunks"] --> B["desc"]
  A --> C["pakt"]
  A --> D["kuki"]
  B --> E["AudioDescription"]
  D --> F["AlacConfig"]
  E --> G["AudioTrackProperties"]
  F --> H["CodecPrivate"]
  G --> I["MediaMetadata"]
  H --> I
```

Key structures are `Chunk`, `AudioDescription`, `CafMetadata`, and `AlacConfig`.

## Gaps and Handling

Packet tables are used for header-derived duration and validation but are not retained for packet delivery. Codec naming follows the app model rather than mkvmerge's exact codec lookup display strings.
