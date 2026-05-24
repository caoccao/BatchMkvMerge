# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Common commands

```bash
pnpm install                     # install JS dependencies
pnpm tauri dev                   # run the desktop app with frontend + Rust hot-reload
pnpm tauri build                 # build installers for the current platform
pnpm build                       # tsc + vite build (type-check + frontend bundle only)
cd src-tauri && cargo check      # fast Rust type-check without linking
cd src-tauri && cargo test       # run the Rust unit + integration test suite
```

Version bumps are done with a Deno script — edit the two arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts && deno task change-version
```

That rewrites `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and all three `.github/workflows/*.yml` files.

### Rust toolchain

The toolchain is pinned in `src-tauri/rust-toolchain.toml` (currently **1.94.0**). The pin is required by `specta-2.0.0-rc.25`, which relies on `debug_closure_helpers` (stable since 1.91). The GitHub Actions workflows use `actions-rust-lang/setup-rust-toolchain@v1`, which honours the pin. They also clear `rustflags` so the action's default `-D warnings` doesn't fail CI on intentionally-not-yet-consumed parser surface during phased delivery.

### Tests

The parser sub-tree (`src-tauri/src/media_metadata/`) ships with unit tests inline (`#[cfg(test)] mod tests {}`) and one integration test at `src-tauri/tests/protocol_typescript.rs`. `cargo test` runs both. Coverage is measured with `cargo-llvm-cov` — every parser sub-module is held to ≥ 90 % line coverage. To refresh the checked-in `src/protocol.generated.ts` after changing any model struct:

```bash
cd src-tauri && BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript
```

The non-regen run of that test fails with a line-by-line diff when the checked-in file is stale — CI catches drift automatically.

## Platform-specific notes

- Development here happens on Windows with Git Bash. Use forward-slash paths (`C:/…` not `C:\\…`), `/dev/null` not `NUL`, and avoid `cd <project>` before `git` commands — git already runs against the working tree and prepending `cd` triggers an extra permission prompt.
- MKVToolNix (`mkvmerge`, `mkvmerge`) must be installed separately — the app shells out to them.

## Architecture

### Frontend ↔ Backend split

React app under `src/` talks to a Rust Tauri v2 backend under `src-tauri/`. All communication goes through Tauri's `invoke` (commands) and the event system — no other IPC. Everything crossing the boundary is JSON; `src/protocol.ts` mirrors `src-tauri/src/protocol.rs` and must stay in sync (including the serde `#[serde(rename = "camelCase")]` attributes vs. the TS interface fields).

### Single source of truth: Zustand store

`src/store.ts` owns:
- the dropped file list,
- the extraction queue (grouped per-drive on Windows — single bucket on Linux/macOS), with per-item status, progress, timestamps, cancel flag, and error,
- the persisted config (theme, language, profiles, mkvtoolnix path, window geometry),
- a registry of per-card extract handlers and selection flags that the Toolbar consumes for Extract All.

**Important:** `updateConfig` applies the patch optimistically and discards the backend's response. Don't re-add a post-await `set({ config: saved })` — it re-introduces a race condition where rapid typing reverts edits (older async responses land on top of newer optimistic state). The backend also doesn't transform the config inside `set_config`, so the response would be identical anyway.

### Status state machine

`QueueItemStatus` is a string enum in `src/protocol.ts`: `Waiting | Extracting | Completed | Cancelled | Failed`. Backend snapshots only ever carry `Waiting` / `Extracting`; terminal states are set on the frontend:

1. The backend emits an `extraction-finished` event when the worker exits, carrying the authoritative outcome (`Completed`, `Cancelled`, or `Failed`) — `FileList.tsx` listens and calls `recordFinishedOutcome` which sets the status directly.
2. As a fallback, `applyExtractSnapshot` transitions any item that's disappeared from the backend snapshot to `Completed` (or `Cancelled` if `cancelRequested` was flagged). This handles races where the event arrives after the next poll.
3. The terminal-status check in `applyExtractSnapshot` skips items already in a terminal state, so the event-driven update is never overwritten.

Polling runs every **200 ms** from `FileList.tsx` via `get_extract_status`.

### Per-drive extraction queue (backend)

`src-tauri/src/extract.rs` owns all live extraction state via a module-level `OnceLock<Mutex<ExtractState>>`:

