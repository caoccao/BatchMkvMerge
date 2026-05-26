# MPEG Program Stream Parser

Implementation progress: 72%

## Purpose

The MPEG-PS parser recognises MPEG program streams and VOB-like files, discovers PES streams, uses program-stream maps when present, and enriches video/audio metadata from payload prefixes.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/mpeg_ps/reader.rs`
- Related modules: `packet.rs`, `pes.rs`, `stream_map.rs`, `identify.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_mpeg_ps.cpp`, `../mkvtoolnix/src/input/r_mpeg_ps.h`

The parser scans start codes, recognises pack and system headers, parses program stream maps, discovers private-stream sub IDs, accumulates bounded payload prefixes, and classifies MPEG video, AVC, VC-1, MPEG audio, AAC, AC-3, DTS, TrueHD, LPCM, and VobSub-style private streams.

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

## Open Issues

### PARSER-252: MPEG-PS MPEG audio layer specialization is ignored

Native MPEG-PS classifies bare audio stream IDs as `A_MPEG/L3` and Program Stream Map types `0x03`/`0x04` as `A_MPEG/L2`, then enrichment only copies sample rate and channels from the parsed MPEG audio header. It also asks for two consecutive MPEG audio headers, while mkvmerge's `new_stream_a_mpeg` accepts one header, then replaces the track codec with `header.get_codec()` to preserve whether the actual payload is Layer I, II, or III. Native can therefore mislabel MPEG audio tracks whenever the stream-id or PSM default does not match the first real frame, and can miss audio parameters in a short bounded probe that mkvmerge can still identify.
