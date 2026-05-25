# MP4 / QuickTime Parser

Implementation progress: 85%

## Purpose

The MP4 parser recognises ISO BMFF, MP4, M4V, MOV, and QuickTime-style files. It extracts movie metadata, tracks, sample-entry codec data, iTunes metadata, fragments, and bounded first-sample verification.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/mp4/reader.rs`
- Related modules: `src-tauri/src/media_metadata/mp4/atom.rs`, `ftyp.rs`, `moov/`, `codec_specific/`, `meta/`, `fragments.rs`, `identify.rs`, `verify.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_qtmp4.cpp`, `../mkvtoolnix/src/input/r_qtmp4.h`, upstream helpers under `../mkvtoolnix/src/common`

The parser scans top-level boxes, handles normal and zlib-compressed `moov` boxes, parses `ftyp`, `mvhd`, `trak`, `tkhd`, `mdia`, `mdhd`, `hdlr`, `stbl`, `stsd`, `stts`, `stsc`, `edts/elst`, `mvex/trex`, `moof/traf/tfhd/trun`, and `udta/meta/ilst`. Codec-specific parsers cover AVC, HEVC, AV1, AAC, ALAC, Opus, FLAC, color, pixel aspect ratio, and Dolby Vision block-addition records.

## Data Structures

```mermaid
flowchart TD
  A["MP4 boxes"] --> B["BoxHeader walker"]
  B --> C["FileType"]
  B --> D["MoovBuilder"]
  D --> E["TrackBuilder"]
  E --> F["codec_specific parsers"]
  D --> G["Fragment summaries"]
  E --> H["Track"]
  C --> I["ContainerProperties"]
```

Key structures are `BoxHeader`, `FileType`, `MoovBuilder`, `TrackBuilder`, `TrexDefaults`, `MoofSummary`, and codec-specific config records.

## Gaps and Handling

Upstream has complete sample-table muxing, interleaving, chapter-track and `tref` behavior, and a wider QuickTime metadata surface. Rust implements enough sample-table handling for first-sample verification but not packet output. Rare atoms and codec branches are intentionally narrower; unknown private data is preserved where useful rather than interpreted unsafely.

## Open Issues

- **PARSER-198: `stsd` sample descriptions after entry 0 are skipped.** Native parsing only calls `parse_first_entry` for the first sample-description entry and skips the rest. mkvtoolnix iterates through every `stsd` entry and lets the demuxer parse each one. Tracks with multiple sample descriptions can lose later codec private data, dimensions, audio properties, or validation behavior.
- **PARSER-199: MP4 Opus `dOps` private data uses the wrong layout.** Native stores the raw `dOps` payload as codec private data. mkvtoolnix builds the Matroska/Ogg Opus ID header by prepending `OpusHead` and converting pre-skip, input sample rate, and output gain fields from MP4 big-endian to little-endian.
- **PARSER-200: MP4 FLAC `dfLa` codec private data includes the FullBox header.** Native stores the whole `dfLa` payload, including the four-byte version/flags header. mkvtoolnix skips those four bytes and stores only the FLAC metadata block chain.
- **PARSER-201: MP4 ALAC codec private data includes the FullBox header.** Native stores the entire `alac` atom payload as codec private data. mkvtoolnix passes only the ALAC magic cookie/config bytes after the `alac` atom header and FullBox header to the ALAC packetizer.
