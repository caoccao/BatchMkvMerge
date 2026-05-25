# AV1 OBU Parser

Implementation progress: 68%

## Purpose

The AV1 OBU parser recognises raw AV1 Open Bitstream Units streams and reports one video track with dimensions, profile, bit depth, chroma subsampling, and color metadata when available.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/elementary/obu.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_obu.cpp`, `../mkvtoolnix/src/input/r_obu.h`, `../mkvtoolnix/src/common/av1.cpp`, `../mkvtoolnix/src/common/av1.h`

The parser decodes OBU headers, LEB128 sizes, sequence headers, operating profile fields, max frame dimensions, bit depth, monochrome/chroma-subsampling flags, and color description fields. Probing requires a sequence header and a frame-like OBU so isolated headers do not claim arbitrary binary files.

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

Rust scans a smaller prefix than upstream and accepts some no-size OBU forms that upstream rejects. It does not expose timing/default duration, operating-point filtering, AV1C generation, metadata OBU retention, or Dolby Vision RPU/block-addition mapping. The parser handles this by reporting base AV1 metadata only; IVF has separate first-frame Dolby Vision extraction for wrapped AV1.

## Open Issues

### PARSER-220: Raw OBUs without size fields are accepted even though mkvmerge rejects them

Native `walk_obus()` treats an OBU without `obu_has_size_field` as extending to the end of the probe buffer (`src-tauri/src/media_metadata/elementary/obu.rs:371-377`), so a no-size sequence header plus frame-like OBU can pass `probe()`/`read_headers()` (`obu.rs:412-455`). Upstream's AV1 parser returns no size for such OBUs (`../mkvtoolnix/src/common/av1.cpp:131-158`) and `parse_obu()` throws `obu_without_size_unsupported_x` when no size is present (`av1.cpp:414-421`), causing `r_obu.cpp` probing to fail. This can make native identify raw AV1-like data that mkvmerge rejects.
