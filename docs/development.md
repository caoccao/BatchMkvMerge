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

The Tauri v2 + React/MUI shell is being replaced by a native [gpui-kit](https://crates.io/crates/gpui-kit) application on the `migrate-gpui-kit` branch, in phases 0-14, one Conventional Commit per phase. The maintainer keeps the plan outside the repository; `CLAUDE.md` lists the phases. The Tauri app is no longer built or run: `src/` and `src-tauri/` stay in the tree as unbuilt porting reference until Phase 12 deletes them, and sections marked "reference" describe that app. The new app is `crates/bmm-app`.

## Tech Stack

### gpui-kit app

- [gpui-kit](https://crates.io/crates/gpui-kit) 0.9.1 (exact pin; GPUI with the `gpui-pre` backend), in `crates/bmm-app` (package `BatchMkvMerge`)
- [rust-i18n](https://crates.io/crates/rust-i18n) 4.2 for localization (9 locales under `crates/bmm-app/locales/`)
- `crates/bmm-platform`: per-OS integrations GPUI lacks (taskbar / Dock / launcher progress through `ITaskbarList3`, `NSDockTile`, the `com.canonical.Unity.LauncherEntry` D-Bus signal)
- Installers: cargo-packager 0.11.8 (`.app`, `.dmg`, NSIS `.exe`, WiX `.msi`, `.deb`, `.AppImage`) and cargo-generate-rpm 0.21.1 (`.rpm`)

### Frontend (reference, unbuilt)

- React 19 + TypeScript + Vite
- Material UI 9, Emotion, and dnd-kit for drag/reorder UX
- i18next + react-i18next for localization (9 locales)
- Zustand as the single source of truth for files, queue state, metadata, and config
- Tauri JS APIs for commands/events and desktop integrations

### Backend

- Rust (edition 2024), a Cargo workspace: the UI-free `crates/bmm-core` library, `crates/bmm-platform` and `crates/bmm-app`; the Tauri v2 shell `src-tauri` is excluded (reference, unbuilt)
- Native Rust media metadata parser under `crates/bmm-core/src/media_metadata/`
- JSON-only wire protocol (`invoke` commands + Tauri events)
- Tokio runtime for async command handling plus dedicated worker threads for merge processes

### Tauri plugins (reference, unbuilt)

- `tauri-plugin-clipboard-manager`
- `tauri-plugin-dialog`
- `tauri-plugin-opener`

### External tools

- MKVToolNix (`mkvmerge`) is required for merge execution
- BetterMediaInfo is optional and enabled when configured in Settings

## Tooling

- [Rust](https://www.rust-lang.org/) 1.100.0 — pinned in `rust-toolchain.toml` at the repository root
- [cargo-packager](https://crates.io/crates/cargo-packager) 0.11.8 and [cargo-generate-rpm](https://crates.io/crates/cargo-generate-rpm) 0.21.1 — installers (`cargo install --locked --version …`)
- [cargo-llvm-cov](https://crates.io/crates/cargo-llvm-cov) — coverage gate
- [Deno](https://deno.com/) — scripts under `scripts/ts/` (e.g. `change-version.ts`)

## Repository layout

- `src/`: React app (UI, state store, protocol/types, service wrappers) — reference, unbuilt
- `src/components/`: UI screens/widgets (cards, queue, settings, toolbar) — reference, unbuilt
- `src/i18n/locales/`: the React locale dictionaries (9 languages) — reference; the app's are in `crates/bmm-app/locales/`
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`: Cargo workspace root (members `crates/bmm-core`, `crates/bmm-platform`, `crates/bmm-app`; `src-tauri` excluded; one `target/`)
- `crates/bmm-app/`: the gpui-kit app (`src/`; `src/domain/` holds the UI logic without GPUI: merge trees, group cards, size and time labels, queue states, parse-error messages, numeric word navigation; `src/state/` the config, file, queue and navigation stores, with background parsing and the profile automation; `src/merge_events.rs` the bridge from the core's merge queue; `src/actions.rs` enqueue, cancel and clear across stores; `src/ui/` the screens and shortcuts; `src/platform/` window geometry and title, close guard, macOS menus and file dialogs; `src/i18n.rs` and `src/theme.rs`), its locale files (`locales/`), theme sets (`themes/`), Phase 2 prototypes (`examples/spike/`), embedded images (`assets/`) and installer inputs (`packaging/`: icons, desktop entry, WiX template)
- `crates/bmm-platform/src/`: taskbar / Dock / launcher progress per OS
- `crates/bmm-core/src/`: UI-free core (config, merge queue and `MergeEvent`s, services, MKVToolNix, update check, wire DTOs; the track rows of the selection table in `media_track.rs`, the `mkvmerge` command line in `merge_args.rs`, and the MKV language list and its lookups in `languages.rs`)
- `crates/bmm-core/src/media_metadata/`: native parser implementation
- `crates/bmm-core/tests/`: parser fixture integration tests, and `merge_args_golden.rs` with its cases in `tests/golden/` (`*.json` plus the synthetic inputs in `fixtures.rs`)
- `src-tauri/src/`: Tauri shell (commands, window state, notifications) — reference, unbuilt
- `src-tauri/tests/`: TypeScript protocol regeneration check — no longer runs
- `docs/parsers/`: parser coverage and behavior notes per format
- `scripts/ts/`: maintenance scripts (including multi-file version bumping)

## Commands

```bash
cargo run --release -p BatchMkvMerge                         # run the gpui-kit app
cargo run --release -p BatchMkvMerge --example spike         # the Phase 2 prototypes
cargo check --release --workspace                            # fast Rust type-check
cargo test --release --workspace                             # run all Rust tests
cargo llvm-cov --release -p bmm-core                         # line coverage of the core crate (CI gate: 90 %)
cargo build --profile dist -p BatchMkvMerge                  # installer build (release + thin LTO, stripped)
cargo packager --profile dist -p BatchMkvMerge -f app,dmg    # macOS; nsis,wix on Windows; deb,appimage on Linux
cargo generate-rpm --profile dist -p crates/bmm-app          # .rpm on Linux
```

Installers land in `target/packager/` (`target/generate-rpm/` for the `.rpm`). The `pnpm` / `tauri` commands of the React app no longer work.

To bump the version across `package.json`, `src-tauri/Cargo.toml`, the three `crates/*/Cargo.toml`, `src-tauri/tauri.conf.json`, and the three workflow files, edit both arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts
deno task change-version
cd ../.. && cargo update -w   # record the new workspace versions in Cargo.lock
```

## Configuration storage

The persisted config file is named `BatchMkvMerge.json` and lives at:

- **Windows** — `%APPDATA%\BatchMkvMerge\` when installed under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`; next to the `.exe` in portable mode.
- **Linux** — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- **macOS** — `~/Library/Application Support/BatchMkvMerge/`.

## Frontend ↔ Backend protocol (reference)

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
	- Emitted by the backend when a merge worker exits, or right away when a still-waiting file is cancelled (the shell forwards `bmm-core`'s `MergeEvent::Finished`).
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

Three GitHub Actions workflows build the gpui-kit app on push/PR and upload its installers under the artifact names of the Tauri releases:

- `linux_build.yml` (`ubuntu-22.04`, for a low glibc floor) — `.deb`, `.AppImage` (cargo-packager), `.rpm` (cargo-generate-rpm)
- `macos_build.yml` — `.dmg` (x86_64 on `macos-15-intel` and arm64 on `macos-15`)
- `windows_build.yml` — `.msi` (one per language) and NSIS `.exe`

Each installs the Rust toolchain pinned in `rust-toolchain.toml` (1.100.0; the setup action gets no `toolchain:` input, so nothing overrides the pin), runs `cargo test --release --workspace --locked`, builds `BatchMkvMerge` with `--profile dist` and packages it. No Node, pnpm or Deno steps. Linux also installs GPUI's build dependencies and Mesa's Vulkan drivers.

Linux additionally enforces ≥ 90 % line coverage for the `bmm-core` crate (`cargo llvm-cov --release -p bmm-core --summary-only --fail-under-lines 90`).