- `tasks: HashMap<file, TaskState>` — metadata (status, progress, args, cancel flag).
- `drives: HashMap<drive_key, DriveState>` — per-drive `extracting + queued`.
- `children: HashMap<file, Arc<Mutex<Child>>>` — kept separately so `cancel()` can `kill()` from a different call site.

`get_drive_key` returns the Windows path `Prefix` component (`C:`, `\\server\share`) uppercased, else `"default"`. When a file is enqueued and the drive's `extracting` slot is free, it's promoted immediately and a worker is `std::thread::spawn`'d. When a worker finishes, `on_worker_finished` emits the event, drops the task, and pops the next file for the same drive.

The tokio runtime is intentionally capped at 4 workers (see `lib.rs` → `tauri::async_runtime::set(runtime.handle().clone())` with `worker_threads(4)`). The actual extraction work runs on dedicated `std::thread::spawn` threads, not on those 4 workers, so the polling endpoint stays responsive under load.

### Profiles and filename templates

Profiles live in the persisted config (one `ConfigProfile` per entry, with a shared `activeProfile` pointer). Each profile carries three templates (video / audio / subtitle) and three auto-select flags. The `Default` profile auto-selects subtitle tracks.

Templates are expanded by `extract-utils.ts::renderTemplate`, a **single-pass character scanner** (not regex-based — don't regress this). It supports `{file_name}`, `{track_id}`, `{track_number}`, `{language}`, `{codec_name}`, `{track_name}`. `{{` / `}}` escape to literal braces; unknown placeholders are emitted verbatim so typos are visible. `{codec_name}` and `{track_name}` are sanitized for filesystem-unsafe characters before substitution.

The active profile is consumed both on track auto-select (once per card, guarded by `autoSelectedRef`) and at extract time (passed into `buildExtractArgs` / `buildCommandString`).

### Window flicker

`tauri.conf.json` creates the main window with `visible: false`. The setup hook in `src-tauri/src/lib.rs` applies the stored position and size, then calls `window.show()` — this is why users don't see a default-geometry flash on startup. Don't flip `visible` back on.

### Config file location

Config is stored per-OS:
- Windows — `%APPDATA%\BatchMkvMerge\` in installed mode (exe under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`), next to the `.exe` in portable mode.
- Linux — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- macOS — `~/Library/Application Support/BatchMkvMerge/`.

Detection lives in `src-tauri/src/config.rs::get_config_dir`. Old config files from earlier schema versions still load because every new field has `#[serde(default = "...")]`. Notable nested blocks:

- `config.externalTools` — MKVToolNix + BetterMediaInfo paths.
- `config.parser.timeoutMs` — per-file parse budget for the native parser (default 1000, clamped to 100–60000 ms by `ConfigParser::effective_timeout_ms`). Exposed in Settings → Parser tab.

### i18n

Nine locales (`de`, `en-US`, `es`, `fr`, `it`, `ja`, `zh-CN`, `zh-HK`, `zh-TW`). When adding a user-facing string, add the key to **all nine** files under `src/i18n/locales/`. Missing keys fall back to `en-US` via i18next, but that's not a design pattern to lean on.

### Native media-metadata parser

A pure-Rust header-only parser is being phased in under `src-tauri/src/media_metadata/`. It will replace the `mkvmerge -J` subprocess shellout in `mkvtoolnix.rs::get_mkv_tracks` and broaden the drag-drop filter beyond `.mkv`. Delivery is split into 12 phases (Phase 1 = io/error/deadline foundations, Phase 2 = model + codec/language tables + Settings UI, Phase 3 = matroska reader + probe foundation, Phase 4 = MP4/QuickTime reader, Phase 5 = AVI + Ogg/OGM readers, Phases 6-10 = remaining containers + elementary streams + subtitles, Phase 11 = Tauri command + frontend migration, Phase 12 = i18n widening + CI coverage gate). Each phase lands as one Conventional Commits commit on the `implement-parser` branch.

Layout (one module tree per format family — every file under 1000 LOC):

