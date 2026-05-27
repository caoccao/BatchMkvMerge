# AVC / H.264 Elementary Stream Parser

Implementation progress: 100%

## Purpose

The AVC parser recognises raw Annex B H.264 elementary streams and reports one video track with dimensions, display dimensions, profile, level, chroma format, bit depth, optional VUI-derived frame duration, and AVC decoder configuration bytes.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/elementary/avc/reader.rs`
- Helpers: `src-tauri/src/media_metadata/elementary/avc/nal.rs`, `src-tauri/src/media_metadata/elementary/avc/sps.rs`
- Upstream basis: `../mkvtoolnix/src/input/r_avc.cpp`, `../mkvtoolnix/src/input/r_avc.h`, `../mkvtoolnix/src/common/avc/*`, `../mkvtoolnix/src/common/xyzvc/*`

The reader scans a bounded prefix for Annex B start codes in 1 MiB chunks, up to the same fifty chunks mkvtoolnix feeds into its AVC elementary-stream parser. `read_headers` checks the configured parser deadline between chunks. The raw probe rejects a first probe byte of `0x47` to avoid claiming MPEG-TS sync-prefixed data, matching mkvtoolnix's `might_be_xyzvc` guard. The scan splits NAL units, requires SPS/PPS, and marks the configuration ready on the same NAL classes that make `mtx::avc::es_parser_c::headers_parsed()` possible: non-IDR/IDR/data-partition slice NALs or non-filler default-branch NALs after the parameter sets. AUD, end-of-sequence, end-of-stream, and filler NALs do not count by themselves. The parser strips emulation-prevention bytes, parses the SPS RBSP, rejects SPS entries whose cropped dimensions are zero, and builds AVCDecoderConfigurationRecord-style codec private data. The SPS constraint-set / profile-compatibility byte (`rbsp[1]`) is captured as `AvcSps::profile_compat` and written verbatim into avcC byte 2, mirroring `buffer[2] = sps.profile_compat` in `../mkvtoolnix/src/common/avc/avcc.cpp::pack`.

The SPS VUI is decoded for both the sample aspect ratio and frame timing. The PAR is read from `aspect_ratio_idc` (the predefined `s_predefined_pars` table) or the `EXTENDED_SAR` (255) explicit 16-bit numerator/denominator, and `AvcSps::display_dimensions` applies it to the cropped pixel dimensions exactly as `es_parser_c::get_display_dimensions` does (PAR ≥ 1 stretches width, PAR < 1 stretches height); with no usable PAR the display dimensions equal the cropped pixel dimensions. The VUI frame duration is `num_units_in_tick * 1e9 / time_scale`, matching `timing_info_t::default_duration()` (no factor of two). Malformed or truncated VUI data now invalidates the SPS instead of being silently defaulted away.

## Data Structures

```mermaid
flowchart TD
  A["Annex B bytes"] --> B["NAL splitter"]
  B --> C["SPS / PPS selection"]
  C --> D["AvcSps"]
  C --> E["AVC codec private"]
  D --> F["VideoTrackProperties"]
  E --> G["CodecInfo V_MPEG4/ISO/AVC"]
  F --> H["MediaMetadata"]
  G --> H
```

Key structures are `NalUnit`, `AvcSps`, and the internal `AvcHeaders` bundle.

## Gaps and Handling

Rust now uses the same bounded chunk horizon, MPEG-TS first-byte guard, and configuration-ready NAL evidence as mkvtoolnix for header discovery. The PAR and VUI default-duration are derived to match mkvmerge; what remains out of scope is the muxing-time "most often used duration" heuristic (which corrects field/frame-rate conventions from actual frame timestamps) — header-only identification reports the SPS-declared value directly.

## Open Issues

- `PARSER-387` - the strict elementary AVC phase uses the same loose long-scan probe as the late phase. Upstream runs AVC with `require_headers_at_start=true` before raw-audio and container probes, then retries loose elementary streams much later (`reader_detection_and_creation.cpp`). The Rust `STRICT_ELEMENTARY_READERS` entry calls `AvcReader::probe` without a start-required mode; that probe reads up to the bounded long prefix and accepts SPS/PPS plus access-unit evidence wherever it appears, apart from a first-byte TS-sync guard. Files with leading junk or container-like prefixes can therefore be claimed as raw AVC before mkvtoolnix would run its loose AVC pass.
