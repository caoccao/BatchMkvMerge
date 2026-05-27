# WavPack Parser

Implementation progress: 100%

## Purpose

The WavPack parser recognises WavPack v4 streams and reports sample rate, channel count, bit depth, total-sample duration, and codec-private version metadata.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/wavpack.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_wavpack.cpp`, `../mkvtoolnix/src/input/r_wavpack.h`, `../mkvtoolnix/src/common/wavpack.cpp`, `../mkvtoolnix/src/common/wavpack.h`

The reader parses the `wvpk` frame header, validates v4 frame fields, gathers multichannel segment information, reads standard and nonstandard sample-rate metadata, applies DSD rate shifters, and derives duration when total samples are known. Block boundaries are located with `read_next_header` (a port of `../mkvtoolnix/src/common/wavpack.cpp:54-90`): rather than trusting the exact `ck_size + 8` offset, it scans forward for the next valid `wvpk` header, skipping padding / junk bytes and only giving up after more than 1 MiB has been skipped (or EOF). The first-frame walk continues until the WavPack `FINAL_BLOCK` flag, EOF/error, or the parser deadline, so large multichannel segments are accumulated like mkvtoolnix instead of stopping after a fixed block count.

## Data Structures

```mermaid
flowchart TD
  A["wvpk frame"] --> B["WavpackHeader"]
  B --> C["WavpackMeta"]
  C --> D["sample rate and channel count"]
  C --> E["bits per sample and duration"]
  D --> F["Audio track"]
  E --> F
```

Core structures are `WavpackHeader` and `WavpackMeta`.

## Gaps and Handling

Upstream can pair correction `.wvc` files for muxing. The Rust parser focuses on the primary `.wv` metadata path and does not retain correction-stream state, which is not surfaced in the UI.
