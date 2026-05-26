# MPEG Program Stream Parser

Implementation progress: 74%

## Purpose

The MPEG-PS parser recognises MPEG program streams and VOB-like files, discovers PES streams, uses program-stream maps when present, and enriches video/audio metadata from payload prefixes.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/mpeg_ps/reader.rs`
- Related modules: `packet.rs`, `pes.rs`, `stream_map.rs`, `identify.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_mpeg_ps.cpp`, `../mkvtoolnix/src/input/r_mpeg_ps.h`

The parser scans start codes, recognises pack and system headers, parses program stream maps, discovers private-stream sub IDs, accumulates bounded payload prefixes, and classifies MPEG video, AVC, VC-1, MPEG audio, AAC, AC-3, DTS, TrueHD, LPCM, and VobSub-style private streams.

MPEG audio (bare stream ids `0xC0..0xDF`, defaulted to `A_MPEG/L3`, and PSM stream types `0x03`/`0x04`, defaulted to `A_MPEG/L2`) is relabelled to the actual Layer I / II / III once the first frame header decodes — mirroring `new_stream_a_mpeg`'s `codec = header.get_codec()` (`r_mpeg_ps.cpp`). The probe needs only a single frame header (not two), matching upstream's `find_mp3_header`, so a short bounded payload that mkvtoolnix can identify is not rejected. When no header decodes, the table default id is retained.

## Data Structures

```mermaid
flowchart TD
  A["Start-code stream"] --> B["Packet scanner"]
  B --> C["ProgramStreamMap"]
  B --> D["StreamObservation"]
  D --> E["payload enrichment"]
  E --> F["Track"]
  C --> F
```

Key structures are `StartCode`, `PesHeader`, `ProgramStreamMap`, `PsmEntry`, and `StreamObservation`.

## Gaps and Handling

Upstream has broader scaling probe windows, timestamp-offset calculation, multi-file VOB opening, packet delivery, and more late-stream recovery. Rust keeps bounded discovery and payload enrichment so metadata extraction remains fast and deterministic.
