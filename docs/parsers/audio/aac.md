# AAC Parser

Implementation progress: 94%

## Purpose

The AAC parser recognises raw AAC streams and reports one audio track with codec identity, profile, channel count, sampling frequency, and output sampling frequency when SBR is present. It covers the two raw stream forms that matter for mkvmerge parity: ADTS and LOAS/LATM. ID3v2 data at the beginning of a file is skipped before probing.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/audio/aac.rs`
- Shared helper: `src-tauri/src/media_metadata/audio/id3v2.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_aac.cpp`, `../mkvtoolnix/src/input/r_aac.h`, `../mkvtoolnix/src/common/aac.cpp`, `../mkvtoolnix/src/common/aac.h`

The Rust reader decodes ADTS fixed and variable headers, AudioSpecificConfig, program-config elements, and LOAS/LATM stream-mux configuration. Probing requires eight consecutive valid frames, mirroring mkvmerge's raw-audio confirmation policy. `read_headers` samples a bounded prefix, collects usable frame headers, and writes a `ContainerFormat::Aac` container plus one `TrackType::Audio` track.

Object type 29 (Parametric Stereo / HE-AACv2) is decoded as an SBR-style extension — when its bitstream guard passes, the output sample rate and inner object type are read and `aac_ps_present` is set, mirroring `header_c::parse_audio_specific_config` (`../mkvtoolnix/src/common/aac.cpp:1224-1232`). Raw-AAC identification promotes ADTS headers with a sample rate of 24 kHz or below to the SBR profile (`PROFILE_SBR`), matching `aac_reader_c::read_headers`/`identify` (`../mkvtoolnix/src/input/r_aac.cpp:73-76`); the core sampling frequency is still reported as-is.

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

Upstream has broader AAC parser branches for less common object types and error-protection details. The Rust parser does not fully mirror ER AAC ELD/CELP paths or mkvmerge's exact search for the first nonzero usable header. Those gaps are handled by returning conservative metadata from the first stable frame sequence and by keeping malformed or underspecified data out of the track list instead of guessing unsupported details.

Packet framing and muxing are upstream responsibilities and are intentionally out of scope for this parser.

## Open Issues

- `PARSER-224`: Raw AAC `read_headers` still emits the first header from the confirmed frame run. mkvmerge drains parsed frames until it finds one with both nonzero sample rate and channel count, so ADTS/LOAS streams whose first confirmed header has channel configuration 0 without a parsed PCE can be reported with missing audio properties instead of advancing to the first usable frame.
