# AAC Parser

Implementation progress: 90%

## Purpose

The AAC parser recognises raw AAC streams and reports one audio track with codec identity, profile, channel count, sampling frequency, and output sampling frequency when SBR is present. It covers the two raw stream forms that matter for mkvmerge parity: ADTS and LOAS/LATM. ID3v2 data at the beginning of a file is skipped before probing.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/aac.rs`
- Shared helper: `src-tauri/src/media_metadata/audio/id3v2.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_aac.cpp`, `../mkvtoolnix/src/input/r_aac.h`, `../mkvtoolnix/src/common/aac.cpp`, `../mkvtoolnix/src/common/aac.h`

The Rust reader decodes ADTS fixed and variable headers, AudioSpecificConfig, program-config elements, and LOAS/LATM stream-mux configuration. Probing requires eight consecutive valid frames, mirroring mkvmerge's raw-audio confirmation policy. `read_headers` samples a bounded prefix, collects usable frame headers, and writes a `ContainerFormat::Aac` container plus one `TrackType::Audio` track.

## Data Structures

```mermaid
flowchart TD
  A["FileSource"] --> B["ID3v2 payload bounds"]
  B --> C["ADTS or LOAS/LATM frame scan"]
  C --> D["AacHeader"]
  D --> E["AudioTrackProperties"]
  D --> F["CodecInfo A_AAC"]
  E --> G["MediaMetadata.tracks[0]"]
  F --> G
```

Key local structures are `AacHeader`, `MultiplexType`, `LatmResult`, and the small bit reader used for AudioSpecificConfig and LATM payloads.

## Gaps and Handling

Upstream has broader AAC parser branches for less common object types and error-protection details. The Rust parser does not fully mirror ER AAC ELD/CELP paths, ADTS implicit-SBR profile overrides, or mkvmerge's exact search for the first nonzero usable header. Those gaps are handled by returning conservative metadata from the first stable frame sequence and by keeping malformed or underspecified data out of the track list instead of guessing unsupported details.

Packet framing and muxing are upstream responsibilities and are intentionally out of scope for this parser.
