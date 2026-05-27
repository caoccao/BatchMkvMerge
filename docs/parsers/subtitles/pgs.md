# PGS SUP Parser

Implementation progress: 93%

## Purpose

The PGS parser recognises HDMV Presentation Graphics `.sup` files and reports one graphical subtitle track with `S_HDMV/PGS` codec identity.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/pgs.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_hdmv_pgs.cpp`, `../mkvtoolnix/src/input/r_hdmv_pgs.h`, upstream HDMV PGS helpers

The parser validates the `PG` segment chain exactly as `hdmv_pgs_reader_c::probe_file` does — it checks the `PG` magic, skips each segment by its declared `segment_length`, and confirms the next segment also carries the `PG` magic. It does **not** gate on `segment_type`, so interactive-composition (`0x18`) and future segment types near the start are walked rather than rejected. Once at least two `PG`-magic segments are observed it emits a single image subtitle track.

## Data Structures

```mermaid
flowchart TD
  A["SUP bytes"] --> B["PG segment counter"]
  B --> C["segment validation"]
  C --> D["S_HDMV/PGS track"]
```

The reader uses helper functions rather than custom persistent structs.

## Gaps and Handling

The probe now matches upstream's `PG`-magic-and-length walk and no longer gates on segment type, so files carrying interactive-composition or future segment types near the start are recognised. The reader remains header-only: it counts segment headers to confirm the format but does not decode palette, object, or composition payloads.

## Open Issues

### PARSER-328: A max-size first PGS segment can exceed the 64 KiB probe window

`pgs.rs` reads only `PROBE_BYTES = 64 * 1024` for both probing and `read_headers`, then requires `count_segments` to see at least two complete `PG` segment headers inside that buffer. Upstream `hdmv_pgs_reader_c::probe_file` reads the first magic, skips the declared 16-bit segment length with a seek, and then reads the next magic. It does not require the second segment header to be inside a fixed 64 KiB memory window.

A legal PGS segment length can be 65535 bytes. With the 13-byte PGS segment header, the second `PG` magic can start at byte 65548, just beyond the Rust parser's 64 KiB buffer, so the native parser returns `Unrecognised` while mkvtoolnix accepts the file. The probe should read the first segment header, seek or skip by the declared segment length, and then read the next magic directly.
