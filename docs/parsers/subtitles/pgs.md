# PGS SUP Parser

Implementation progress: 90%

## Purpose

The PGS parser recognises HDMV Presentation Graphics `.sup` files and reports one graphical subtitle track with `S_HDMV/PGS` codec identity.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/pgs.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_hdmv_pgs.cpp`, `../mkvtoolnix/src/input/r_hdmv_pgs.h`, upstream HDMV PGS helpers

The parser validates the `PG` segment chain, checks segment sizes and known segment types, counts plausible segments, and emits a single image subtitle track.

## Data Structures

```mermaid
flowchart TD
  A["SUP bytes"] --> B["PG segment counter"]
  B --> C["segment validation"]
  C --> D["S_HDMV/PGS track"]
```

The reader uses helper functions rather than custom persistent structs.

## Gaps and Handling

The Rust probe is stricter than upstream and currently omits a valid upstream interactive-composition segment type. This can create false negatives for edge files, but accepted files have complete header-level metadata for this app.

## Open Issues

### PARSER-219: Valid PGS segment chains can be rejected by stricter type checks

Native `count_segments()` requires every early segment type to be one of `0x14`, `0x15`, `0x16`, `0x17`, or `0x80` (`src-tauri/src/media_metadata/subtitles/pgs.rs:44-67`) before accepting the file (`pgs.rs:78-95`). Upstream probe only verifies that the first segment starts with `PG`, skips by the declared segment length, and checks that the next segment also starts with `PG` (`../mkvtoolnix/src/input/r_hdmv_pgs.cpp:26-37`). Upstream also defines interactive-composition segment type `0x18` (`../mkvtoolnix/src/common/hdmv_pgs.h:22-27`). SUP files containing interactive-composition or future segment types near the beginning can be false negatives natively.
