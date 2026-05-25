# FLAC Parser

Implementation progress: 94%

## Purpose

The FLAC parser recognises native FLAC files, extracts STREAMINFO, Vorbis comments, and picture metadata, and reports one lossless audio track plus optional attachment entries.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/flac.rs`
- Shared helper: `src-tauri/src/media_metadata/audio/id3v2.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_flac.cpp`, `../mkvtoolnix/src/input/r_flac.h`, `../mkvtoolnix/src/common/flac.cpp`, `../mkvtoolnix/src/common/flac.h`

The parser skips leading ID3v2 data, checks `fLaC`, walks metadata blocks, decodes STREAMINFO, maps total samples to duration, turns Vorbis comments into tags, promotes title/language fields, and turns PICTURE blocks into attachment metadata.

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

The MIME-to-extension table for pictures is intentionally small and practical. The Rust parser does not run libFLAC frame validation, and attachment payloads are represented by metadata rather than loading full image data into the model. Those choices keep parsing bounded and match the app's need to list tracks and attachments rather than remux FLAC packets.

## Open Issues

- `PARSER-225`: Native FLAC codec-private construction keeps only STREAMINFO and VORBIS_COMMENT. mkvmerge keeps every metadata block except PICTURE and PADDING when building the FLAC packetizer header, so APPLICATION, SEEKTABLE, CUESHEET, and unknown metadata blocks are lost from native codec private data.
- `PARSER-226`: PICTURE attachment extraction requires the full declared image payload to be inside the 1 MiB header read. mkvmerge receives the full picture metadata from libFLAC and can expose larger cover-art attachments; native drops otherwise valid pictures whose metadata block exceeds the cap even though the attachment listing only needs the declared payload length.
