# TrueHD / MLP Parser

Implementation progress: 100%

## Purpose

The TrueHD parser recognises Dolby TrueHD and MLP streams, extracts sample rate and channel count, and reports an embedded AC-3 substream as a second audio track when present.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/truehd.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_truehd.cpp`, `../mkvtoolnix/src/input/r_truehd.h`, `../mkvtoolnix/src/common/truehd.cpp`, `../mkvtoolnix/src/common/truehd.h`, `../mkvtoolnix/src/common/ac3.cpp`, `../mkvtoolnix/src/common/ac3.h`

The parser skips ID3v2 data, searches the same 512 KiB probe range mkvtoolnix gives to `truehd_reader_c`, classifies MLP versus TrueHD, decodes rate and channel-map fields, and scans enough frames to find both the main stream and a coupled AC-3 frame (PARSER-356).

## Data Structures

```mermaid
flowchart TD
  A["Payload bytes"] --> B["major-sync scan"]
  B --> C["Frame"]
  C --> D["Codec TrueHD or MLP"]
  C --> E["optional AC-3 Frame"]
  D --> F["Audio track 0"]
  E --> G["Audio track 1"]
  F --> H["MediaMetadata"]
  G --> H
```

Key structures are `Frame`, `Codec`, and `FrameType`.

## Gaps and Handling

The Rust parser does not verify AC-3 checksums and does not expose less common debug or Atmos extension fields that upstream can inspect while muxing. The current metadata model records the stream identity and usable audio properties, and the probe/read window now matches mkvtoolnix's 512 KiB header-identification range.

## Open Issues

- `PARSER-359` - The shared ID3v2 skipper does not match `mtx::id3::skip_v2_tag`: invalid version or synchsafe size bytes are masked and accepted, and declared tag-size semantics are not propagated as mkvtoolnix's `-1`/`0`/size result. TrueHD/MLP can therefore skip malformed `ID3`-looking prefixes that mkvtoolnix treats as payload, changing probe and header behavior.
