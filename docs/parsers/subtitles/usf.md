# USF Parser

Implementation progress: 90%

## Purpose

The USF parser recognises Universal Subtitle Format XML files and reports one text subtitle track per `<subtitles>` element, carrying the per-track and default-language metadata and the shared codec-private document.

## Implementation

- Primary implementation: `src-tauri/src/media_metadata/subtitles/usf.rs`
- Encoding helper: `src-tauri/src/media_metadata/subtitles/encoding.rs`
- XML engine: `quick-xml` (`=0.39.2`), the event-based stand-in for upstream's pugixml
- Upstream basis: `../mkvtoolnix/src/input/r_usf.cpp`, `../mkvtoolnix/src/input/r_usf.h`

Probing mirrors `usf_reader_c::probe_file` (r_usf.cpp lines 35-47): the leading window must contain an `<?xml` or `<!--` marker, then the document is parsed with a real XML engine and the document (root) element name is validated to be `USFSubtitles`. The document read is bounded at **10 MiB**, matching upstream's `mtx::xml::load_file(..., 10 * 1024 * 1024)` cap (r_usf.cpp line 45).

`read_headers` performs the same three-step walk as upstream:

- `parse_metadata` (r_usf.cpp lines 79-90) — the default language is read from `<metadata><language code="">` directly under the root.
- `parse_subtitles` (r_usf.cpp lines 93-137) — one `S_TEXT/USF` track per direct-child `<subtitles>` element of the root; each track's language comes from a child `<language code="">`.
- `create_codec_private` (r_usf.cpp lines 140-148) — every `<subtitles>` subtree is removed from the document, and the remainder is re-serialized as the single shared codec-private blob handed to every track.

The default-language fallback for tracks lacking a valid language mirrors the loop in `usf_reader_c::read_headers` (r_usf.cpp lines 64-65). Language codes are resolved through `Language::resolve`; invalid codes fall back to `und`.

## Data Structures

```mermaid
flowchart TD
  A["USF XML text (≤ 10 MiB, BOM-stripped)"] --> B["quick-xml event walk"]
  B --> C["root == USFSubtitles?"]
  B --> D["UsfDocument"]
  D --> E["default_language (metadata/language code)"]
  D --> F["tracks: per-subtitles language code"]
  D --> G["codec_private (document minus all subtitles subtrees)"]
  F --> H["Subtitle tracks"]
  G --> H
  E --> H
  H --> I["MediaMetadata"]
```

`UsfDocument` is produced by a single event pass: it validates the root, collects the default language and per-track language codes, and re-serializes the document with every `<subtitles>` subtree dropped for the shared codec private.

## Gaps and Handling

The reader is header-only: it does not decode subtitle entry text, timestamps, or byte sizes (these only matter to the upstream packetizer/extraction path, not identification). An unbalanced/malformed document is rejected as `Unrecognised`. quick-xml's namespace-aware local names are compared, so namespaced documents are tolerated.

## Open Issues

- `PARSER-236`: USF probing uses a broader XML-marker window and namespace-stripped root comparison. mkvmerge only looks for `<?xml` or `<!--` in the first roughly 1000 characters and requires the document element name to be exactly `USFSubtitles`; native scans the full 10 MiB decoded document and accepts namespaced roots such as `<usf:USFSubtitles>`.
