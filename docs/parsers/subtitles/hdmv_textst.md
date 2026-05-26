# HDMV TextST Parser

Implementation progress: 88%

## Purpose

The HDMV TextST parser recognises Blu-ray text subtitle streams, extracts the first Dialog Style segment as codec-private data, and reports one subtitle track.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/hdmv_textst.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_hdmv_textst.cpp`, `../mkvtoolnix/src/input/r_hdmv_textst.h`, upstream HDMV TextST helpers

The parser validates the `TextST` magic, walks segment headers, finds a Dialog Style segment, and stores it as codec private for the emitted `S_HDMV/TEXTST` track. Mirroring `hdmv_textst_reader_c::identify` (`r_hdmv_textst.cpp`), the track is reported as a subtitle whose payload is the Dialog Style segment; it is **not** flagged `text_subtitles` and carries no `encoding`, because the TextST character coding is part of the Blu-ray data model and is not necessarily UTF-8.

## Data Structures

```mermaid
flowchart TD
  A["TextST bytes"] --> B["segment counter"]
  B --> C["Dialog Style segment"]
  C --> D["CodecPrivate"]
  D --> E["Subtitle track"]
```

The reader is implemented through segment helper functions rather than long-lived parser structs.

## Gaps and Handling

Rust does not model the two-byte frame-count boundary exactly like upstream. The codec-private header path is the important parity point and is implemented.
