# FLV Parser

Implementation progress: 100%

## Purpose

The FLV parser recognises Flash Video files, reads tag headers, extracts script metadata, and reports audio/video tracks for supported FLV codecs.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/flv/reader.rs`
- Related modules: `src-tauri/src/media_metadata/flv/header.rs`, `tag.rs`, `script_data.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_flv.cpp`, `../mkvtoolnix/src/input/r_flv.h`, upstream AMF helpers

The parser validates the FLV header, starts tag scanning at the fixed 9-byte header boundary like mkvtoolnix, walks actual tags in a bounded region independent of stale header type flags, skips encrypted tags, decodes AMF0 `onMetaData` values for width, height, and frame rate, and parses AAC, MP3, H.264, H.265, Sorenson H.263, VP6, and VP6-alpha metadata. Clear tag types are classified by exact flag byte (`0x08`, `0x09`, `0x12`) so reserved high bits do not masquerade as audio/video/script tags.

For the per-frame duration mkvmerge reports, the AMF `framerate` wins; for AVC/HEVC the value then falls back to the SPS VUI timing (`num_units_in_tick` / `time_scale`) and finally to mkvmerge's 25 fps default, matching `new_stream_v_avc` / `new_stream_v_hevc` (`../mkvtoolnix/src/input/r_flv.cpp:427-445`, `455-472`). Other codecs keep a default duration only when AMF supplied a frame rate. AVC/HEVC sequence-header tags always preserve the private config bytes and mark the stream discovered; SPS/PPS/VPS parsing only enriches dimensions and structured codec details when the config is complete enough. HEVC `hvcC` fallback fields read `chromaFormat`, `bitDepthLumaMinus8`, and `bitDepthChromaMinus8` from bytes 16, 17, and 18, matching the `HEVCDecoderConfigurationRecord` layout and mkvtoolnix's `hevcc_c::unpack`. AAC tags mark the audio track valid from the FLV audio tag flags even when the AAC packet is raw data or its sequence header cannot be parsed; a valid AudioSpecificConfig still enriches codec private/config details when present.

## Data Structures

```mermaid
flowchart TD
  A["FLV header"] --> B["Tag walker"]
  B --> C["AudioState"]
  B --> D["VideoState"]
  B --> E["ScriptMetadata"]
  C --> F["Audio track"]
  D --> G["Video track"]
  E --> G
```

Key structures are `FlvHeader`, `FlvTagHeader`, `AudioTagFlags`, `VideoCodecId`, `ScriptMetadata`, and internal audio/video state.

## Gaps and Handling

Rust extracts selected AMF fields and does not perform timestamp/min-offset work or packet muxing. AVC/HEVC now mirror upstream's SPS-timing-then-25-fps default-duration fallback and keep sequence-header tracks even when config parsing cannot recover dimensions. Unsupported Screen video codecs are dropped like upstream, encrypted payloads are skipped rather than parsed, and AAC discovery follows upstream's flag-derived fallback behavior.
