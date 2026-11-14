# Development

## Build rules

- **Always build and run the release profile. Never build a debug build.**
  Every cargo invocation in this repo, in docs and in CI carries `--release`
  (or `-r`): `cargo build --release`, `cargo run --release`,
  `cargo test --release`, `cargo check --release`, `cargo llvm-cov --release`.
  GPUI debug builds are slow to compile, slow to start and render at a
  fraction of release frame rates. Do not add `[profile.dev]` tuning as a
  workaround; use release.
- **Installers are built with the `dist` profile**, which inherits `release`
  and adds LTO, `codegen-units = 1` and symbol stripping:
  `cargo packager --profile dist`, `cargo generate-rpm --profile dist`.
  `dist` is a release build with slower, tighter optimisation; it is used for
  packaging only, never for the edit-test loop.

## Migration in progress

The Tauri v2 + React/MUI shell is being replaced by a native [gpui-kit](https://crates.io/crates/gpui-kit) application on the `migrate-gpui-kit` branch, in phases 0-14, one Conventional Commit per phase. The maintainer keeps the plan outside the repository; `CLAUDE.md` lists the phases. Until the Tauri app is removed (Phase 12), it stays buildable and this document describes it.

## Tech Stack

### Frontend

- React 19 + TypeScript + Vite
- Material UI 9, Emotion, and dnd-kit for drag/reorder UX
- i18next + react-i18next for localization (9 locales)
- Zustand as the single source of truth for files, queue state, metadata, and config
- Tauri JS APIs for commands/events and desktop integrations

### Backend

- Rust (edition 2024) with Tauri v2
- Native Rust media metadata parser under `src-tauri/src/media_metadata/`
- JSON-only wire protocol (`invoke` commands + Tauri events)
- Tokio runtime for async command handling plus dedicated worker threads for merge processes

### Tauri plugins

- `tauri-plugin-clipboard-manager`
- `tauri-plugin-dialog`
- `tauri-plugin-opener`

### External tools

- MKVToolNix (`mkvmerge`) is required for merge execution
- BetterMediaInfo is optional and enabled when configured in Settings

## Tooling

- [pnpm](https://pnpm.io/) — package manager
- [Rust](https://www.rust-lang.org/) 1.100.0 — pinned in `src-tauri/rust-toolchain.toml`
- [Deno](https://deno.com/) — scripts under `scripts/ts/` (e.g. `change-version.ts`)

## Repository layout

- `src/`: React app (UI, state store, protocol/types, service wrappers)
- `src/components/`: UI screens/widgets (cards, queue, settings, toolbar)
- `src/i18n/locales/`: locale dictionaries (9 languages)
- `src-tauri/src/`: Tauri backend (config, controller, merge queue, protocol)
- `src-tauri/src/media_metadata/`: native parser implementation
- `src-tauri/tests/`: Rust integration tests (including TypeScript protocol regeneration checks)
- `docs/parsers/`: parser coverage and behavior notes per format
- `scripts/ts/`: maintenance scripts (including multi-file version bumping)

## Commands

```bash
pnpm install                           # install JS dependencies
pnpm tauri dev --release               # run the app in development (hot-reload frontend + Rust, release build)
pnpm tauri build                       # build installers for the current platform
pnpm dev                               # run only the Vite dev server (no Tauri window)
pnpm build                             # type-check and build the frontend
cd src-tauri && cargo check --release  # fast Rust type-check
cd src-tauri && cargo test --release   # run Rust unit + integration tests
```

To bump the version across `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and the three workflow files, edit both arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts
deno task change-version
```

## Configuration storage

The persisted config file is named `BatchMkvMerge.json` and lives at:

- **Windows** — `%APPDATA%\BatchMkvMerge\` when installed under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`; next to the `.exe` in portable mode.
- **Linux** — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- **macOS** — `~/Library/Application Support/BatchMkvMerge/`.

## Frontend ↔ Backend protocol

All communication goes through Tauri's `invoke` and event system; there's no IPC codec beyond JSON.

### Commands (`#[tauri::command]`)

- App/config/update:
	- `get_about`
	- `get_config`
	- `set_config`
	- `get_update_result`
	- `skip_version`
	- `get_launch_args`
- File discovery + metadata:
	- `get_media_files`
	- `get_media_metadata`
- Merge workflow:
	- `enqueue_merge`
	- `cancel_merge`
	- `get_merge_status`
	- `resolve_merge_output_path`
	- `resolve_overridden_output_path`
	- `check_output_path_writable`
	- `output_path_exists`
- External tools:
	- `is_mkvtoolnix_found`
	- `detect_better_media_info`
	- `launch_better_media_info`

### Events

- `merge-finished`
	- Emitted by the backend when a merge worker exits.
	- Payload: file path, terminal outcome (`Completed`, `Cancelled`, `Failed`), optional error.
	- Frontend listens in `FileList.tsx` and finalizes queue status/notifications.

## Concurrency

- Merge scheduling is per drive:
	- each drive has at most one active `mkvmerge` worker,
	- additional items for that drive are queued FIFO.
- Different drives can merge in parallel.
- Backend snapshots expose active states (`Waiting`, `Merging`); terminal outcomes are finalized by event handling plus snapshot fallback logic.
- Frontend polls `get_merge_status` every 200 ms for progress updates.

## Track-name automation

- Profiles include language and naming automation (`reset_und_language`, `set_track_name`, `reset_default_track`, `reset_forced_display`).
- The automation settings are persisted in config and applied by the frontend track workflow.

## CI

Three GitHub Actions workflows build release bundles on push/PR:

- `linux_build.yml` — `.deb`, `.rpm`, `.AppImage`
- `macos_build.yml` — `.dmg` (x86_64 and arm64 via matrix)
- `windows_build.yml` — `.msi`, `.exe` (NSIS)

Each installs Node 24, pnpm 11, and the Rust toolchain pinned in `src-tauri/rust-toolchain.toml` (1.100.0), then runs frontend build + Rust tests (`cargo test -r`) before packaging.

Linux additionally enforces parser line coverage (`cargo llvm-cov --fail-under-lines 90`) for `src-tauri/src/media_metadata/`. The CI step gains `--release` in Phase 1 of the migration; run it locally as `cargo llvm-cov --release`.
