# FLAC Parser

Implementation progress: 96%

## Purpose

The FLAC parser recognises native FLAC files, extracts STREAMINFO, Vorbis comments, and picture metadata, and reports one lossless audio track plus optional attachment entries.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/flac.rs`
- Shared helper: `src-tauri/src/media_metadata/audio/id3v2.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_flac.cpp`, `../mkvtoolnix/src/input/r_flac.h`, `../mkvtoolnix/src/common/flac.cpp`, `../mkvtoolnix/src/common/flac.h`

The parser skips leading ID3v2 data, checks `fLaC`, walks metadata blocks, decodes STREAMINFO, maps total samples to duration, turns Vorbis comments into tags, promotes title/language fields, and turns PICTURE blocks into attachment metadata.

The codec-private header is rebuilt exactly as `flac_reader_c::read_headers` does (`r_flac.cpp:57-89`): the `fLaC` magic followed by **every metadata block except PICTURE and PADDING** — so STREAMINFO, VORBIS_COMMENT, APPLICATION, SEEKTABLE, CUESHEET, and any unknown blocks are all preserved verbatim — with the "last metadata block" flag re-normalised so only the final kept block carries it. PICTURE blocks become attachments using the **declared** payload length read from the block header; the payload bytes themselves are never materialised, so cover art larger than the bounded 1 MiB header read is still surfaced (a block whose declared payload does not fit within the block, or extends past EOF, is dropped, matching libFLAC's all-or-nothing block read).

## Data Structures

```mermaid
flowchart TD
  A["fLaC stream"] --> B["Metadata block walker"]
  B --> C["FlacStreaminfo"]
  B --> D["Vorbis comments"]
  B --> E["FlacPicture"]
  C --> F["Audio track"]
  D --> G["TagsBundle"]
  E --> H["Attachments"]
```

The central structures are `FlacMetadata`, `FlacStreaminfo`, and `FlacPicture`.

## Gaps and Handling

The MIME-to-extension table for pictures is intentionally small and practical. The Rust parser does not run libFLAC frame validation, and attachment payloads are represented by metadata rather than loading full image data into the model. Each kept metadata block is read up to a 16 MiB bound (and the PICTURE header up to 1 MiB), so a pathologically large block body is capped; in practice STREAMINFO/SEEKTABLE/CUESHEET/APPLICATION blocks are well under that. Those choices keep parsing bounded and match the app's need to list tracks and attachments rather than remux FLAC packets.
