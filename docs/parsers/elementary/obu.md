# AV1 OBU Parser

Implementation progress: 72%

## Purpose

The AV1 OBU parser recognises raw AV1 Open Bitstream Units streams and reports one video track with dimensions, profile, bit depth, chroma subsampling, and color metadata when available.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/elementary/obu.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_obu.cpp`, `../mkvtoolnix/src/input/r_obu.h`, `../mkvtoolnix/src/common/av1.cpp`, `../mkvtoolnix/src/common/av1.h`

The parser decodes OBU headers, LEB128 sizes, sequence headers, operating profile fields, max frame dimensions, bit depth, monochrome/chroma-subsampling flags, and color description fields. Probing requires a sequence header and a frame-like OBU so isolated headers do not claim arbitrary binary files. The OBU walker also requires every OBU to carry `obu_has_size_field`: an OBU without a size field stops the walk (and rejects the stream), mirroring `parse_obu()`'s `obu_without_size_unsupported_x` throw, so size-less raw OBU data that mkvmerge rejects is not claimed.

## Data Structures

```mermaid
flowchart TD
  A["OBU stream"] --> B["OBU walker"]
  B --> C["SequenceHeader"]
  C --> D["ColorDescription"]
  C --> E["VideoTrackProperties"]
  D --> E
  E --> F["MediaMetadata"]
```

Important structures are `ObuHeader`, `SequenceHeader`, and `ColorDescription`.

## Gaps and Handling

Rust scans a smaller prefix than upstream. It does not expose timing/default duration, operating-point filtering, AV1C generation, metadata OBU retention, or Dolby Vision RPU/block-addition mapping. The parser handles this by reporting base AV1 metadata only; IVF has separate first-frame Dolby Vision extraction for wrapped AV1.