```
src-tauri/src/media_metadata/
├── mod.rs              # `pub fn parse(path, ParseOptions)`; ParseError; Deadline
├── error.rs            # ParseError enum (Timeout / Io / Malformed / OversizedElement / ...)
├── deadline.rs         # soft per-file budget; `check(stage)` at every coarse boundary
├── reader.rs           # `trait Reader { probe; read_headers }` — populates &mut MediaMetadata
├── io/                 # FileSource, BufReader-wrapped, BitReader, endian, VINT decoders
├── codec/              # Matroska CodecID + FOURCC + MPEG-TS stream_type lookup tables
├── language/           # ISO 639-2 alpha-3 table + BCP-47 wrapper (`language-tags` crate)
├── model/              # Wire-format structs — camelCase, nested, never flattened
├── probe/              # 6-phase dispatch cascade + extension table + magic signatures
├── matroska/           # native EBML reader (ebml, ids, info, seek_head, tracks/*, attachments, chapters, tags)
├── mp4/                # native MP4/QuickTime reader (atom, ftyp, moov/*, codec_specific/*, meta/*, fragments)
├── avi/                # native AVI reader (riff, avih, strl, odml, identify, reader)
└── ogg/                # native Ogg/OGM reader (page, codecs/*, comments, identify, reader)
```

**Probe registry:** `probe::dispatch` walks `probe::registered_readers()` in priority order, calling `Reader::probe` on each. The first reader that claims the file is handed `read_headers`. Adding a new format reader is a one-line insert at the right priority level (see `probe/dispatch.rs::registered_readers`). The registry currently contains Matroska + AVI + Ogg + MP4 readers; other formats land in subsequent phases.

**Matroska reader:** pure-Rust port of `mkvtoolnix/src/input/r_matroska.cpp` — no libebml/libmatroska dependency. The EBML walker (`matroska/ebml.rs`) is iterator-based (callers maintain their own container stack, so user-controlled nesting depth never blows the stack). All element IDs are in `matroska/ids.rs`. SeekHead-based dispatch mirrors mkvtoolnix's `m_deferred_l1_positions` bookkeeping. Cluster payloads are never entered — header-only.

**MP4 reader:** pure-Rust port of `mkvtoolnix/src/input/r_qtmp4.cpp` — header-only walk of the ISO BMFF / QuickTime box hierarchy. Supports 32-bit, 64-bit large-size, and size=0 (to-EOF) box forms. `ftyp` classifies QuickTime (`qt  `) vs MP4 brands into `ContainerFormat`; `moov` drives `mvhd` + per-`trak` walks (`tkhd`, `mdia → mdhd / hdlr / minf → stbl → stsd / stts`, `edts/elst`). Codec-specific sub-boxes (`avcC`, `hvcC`, `esds`, `colr`, `pasp`, `dvcC` / `dvvC`) populate `VideoCodecConfig` / `AudioCodecConfig`. iTunes metadata (`udta → meta → ilst`) feeds container title / muxing app / date_utc; unknown tags land in `tags.global`. Fragmented MP4 (`mvex/trex` + `moof/traf/tfhd/trun`) sets `is_fragmented` and aggregates fragment sample counts into `num_index_entries`. Cluster-equivalent `mdat` payloads are never read.

**AVI reader:** pure-Rust port of `mkvtoolnix/src/input/r_avi.cpp` — walks the RIFF chunk hierarchy via a hand-rolled chunk walker (no `avilib` dependency). `RIFF/AVI ` is the entry point; `LIST/hdrl` hosts `avih` (MainAVIHeader → frame interval, total frames, dimensions, flags) and one `LIST/strl` per stream containing `strh` (kind + codec FOURCC + timebase) and `strf` (`BITMAPINFOHEADER` for video, `WAVEFORMATEX(TENSIBLE)` for audio). ODML's `LIST/odml/dmlh` provides the 32-bit total-frame count for files > 2 GB. Negative `BITMAPINFOHEADER` heights (top-down DIB) are flipped positive; WAVEFORMATEX `extra` bytes become `codec_private`.

**Ogg / OGM reader:** pure-Rust port of `mkvtoolnix/src/input/r_ogm.cpp`. Walks pages per RFC 3533 (no `ogg` crate dependency) — extracts `bitstream_serial`, `granule_position`, segment-table → packet boundaries. The first packet of each Beginning-Of-Stream page is fed to the codec sniffers under `ogg/codecs/`: Vorbis (`\x01vorbis`), Opus (`OpusHead`), Theora (`\x80theora` + KEYFRAME_GRANULE), FLAC-in-Ogg (`\x7FFLAC` + STREAMINFO), Speex (`Speex   ` 8-byte signature), Kate (`\x80kate\0\0\0`), and OGM legacy stream headers (`\x01video/audio/text...`). VorbisComment blocks on the second packet populate per-track tags + the container's `muxing_app`. Stops once every stream has a comment block to keep identification fast for huge files.

