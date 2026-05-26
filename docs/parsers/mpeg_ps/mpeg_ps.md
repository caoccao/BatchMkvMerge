# MPEG Program Stream Parser

Implementation progress: 75%

## Purpose

The MPEG-PS parser recognises MPEG program streams and VOB-like files, discovers PES streams, uses program-stream maps when present, and enriches video/audio metadata from payload prefixes.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/mpeg_ps/reader.rs`
- Related modules: `packet.rs`, `pes.rs`, `stream_map.rs`, `identify.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_mpeg_ps.cpp`, `../mkvtoolnix/src/input/r_mpeg_ps.h`

The parser scans start codes, recognises pack and system headers, parses program stream maps, discovers private-stream sub IDs, accumulates bounded payload prefixes, and classifies MPEG video, AVC, VC-1, MPEG audio, AAC, AC-3, DTS, TrueHD, LPCM, and VobSub-style private streams.

MPEG audio (bare stream ids `0xC0..0xDF`, defaulted to `A_MPEG/L3`, and PSM stream types `0x03`/`0x04`, defaulted to `A_MPEG/L2`) is relabelled to the actual Layer I / II / III once the first frame header decodes — mirroring `new_stream_a_mpeg`'s `codec = header.get_codec()` (`r_mpeg_ps.cpp`). The probe needs only a single frame header (not two), matching upstream's `find_mp3_header`, so a short bounded payload that mkvtoolnix can identify is not rejected. When no header decodes, the table default id is retained.

Program Stream Map classification is limited to mkvmerge's `found_new_stream` `es_type` switch: `0x01`, `0x02`, `0x03`, `0x04`, `0x0f`, `0x10`, `0x11`, `0x1b`, `0x80`, and `0x81`. Unsupported nonzero PSM stream types are left unclassified and dropped rather than falling back to a bare stream-id guess; DTS, TrueHD, LPCM, and VobSub handling still comes from private-stream-1 substream ids.

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

### PARSER-272: MPEG-1 PES optional headers are skipped with the MPEG-2-only layout

`pes_payload_offset` always assumes the MPEG-2 PES optional header shape and returns `9 + bytes[8]`. The MPEG-PS reader uses that offset for bare audio/video streams and private-stream-1 payloads before codec probing.

mkvtoolnix's MPEG-PS reader supports the older MPEG-1 PES layout as well: after the 6-byte packet prefix it skips stuffing bytes, optional STD buffer bytes, MPEG-1 PTS/DTS encodings, the MPEG-2 optional header when present, or the `0x0f` marker before exposing elementary payload (`r_mpeg_ps.cpp:347-466`). MPEG-1 program streams do not have `PES_header_data_length` at byte 8.

For MPEG-1 PES packets, Rust can interpret stuffing, PTS/DTS, or elementary payload bytes as the MPEG-2 header length and skip into or past the real stream data. That can hide MPEG video, MPEG audio, AC-3/DTS/LPCM, or private-stream headers that mkvtoolnix would see. Fix by porting the MPEG-1/MPEG-2 PES depacketizing logic before applying private-stream substream skips and codec probes.
