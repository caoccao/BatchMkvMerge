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

The Tauri v2 + React/MUI shell is being replaced by a native [gpui-kit](https://crates.io/crates/gpui-kit) application on the `migrate-gpui-kit` branch, in phases 0-14, one Conventional Commit per phase. The maintainer keeps the plan outside the repository; `CLAUDE.md` lists the phases. Phase 12 deleted the Tauri shell (`src-tauri/`) and the React app (`src/`); the React files that code comments name as the origin of a port are in the git history before that commit. The app is `crates/bmm-app`.

## Tech Stack

### gpui-kit app

- [gpui-kit](https://crates.io/crates/gpui-kit) 0.9.1 (exact pin; GPUI with the `gpui-pre` backend), in `crates/bmm-app` (package `BatchMkvMerge`)
- [rust-i18n](https://crates.io/crates/rust-i18n) 4.2 for localization (9 locales under `crates/bmm-app/locales/`)
- `crates/bmm-platform`: per-OS integrations GPUI lacks (taskbar / Dock / launcher progress through `ITaskbarList3`, `NSDockTile`, the `com.canonical.Unity.LauncherEntry` D-Bus signal)
- Installers: cargo-packager 0.11.8 (`.app`, `.dmg`, NSIS `.exe`, WiX `.msi`, `.deb`, `.AppImage`) and cargo-generate-rpm 0.21.1 (`.rpm`)

### Backend

- Rust (edition 2024), a Cargo workspace: the UI-free `crates/bmm-core` library, `crates/bmm-platform` and `crates/bmm-app`
- Native Rust media metadata parser under `crates/bmm-core/src/media_metadata/`
- Dedicated worker threads for merge processes, which push `MergeEvent`s to the app

### External tools

- MKVToolNix (`mkvmerge`) is required for merge execution
- BetterMediaInfo is optional and enabled when configured in Settings

## Tooling

- [Rust](https://www.rust-lang.org/) 1.100.0 — pinned in `rust-toolchain.toml` at the repository root
- [cargo-packager](https://crates.io/crates/cargo-packager) 0.11.8 and [cargo-generate-rpm](https://crates.io/crates/cargo-generate-rpm) 0.21.1 — installers (`cargo install --locked --version …`)
- [cargo-llvm-cov](https://crates.io/crates/cargo-llvm-cov) — coverage gate
- [Deno](https://deno.com/) — scripts under `scripts/ts/` (e.g. `change-version.ts`)

## Repository layout

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`: Cargo workspace root (members `crates/bmm-core`, `crates/bmm-platform`, `crates/bmm-app`; one `target/`)
- `crates/bmm-app/`: the gpui-kit app (`src/`; `src/domain/` holds the UI logic without GPUI: merge trees, group cards, size and time labels, queue states, parse-error messages, numeric word navigation; `src/state/` the config, file, queue and navigation stores, with background parsing, the profile automation, the profile edits (add, delete, reset, switch) and the track tables' edits (which fan out over a group card's units); `src/merge_events.rs` the bridge from the core's merge queue; `src/notifications.rs` the toast of every finished merge and the notice sent once the queue drains; `src/actions.rs` enqueue, copy commands, cancel, delete, clear and the queue's per-drive Resume All / Cancel All / Clear Completed across stores, and BetterMediaInfo detection; `src/tools.rs` the MKVToolNix and BetterMediaInfo detection behind the Integration settings; `src/updates.rs` the startup update check and Skip this version; `src/ui/` the screens and shortcuts, with the file and group cards in `src/ui/cards/`, the output path dialog in `src/ui/output_dialog.rs`, the track table and its language and track-name cells in `src/ui/track_table/`, the Queue tab in `src/ui/queue/`, the topmost completion window in `src/ui/topmost.rs`, the Settings tab (gpui-kit's `Settings` component: pages, the language picker, the track-name editor, the tool path rows) in `src/ui/settings/`, the update banner in `src/ui/update_banner.rs`, the toolbar's group-mode and profile menus in `src/ui/toolbar.rs` and the numeric word stops of every text input in `src/ui/word_navigation.rs`; `src/platform/` window geometry and title, close guard, macOS menus, file dialogs and the taskbar progress; `src/i18n.rs` and `src/theme.rs`), its locale files (`locales/`), theme sets (`themes/`), Phase 2 prototypes (`examples/spike/`), the screenshot renderer (`examples/screenshots.rs`), embedded images (`assets/`) and installer inputs (`packaging/`: icons, desktop entry, WiX template)
- `crates/bmm-platform/src/`: taskbar / Dock / launcher progress per OS
- `crates/bmm-core/src/`: UI-free core (config, merge queue and `MergeEvent`s, services, MKVToolNix, update check, the result types of `protocol.rs`; the track rows of the selection table in `media_track.rs`, the `mkvmerge` command line in `merge_args.rs`, and the MKV language list and its lookups in `languages.rs`)
- `crates/bmm-core/src/media_metadata/`: native parser implementation
- `crates/bmm-core/tests/`: parser fixture integration tests, and `merge_args_golden.rs` with its cases in `tests/golden/` (`*.json` plus the synthetic inputs in `fixtures.rs`)
- `docs/parsers/`: parser coverage and behavior notes per format
- `scripts/ts/`: maintenance scripts (including multi-file version bumping)

## Commands

```bash
cargo run --release -p BatchMkvMerge                         # run the gpui-kit app
cargo run --release -p BatchMkvMerge --example spike         # the Phase 2 prototypes
cargo run --release -p BatchMkvMerge --example screenshots   # regenerate docs/screenshots/ (macOS)
cargo check --release --workspace                            # fast Rust type-check
cargo test --release --workspace                             # run all Rust tests
cargo llvm-cov --release -p bmm-core                         # line coverage of the core crate (CI gate: 90 %)
cargo build --profile dist -p BatchMkvMerge                  # installer build (release + thin LTO, stripped)
cargo packager --profile dist -p BatchMkvMerge -f app,dmg    # macOS; nsis,wix on Windows; deb,appimage on Linux
cargo generate-rpm --profile dist -p crates/bmm-app          # .rpm on Linux
```

Installers land in `target/packager/` (`target/generate-rpm/` for the `.rpm`).

The Linux dependencies in `crates/bmm-app/Cargo.toml` come from two sources. `dpkg-shlibdeps` on the `ubuntu-22.04` `dist` binary gives the libraries it links (`libc6 (>= 2.35)`, `libgcc-s1`, `libxcb1`, `libxkbcommon0`, `libxkbcommon-x11-0`; rpm adds the same ones by soname). The rest are what GPUI loads at run time, so no tool sees them: the Vulkan loader, the Wayland client, EGL and wayland-egl (wgpu's GL fallback), fontconfig's `/etc/fonts/fonts.conf` (read by a pure-Rust parser) and Noto Color Emoji. The `.rpm` names them as capabilities (`libvulkan.so.1()(64bit)`, …), so they resolve on Fedora and openSUSE alike. Re-derive the list after a gpui-kit bump: list the binary's `NEEDED` entries and the `lib*.so*` names in it, and run `dpkg-shlibdeps -O` in an `ubuntu:22.04` container.

The screenshots example draws the app from synthetic files (nothing is parsed or merged, the config stays in memory) and overwrites the four pictures of `docs/screenshots/`; `-- --out <dir>` writes them elsewhere, and `-- --review <dir>` renders the visual-review set instead: every locale through every tab, menu, dialog and settings page at the 600 × 450 minimum window size, and every theme in light and dark. It renders offscreen through GPUI's `render_to_image`, which only macOS implements.

The twenty colour themes in `crates/bmm-app/themes/` are derived from the React palette's main colours in `crates/bmm-app/src/theme.rs` (`THEMES`); the test `every_theme_file_is_derived_from_its_main_colour` fails when a file differs. After changing a colour, rewrite the files with:

```bash
BMM_WRITE_THEMES=1 cargo test --release -p BatchMkvMerge --lib theme::
```

gpui-kit's own strings that the app shows (the settings search and Reset All, the select placeholder and search box, the input menu, the dialog buttons) are translated in the `gpui_component` part of every locale file; gpui-kit ships none for `de`, `es`, `fr` and `ja`.

In every text field, Ctrl+Left/Right (Option on macOS), with Shift, and the word deletions treat each part of a number as a word: `Movie.2024.1080p` stops at 5, 6, 10 and 16 going right. Text without digits moves as gpui-kit's inputs do. The stops live in `crates/bmm-app/src/domain/word_navigation.rs`.

To try the app by hand on macOS or Linux without touching your settings, start it with a scratch home, because the config directory follows `$HOME` (on Linux only while `XDG_CONFIG_HOME` is unset): `HOME=/tmp/bmm-home cargo run --release -p BatchMkvMerge`.

To bump the version across the three `crates/*/Cargo.toml`, the three workflow files and the macOS `CFBundleVersion` (`crates/bmm-app/packaging/macos/Info.plist`), edit both arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts
deno task change-version
```

The script then runs `cargo update -w`, so `Cargo.lock` records the new workspace versions, and warns when `docs/release_notes.md` has no section for the new version yet.

## Configuration storage

The persisted config file is named `BatchMkvMerge.json` and lives at:

- **Windows** — `%APPDATA%\BatchMkvMerge\` when installed under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`; next to the `.exe` in portable mode.
- **Linux** — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- **macOS** — `~/Library/Application Support/BatchMkvMerge/`.

## Concurrency

- Merge scheduling is per drive:
	- each drive has at most one active `mkvmerge` worker,
	- additional items for that drive are queued FIFO.
- Different drives can merge in parallel.
- The workers push `MergeEvent`s (`Started`, `Progress`, exactly one `Finished` per file) through a channel to the app's queue store; nothing polls.

## Track-name automation

- Profiles include language and naming automation (`reset_und_language`, `set_track_name`, `reset_default_track`, `reset_forced_display`).
- The automation settings are persisted in config and applied once a unit's files are parsed (`crates/bmm-app/src/state/files/load.rs`).

## CI

Three GitHub Actions workflows build the gpui-kit app on push/PR and upload its installers under the artifact names of the Tauri releases:

- `linux_build.yml` (`ubuntu-22.04`, for a low glibc floor) — `.deb`, `.AppImage` (cargo-packager), `.rpm` (cargo-generate-rpm)
- `macos_build.yml` — `.dmg` (x86_64 on `macos-15-intel` and arm64 on `macos-15`)
- `windows_build.yml` — `.msi` (one per language) and NSIS `.exe`

Each installs the Rust toolchain pinned in `rust-toolchain.toml` (1.100.0; the setup action gets no `toolchain:` input, so nothing overrides the pin), runs `cargo test --release --workspace --locked`, builds `BatchMkvMerge` with `--profile dist` and packages it. No Node, pnpm or Deno steps. Linux also installs GPUI's build dependencies and Mesa's Vulkan drivers.

Linux additionally enforces ≥ 90 % line coverage for the `bmm-core` crate (`cargo llvm-cov --release -p bmm-core --summary-only --fail-under-lines 90`).