The sub-tree opts into `#![forbid(unsafe_code)]` at `media_metadata/mod.rs`.

**Wire-format invariants** (see [[feedback_protocol_shape]] memory):

- camelCase via `#[serde(rename_all = "camelCase")]` everywhere.
- Domain sub-trees (`video / audio / subtitle`) live as `Option<_>` on `TrackProperties` — never flattened into the parent.
- Fields containing digits use explicit `#[serde(rename = "...")]` because serde's `camelCase` rule mangles them (e.g. `iso639_2` → `iso6392` without the override).
- `u64` / `i64` fields **must** carry `#[specta(type = specta_typescript::Number)]` (or `Option<Number>` / `Vec<Number>`) — specta-typescript otherwise rejects them to prevent JS precision loss. Accept the loss explicitly via the override.
- `protocol_version: u32` (currently 1) is ours; bump it on breaking changes.

The TypeScript counterpart `src/protocol.generated.ts` is **auto-generated by specta** from the model structs and **must be regenerated** any time a model struct changes (`BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript`). The non-regen run of the same test asserts the checked-in file matches what specta would emit, so CI catches drift. Don't edit it by hand.

**Configurable timeout** (see [[feedback_parser_timeout]] memory): the per-file budget comes from `config.parser.timeoutMs` (default 1000, clamped 100–60000). On expiry the parser returns `Err(ParseError::Timeout { budget_ms, stage })` immediately — no partial result. The `BMM_PARSER_BUDGET_MS` env var is kept as a dev-only override; the persisted config wins when both are set.

### External tools

Two optional external binaries are integrated, both opt-in via Settings:

- **MKVToolNix** (`mkvmerge`, `mkvmerge`) — required for the core extraction flow; the app shells out to them.
- **BetterMediaInfo** — optional; when its path is configured, the "Open in BetterMediaInfo" entry becomes available on each card.

Both paths live under `config.externalTools` (`mkvToolNixPath`, `betterMediaInfoPath`). Backend probe commands return `MkvToolNixStatus` / `BetterMediaInfoStatus` (`src-tauri/src/protocol.rs`) with auto-detected directories — see `controller.rs` for the per-OS search paths (Windows registry / `Program Files`, macOS `/Applications`, Linux `$PATH`). The shared **`src/components/settings/ExternalToolPathRow.tsx`** + **`useToolPathDetection.ts`** drive the Browse/Detect UI for both tools — change them once and both settings rows update.

## Project conventions

- **Copyright headers** use `2026` only (e.g. `Copyright (c) 2026. caoccao.com Sam Cao`, `© Copyright 2026`). Don't reintroduce ranges like `2024-2026` — this project started in 2026; the range is a template-copy artifact from other caoccao projects.
- Status labels in the UI use **PascalCase enum values** (`Waiting`, `Extracting`, …). i18n keys that back them are lowercase (`queue.status.waiting`, …); the `statusLabel` helper in `Queue.tsx` bridges with `.toLowerCase()`.
- macOS `Stack` alignment must use `sx={{ alignItems: "center" }}`, not the `alignItems` prop — MUI v9 dropped the prop.
- When cards register their `handleExtract` into `fileExtractHandlers`, the signature is `() => Promise<void>`. The toolbar's Extract All awaits each handler sequentially so backend per-drive queue order matches the on-screen file order.
- All `if-else` must have braces.
- Commit messages follow **Conventional Commits** (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, `test:`, `build:`) with a detailed body. The parser delivery uses one commit per phase.
- When adding `u64` / `i64` fields to any `media_metadata::model` struct, annotate them with `#[specta(type = specta_typescript::Number)]` (or `Option<Number>` / `Vec<Number>` as appropriate) and re-run `BMM_REGEN_PROTOCOL_TS=1 cargo test --test protocol_typescript` so `src/protocol.generated.ts` stays in sync.
- Never edit `src/protocol.generated.ts` by hand — it is regenerated from the Rust model.
