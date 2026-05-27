# DTS / DTS-HD Parser

Implementation progress: 100%

## Purpose

The DTS parser recognises DTS core streams and DTS-HD chunked files. It reports codec family, channel count, sample rate, bit depth, and DTS-HD specialization when the header exposes it.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/dts.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_dts.cpp`, `../mkvtoolnix/src/input/r_dts.h`, `../mkvtoolnix/src/common/dts.cpp`, `../mkvtoolnix/src/common/dts.h`, `../mkvtoolnix/src/common/dts_parser.cpp`, `../mkvtoolnix/src/common/dts_parser.h`

The Rust implementation detects 16-bit big-endian, 16-bit little-endian, 14-bit big-endian, and 14-bit little-endian sync forms. It can transform 14-bit and swapped data into a normal frame view before parsing. DTS-HD `DTSHDHDR` chunks are walked until EOF to find the first usable `STRMDATA` payload, and files with a DTS-HD header but no stream-data chunk are rejected instead of falling back to byte zero. Extension substreams are inspected for XLL, LBR, X96, XCH, channel masks, and source PCM resolution (PARSER-357).

## Data Structures

```mermaid
flowchart TD
  A["File prefix"] --> B["sync or DTSHDHDR"]
  B --> C["byte-swap / 14-bit transform"]
  C --> D["Header"]
  D --> E["DtsType and assets"]
  E --> F["AudioTrackProperties"]
  E --> G["CodecInfo A_DTS"]
  F --> H["MediaMetadata"]
  G --> H
```

Key structures include `Header`, `DtsType`, internal `Asset`, and helper enums for frame and LFE types.

## Gaps and Handling

Only the first DTS-HD `STRMDATA` payload is used for metadata. Upstream keeps richer packet-era state for selecting core versus extension payloads while muxing, which is not needed for the native metadata parser. DTS-HD chunk discovery now mirrors mkvtoolnix's identify behavior by walking the chunk chain to EOF and treating a missing stream-data chunk as unrecognised.

## Open Issues

- `PARSER-359` - The shared ID3v2 skipper does not match `mtx::id3::skip_v2_tag`: invalid version or synchsafe size bytes are masked and accepted, and a declared tag size beyond the bounded probe bytes can leave callers slicing past the bytes actually read. DTS can therefore skip malformed `ID3`-looking prefixes that mkvtoolnix treats as payload, or panic instead of returning `Unrecognised`.
