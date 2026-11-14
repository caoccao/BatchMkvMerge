# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

The Tauri v2 + React/MUI shell is being replaced by a native [gpui-kit](https://crates.io/crates/gpui-kit) application on the `migrate-gpui-kit` branch. The maintainer keeps the migration plan (phases 0-14) outside the repository. Each phase lands as one Conventional Commit on that branch, so `git log` shows which phases are done. The work ends with a pull request into `main`; the branch creates no tags and no releases.

The Tauri app is no longer built, run or tested. `src/` (React) and `src-tauri/` stay in the tree as unbuilt porting reference until Phase 12 deletes them; `src-tauri` is excluded from the Cargo workspace. The architecture sections further below describe that reference, not the new app. The new app is `crates/bmm-app` (package `BatchMkvMerge`), described in "gpui-kit app" below.

| Phase | Content |
| --- | --- |
| 0 | Ground rules (the Build rules above), branch |
| 1 | Cargo workspace, `crates/bmm-core` extraction, `MergeEvent` sink |
| 2 | Spike: packaged skeleton on CI, four platform prototypes, upgrade identities of the last Tauri release (go/no-go gate) |
| 3 | Domain logic ported to Rust with tests |
| 4 | Application shell |
| 5 | State entities, metadata pipeline, merge events |
| 6 | Track table and autocomplete cells |
| 7 | Cards and file list |
| 8 | Queue, notifications, taskbar progress |
| 9 | Settings and toolbar menus |
| 10 | Themes, i18n polish, visual pass |
| 11 | Numeric word-boundary navigation |
| 12 | Parity audit, Tauri removal |
| 13 | Final packaging, CI, upgrade tests, version 1.0.0 |
| 14 | Docs, cross-platform QA, pull request |

## Common commands

```bash
cargo run --release -p BatchMkvMerge                   # run the gpui-kit app
cargo run --release -p BatchMkvMerge --example spike   # the Phase 2 prototypes
cargo check --release --workspace                      # fast Rust type-check without linking
cargo test --release --workspace                       # run the Rust test suite (includes the spike's tests)
cargo build --profile dist -p BatchMkvMerge            # installer build: release + thin LTO
cargo packager --profile dist -p BatchMkvMerge -f app,dmg   # or nsis,wix / deb,appimage (cargo-packager 0.11.8)
cargo generate-rpm --profile dist -p crates/bmm-app    # .rpm on Linux (cargo-generate-rpm 0.21.1)
```

The `pnpm` / `tauri` commands of the React app no longer work: `src-tauri` is not a workspace member.

Version bumps are done with a Deno script — edit the two arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts && deno task change-version
```

That rewrites `package.json`, `src-tauri/Cargo.toml`, `crates/bmm-app/Cargo.toml`, `crates/bmm-core/Cargo.toml`, `crates/bmm-platform/Cargo.toml`, `src-tauri/tauri.conf.json`, and all three `.github/workflows/*.yml` files. Run `cargo update -w` afterwards so `Cargo.lock` records the new workspace versions.

### Rust toolchain

The toolchain is pinned in `rust-toolchain.toml` at the repository root (currently **1.100.0**). The pin must stay at 1.91 or newer for `specta-2.0.0-rc.25`, which relies on `debug_closure_helpers` (stable since 1.91); the gpui-kit migration needs 1.92 or newer. The GitHub Actions workflows use `actions-rust-lang/setup-rust-toolchain@v1` **without** a `toolchain:` input, so the action installs the pinned toolchain; with `toolchain: stable` it would also run `rustup override set stable` on the repository root, and that directory override outranks the toolchain file. They also clear `rustflags` so the action's default `-D warnings` doesn't fail CI on intentionally-not-yet-consumed parser surface during phased delivery.

### Tests

The Rust code is a Cargo workspace (root `Cargo.toml`, one `Cargo.lock`, one `target/`) with three members: `crates/bmm-core` holds everything UI-free — the parser sub-tree `crates/bmm-core/src/media_metadata/`, config, merge queue, services, MKVToolNix, update check, and the track rows, merge arguments and language lookups ported from the React app; `crates/bmm-platform` holds per-OS code GPUI lacks (taskbar / Dock / launcher progress); `crates/bmm-app` is the gpui-kit application. `src-tauri` is excluded from the workspace. Unit tests are inline (`#[cfg(test)] mod tests {}`); the parser fixture tests are `crates/bmm-core/tests/*_fixtures.rs`; the spike example's tests run too (`test = true` on the `[[example]]`). The golden test `crates/bmm-core/tests/merge_args_golden.rs` parses the synthetic files of `tests/golden/fixtures.rs` and checks the rows, `mkvmerge` arguments and copyable command of every `tests/golden/*.json` case; the expected values were produced by the React app's own `metadataToMediaTracks` / `buildMergeArgs(Multi)` before it was retired, so change them only when the merge command is meant to change. `cargo test --release --workspace` runs all of them. Coverage is measured with `cargo-llvm-cov`: the whole `bmm-core` crate is held to ≥ 90 % line coverage, enforced by the Linux CI workflow (`.github/workflows/linux_build.yml`) via `cargo llvm-cov --release -p bmm-core --summary-only --fail-under-lines 90`. Scoping by package keeps the UI crates out and every new core file in, with no file-name exclusion list; local baseline is ~93 % for the crate (~96 % for the parser alone). `merge.rs` is tested through a fake process launcher (`MergeQueue::new`), never a real `mkvmerge`.

The TypeScript drift check `src-tauri/tests/protocol_typescript.rs` no longer runs (its crate is not built), so `src/protocol.generated.ts` is frozen; both go away in Phase 12. The model structs still derive specta until then.

### gpui-kit app (`crates/bmm-app`)

- Package `BatchMkvMerge`: a library `batch_mkvmerge` with all code and tests, and a three-line binary that calls `batch_mkvmerge::run()` (`#![windows_subsystem = "windows"]` unconditionally, there is no debug build). The app identity lives in `lib.rs` constants: `APP_IDENTIFIER` (`com.caoccao.batch-mkvmerge`), `DESKTOP_ID` (`BatchMkvMerge`, the Linux desktop file basename, Wayland app id and launcher-progress URI).
- `gpui-kit` is an exact pin (`=0.9.1`). Its `test-support` feature is a dev-dependency only, so it never reaches the shipped binary. Because Cargo resolves weak `dep?/feature` entries into the lockfile, `test-support` makes `Cargo.lock` carry the `gpui-fast*` crates and their exact pins; they are never compiled. If a resolution conflict appears after a gpui-kit bump, re-resolve the gpui graph in one go instead of downgrading crates one by one.
- Icons: `gpui_kit::assets::IconName` is the full Lucide catalog; `gpui_kit::component::IconName` is a small legacy subset. Only 106 SVGs are embedded by default; `src/assets.rs` adds the others the UI uses through `icon_assets!` and serves the app's own images (`images/icon.png`).
- UI tests use `#[gpui_kit::test]` and `gpui_kit::test::TestWindowExt` (`click`, `drag`, `input`, `press`, `find`). Mark elements a test must find with `.test_support()` (`gpui_kit::base::TestSupportExt`; a no-op without the feature). In a test module that sees `gpui_kit::*`, add `use core::prelude::v1::test;`, because the glob also exports GPUI's `test` macro and `#[test]` would recurse. GPUI delivers focus events only to an active window and through deferred effects: call `window.activate_window()` once and `cx.run_until_parked()` after a click before typing.
- GPUI starts its own drag after a hard-coded 2 px and then drops the click. Row reordering with dnd-kit's 5 px activation uses a pointer state machine instead (`examples/spike/reorder.rs`).
- An `absolute()` element needs explicit insets (`top_0().left_0()`); without them taffy places it at its static position after its siblings.
- `KeyBinding::new` cannot parse `secondary-…` (it needs the OS); bind `cmd-…` / `ctrl-…` per platform.
- Ported logic (Phase 3): `bmm-core` has `media_track` (the selection table's rows from a parsed file), `merge_args` (the `mkvmerge` command line, profile auto-selection, path helpers) and `languages` (the MKV language list, alias and short-code lookups, autocomplete options); `bmm-app::domain` has `file_tree` (merge trees, manual merges, detaches, combined track order, flag automation), `grouping` (group-card keys), `format`, `status` (queue item states from `MergeEvent`s), `metadata_error` and `word_navigation`. They reproduce the React app's behaviour, but numbers, strings and rounding follow Rust, not JavaScript: `str::parse::<f64>`, `format!("{:.*}")` (exact ties round to even: 1.125 → `1.12`), byte-wise string order and length, `char::is_whitespace`. A combined track id joins file and key with a NUL, as the React app did. Word navigation works on UTF-8 byte offsets.
- Application shell (Phase 4): `lib.rs::run` loads the config, sets the locale (`i18n.rs`, `t!` through the crate-local `tr!` macro that returns a `SharedString`), registers the theme sets of `themes/` (`theme.rs`; only Ocean so far, every theme falls back to it), and opens the main window (`platform/window.rs`). `state/` holds the stores handed to every view as `Stores`: `ConfigStore` (optimistic edits, background writes with a revision so an older write never lands last, a synchronous flush on quit), `FileStore` and `UiState` (tabs, toasts). `ui/` has one module per React screen (`root` = `Layout.tsx`, `toolbar`, `main_content`, `file_list`, `welcome`, `about`, `footer`) and `actions.rs` (shortcuts, global handlers, so they work without focus). `platform/` has the window geometry, the close guard, the macOS menus and the file dialogs.
- State and merges (Phase 5): `Stores::new` takes `Services` (config persistence, a `MetadataSource` for parsing, a `MergeBackend` for enqueue/cancel), so tests pass fixtures and recorders. `FileStore` (`state/files/`) holds the files, parsed metadata and table rows, selections, row edits, output paths, manual merges and detaches, and derives the card units and the file list's entries after every change; `load.rs` parses every file once on the background executor (in-flight guard) and runs the profile's automation once per unit membership. `QueueStore` (`state/queue.rs`) is driven only by `MergeEvent`s and ticks every second while a merge runs. `merge_events.rs` bridges the core's worker threads to the queue store through an `async-channel`; `actions.rs` (crate root) holds what spans stores: `MergeRequest` (read on the foreground, output path and arguments resolved on the background executor, enqueued in order), cancel, clear, remove. A store notifies only on structural changes (files or queue items added or removed, a status, units or groups, the first or last selection); every change is also emitted as `FileEvent::Changed` / `QueueEvent::ItemChanged` naming the files, for per-card repaints.
- Track table (Phase 6): `ui/track_table/` is the table of one card (`TrackTable`, built on gpui-base's interactive `Table` primitives, because gpui-kit's composable `Table` rows take no mouse handlers) with the language and track-name cells (`autocomplete.rs`, `AutocompleteCell`: every option listed, preferred languages first with a divider, the match highlighted and centred, ↓/↑/Enter/Escape as MUI's Autocomplete, an alias such as `ger` normalised to `de` on Enter). `state/files/table.rs` holds `TableScope` (a single card's unit, or a group card's units whose edits fan out to the same member position) and the `FileStore` edits behind each control. The tables are kept by the card's first root so that typing (which can change a group key) keeps the views. Row selection, the keyboard cursor and the drag state live in the table view; a click in a card makes it the active card. The table keys (↑/↓, Space, Ctrl/Cmd+A, Ctrl/Cmd+Shift+A) go through a keystroke interceptor on the file list: it acts on the active card's table on the File List tab, unless a text field has the focus or another binding claims the key, and Space only when nothing or that table has the focus (a focused button or checkbox takes Space). Row drag-reorder is the Phase 2 pointer state machine (5 px activation).
- Cards (Phase 7): `ui/cards/` holds `FileCard` (`MkvFileCard.tsx`: status icon, root path with the child-count badge whose popover lists the other members with a remove button, the summary, Set Output Path, Open in BetterMediaInfo, Copy Command, Merge, Delete, the drag handle, the progress row with `elapsed / ETA` and Cancel while active, the track table) and `GroupCard` (`GroupCard.tsx`: the directory and summary, output directory, BetterMediaInfo, Copy All Commands, Merge All, Clear All, the roots with status and progress on the left of a 6 px splitter, min 160 px, at least 240 px left for the table). `ui/output_dialog.rs` is `OutputPathDialog` (directory or file mode, 300 ms debounced "will be created" warning, writable check on OK; also the toolbar's global output path). The file list keeps cards and tables by the card's first root, makes a card again around the same table when it changes kind, and hit-tests a card drag (`CardDrag`, GPUI `on_drag` from the handle with a file-name chip) against the cards' bounds: a file card other than the source is the drop target (copy cursor, dashed outline), a group card or the source refuses it. Card actions live in `actions.rs` (`MergeRequest::group_unit`, `MergeRequest::command`, `copy_commands`, `delete_unit`, `clear_units`, `open_in_better_media_info`, `watch_better_media_info`); Copy Command and the dialog's default path use exactly the output Merge writes. Card messages are bottom-centre toasts (`UiState::show_snackbar`). The encoded date is shown in local time as `2026-05-24 09:00:00` (`format::format_encoded_date`).
- Cached cards (Phase 7): the file list draws each card with `Entity::cached` at the height the card measured in its last render (`CardHeight`, a canvas probe that asks for one more frame when the height changes), so a progress event or tick re-renders only that card. A cached view is laid out from the given style, never measured from its content, and it re-renders only when it or a descendant view is notified, when the window refreshes, or when its bounds move: a card growing a progress row moves the cards below it, which then render once. Reading a store during render does not invalidate a cached view; cards subscribe to the events of their own files. Every interactive element still works inside a cached view (hover and tooltips notify the view). `TestWindowExt::render_frame` refreshes the window and so bypasses caching; a test that counts renders draws with `window.draw(cx).clear(cx)`. Text components record their text for tests (`gpui_kit::base::TestTextExt::test_text`, read back with `gpui_kit::base::test_support::texts(window)`); the cards mark their texts so tests can read what is shown.
- Queue and notifications (Phase 8): `ui/queue/` is the Queue tab (`Queue.tsx`): one card per drive key titled with the drive (`C:`, `\\SERVER\SHARE`; "Queue" elsewhere and for the empty card), the System Notification and Topmost Notification toggles (session-only, in `UiState`), Resume All / Cancel All / Clear Completed (`actions::resume_drive`, `cancel_drive`, `clear_completed_in_drive`), and a table whose rows are `QueueRow` entities drawn as cached views at their measured height (`CardHeight`), so only the merging row repaints; a failed status shows the error as a tooltip; below 200 px for File Path the table scrolls sideways. `QueueStore` tracks a batch from the moment an item waits or merges until none does and emits `QueueEvent::Drained(QueueBatch)` once. `notifications.rs` shows the per-file toast (`QueueEvent::Finished`) and turns a drained batch into a `CompletionNotice`: **the OS notification and the topmost window are sent once per drained batch, not per file** (a fix over React), only when a merge of the batch completed or failed, never while the window closes (`UiState::set_closing`, set by the exit dialog's Yes). A batch of one completed file keeps React's text (the file, "Completed in"); any other shows the counts and "Finished in" from the first start to the last end. The root view delivers it: `Notification::…system()` (OS centre only; dropped silently by an unbundled `cargo run` on macOS) and `ui/topmost.rs` (`TopmostWindow`: a 440×200 `PopUp` window centred on the main window's display work area, reused while open, Escape/Enter/Close close it; on Wayland an overlay layer-shell surface, falling back to `PopUp` without layer-shell). The app quits when the main window closes, even with the topmost window open. The taskbar / Dock / launcher progress is `platform::taskbar::Taskbar` (a service in `Services`, `bmm_platform::TaskbarProgress` in the app, a recorder in tests), set from `QueueSummary` on every queue change while a merge is active and hidden otherwise. Tests read system notifications with `TestAppContext::shown_system_notifications()`, which records only after `cx.set_app_identity(...)`.
- GPUI findings (Phase 7): a gpui-kit dialog binds Enter to its Confirm action, which closes it without calling a form's own handler: route `Dialog::on_ok` to the form and return `false` until the form closes the dialog itself. A disabled gpui-kit button keeps an icon colour set with `Icon::text_color`, so a coloured icon is applied only while the button is enabled.
- GPUI findings (Phase 6): a key binding beats an element's key-down listeners and text input, so a plain `space` binding would steal Space from focused buttons and from text fields; the interceptor (`cx.intercept_keystrokes`) sees the context stack and the window first and consumes only what it handles. GPUI does not draw before dispatching a mouse event, so listeners registered in a paint (`window.on_mouse_event`) miss a release that comes before the next frame; the table also listens with `on_mouse_up` / `on_mouse_up_out`. A scroll container without `restrict_scroll_to_axis()` turns a wheel delta of the other axis into its own (a vertical wheel over a sideways-scrolling table slides it). `InputState::set_value` emits no `Change`, and a focused cell is never overwritten from the store: its own edits arrive only after the keystroke's effects run. gpui-kit's `Scrollable` installs its own scroll handle; add a scrollbar to a tracked handle with `ScrollableElement::vertical_scrollbar(&handle)` instead. gpui-kit's `Checkbox` has no indeterminate state; gpui-base's has (and reports it as mixed).
- An action dispatched by a key press runs while its window is being updated, so a global handler cannot `update` that window: defer with `cx.defer` (the Quit handler does). gpui-kit dialogs bind Enter to their OK button; the exit dialog has its own footer so Enter and Escape answer No, as in the React app. GPUI's test scheduler panics when another thread wakes a foreground task: a test that runs the core's merge queue relays the worker events through the test thread.
- Window geometry: GPUI's macOS window opens with a *content* size but reports its *frame* (title bar included), so the saved size is `window.viewport_size()`; a maximized or fullscreen window is not saved, so its restore geometry survives. Saves are debounced 500 ms and repeated on close and Quit.
- UI test queries see only elements marked `.test_support()` and gpui-kit's own components; icon-only buttons need `.accessibility_label(...)` (the tooltip text) to be found by label, which also names them for screen readers. The rust-i18n locale is process-wide: a test that renders the shell or switches the language holds `i18n::tests::LOCALE_LOCK` (the shell test helper does), or text width changes under another test's click. `Config::default()` detects the system language; tests set `language` explicitly.
- `ButtonCustomVariant` paints its colour at 20 % strength in gpui-kit 0.9.1; a solid button adds `.bg(...)`.
- Packaging metadata is in `crates/bmm-app/Cargo.toml` (`[package.metadata.packager]`, `[package.metadata.generate-rpm]`). Every installer identity matches the 0.1.0 Tauri release: deb and rpm package `batch-mkv-merge`, bundle id, NSIS per-user install with publisher `caoccao`, and the MSI UpgradeCode, which `packaging/wix/main.wxs` pins because cargo-packager would derive a different one. cargo-packager rejects unknown keys (NSIS takes `installMode`, not `install-mode`).

## Platform-specific notes

- Development here happens on Windows with Git Bash. Use forward-slash paths (`C:/…` not `C:\\…`), `/dev/null` not `NUL`, and avoid `cd <project>` before `git` commands — git already runs against the working tree and prepending `cd` triggers an extra permission prompt.
- MKVToolNix (`mkvmerge`, `mkvmerge`) must be installed separately — the app shells out to them.

## Architecture

### Frontend ↔ Backend split

React app under `src/` talks to a Rust Tauri v2 backend: the thin shell under `src-tauri/` (commands, window, notifications) on top of the UI-free `crates/bmm-core` library. All communication goes through Tauri's `invoke` (commands) and the event system — no other IPC. Everything crossing the boundary is JSON. The parser model lives in `crates/bmm-core/src/media_metadata/model/` and was mirrored into `src/protocol.generated.ts` via specta by `src-tauri/tests/protocol_typescript.rs`, which is no longer built (the file is frozen); `src/protocol.ts` re-exports those types and adds UI-only ones (`MkvToolNixStatus`, `BetterMediaInfoStatus`, `QueueItemStatus`, `MediaMetadataError`, …). The hand-written `crates/bmm-core/src/protocol.rs` only carries the non-parser wire types (config status DTOs, merge events, the structured `MediaMetadataErrorPayload`).

The Tauri command surface for files / metadata is:

- `get_media_files(paths)` — recursive directory walk filtered by `media_metadata::probe::extension_hint::is_supported_media_path`.
- `get_media_metadata(file)` — wraps `media_metadata::parse` with `ParseOptions` built by `services::parser_options_from_config` (`crates/bmm-core/src/services.rs`). The persisted `config.parser.timeoutMs` wins over the legacy `BMM_PARSER_BUDGET_MS` env override; env only applies while the config still holds the default timeout. The timeout is no longer exposed in the UI — edit the config file to change it. Failures arrive on the frontend as a tagged `MediaMetadataError` (`io | unexpectedEof | unrecognised | timeout | malformed | oversizedElement | internal`); `MkvFileCard.formatMetadataError` maps each kind to a `merge.error.parser.*` i18n key.

The old `get_mkv_files` / `get_mkv_tracks` commands and the `MkvTrack` wire struct were removed in Phase 11; the MKV-only `is_mkv` predicate is replaced by `is_supported_media_path` (Phase 11), so the drag-drop accept-list now covers every extension `mkvmerge -J` recognises. It also accepts a VobSub `.sub` data file directly when the sibling `.idx` manifest is present, matching mkvtoolnix's path-aware VobSub open flow.

### Single source of truth: Zustand store

`src/store.ts` owns:
- the dropped file list,
- the merge queue (grouped per-drive on Windows — single bucket on Linux/macOS), with per-item status, progress, timestamps, cancel flag, and error,
- the persisted config (theme, language, profiles, mkvtoolnix path, window geometry),
- a registry of per-card merge handlers and selection flags that the Toolbar consumes for Merge All,
- per-file parser results in two parallel maps: `fileMetadata: Record<string, MediaMetadata>` (the raw parser output, kept for future detail panels) and `fileTracks: Record<string, MediaTrack[]>` (the flattened rows the selection table renders, derived by `metadataToMediaTracks` in `src/media-metadata.ts`). `setFileMetadata` writes both maps and the per-kind count summary in one shot. The `MediaTrack` adapter shape, the `MediaTrackType` row-kind enum (`"track" | "chapters" | "attachment"`), and the `mediaTrackCounts` helper all live in `src/media-metadata.ts` — keep new UI-row logic there, not in `src/merge.ts`.

**Important:** `updateConfig` applies the patch optimistically and discards the backend's response. Don't re-add a post-await `set({ config: saved })` — it re-introduces a race condition where rapid typing reverts edits (older async responses land on top of newer optimistic state). The backend also doesn't transform the config inside `set_config`, so the response would be identical anyway.

### Status state machine

`QueueItemStatus` is a string enum in `src/protocol.ts`: `Waiting | Merging | Completed | Cancelled | Failed`. Backend snapshots only ever carry `Waiting` / `Merging`; terminal states are set on the frontend:

1. The backend emits a `merge-finished` event when the worker exits (or right away when a still-waiting file is cancelled), carrying the authoritative outcome (`Completed`, `Cancelled`, or `Failed`) — `FileList.tsx` listens and calls `recordFinishedOutcome` which sets the status directly.
2. As a fallback, `applyMergeSnapshot` transitions any item that's disappeared from the backend snapshot to `Completed` (or `Cancelled` if `cancelRequested` was flagged). This handles races where the event arrives after the next poll.
3. The terminal-status check in `applyMergeSnapshot` skips items already in a terminal state, so the event-driven update is never overwritten.

Polling runs every **200 ms** from `FileList.tsx` via `get_merge_status`.

### Per-drive merge queue (backend)

`crates/bmm-core/src/merge.rs` owns all live merge state in a `MergeQueue`. One process-wide instance sits behind the free functions `enqueue` / `cancel` / `snapshot`; tests build their own with a fake launcher.

- `tasks: HashMap<file, TaskState>` — metadata (status, progress, args, cancel flag).
- `drives: HashMap<drive_key, DriveState>` — per-drive `merging + queued`.
- `children: HashMap<file, Arc<Mutex<Box<dyn MergeProcessControl>>>>` — kept separately so `cancel()` can `kill()` from a different call site.

`get_drive_key` returns the Windows path `Prefix` component (`C:`, `\\server\share`) uppercased, else `"default"`. When a file is enqueued and the drive's `merging` slot is free, it's promoted immediately and a worker is `std::thread::spawn`'d. When a worker finishes, `finish` sends `MergeEvent::Finished`, drops the task, and pops the next file for the same drive.

Workers push `MergeEvent`s into the sink installed with `merge::init`: `Started`, `Progress` (only when the percentage changes), and `Finished` with `Completed | Cancelled | Failed`. Every enqueued file gets exactly one `Finished` — also a file cancelled while still waiting, and a worker that panics (`Failed`). The Tauri shell's sink (`src-tauri/src/application.rs`) forwards only `Finished`, as the `merge-finished` event; the frontend still polls `get_merge_status` for progress.

The tokio runtime is intentionally capped at 4 workers (see `src-tauri/src/application.rs::configure_runtime` → `tauri::async_runtime::set(runtime.handle().clone())` with `worker_threads(4)`). The actual merge work runs on dedicated `std::thread::spawn` threads, not on those 4 workers, so the polling endpoint stays responsive under load.

### Profiles

Profiles live in the persisted config (one `ConfigProfile` per entry, with a shared `activeProfile` pointer). Each profile carries per-track-type auto-select flags plus language filters (video / audio / subtitle). The `Default` profile auto-selects subtitle tracks. (The per-stream filename templates were removed — the app merges files rather than naming per-track outputs.)

The active profile is consumed both on track auto-select (once per card, guarded by `autoSelectedRef`) and at merge time (passed into `buildMergeArgs` / `buildCommandString`).

### Window flicker

`tauri.conf.json` creates the main window with `visible: false`. The setup hook in `src-tauri/src/application.rs` applies the stored position and size, then calls `window.show()` — this is why users don't see a default-geometry flash on startup. Don't flip `visible` back on.

### Config file location

Config is stored per-OS:
- Windows — `%APPDATA%\BatchMkvMerge\` in installed mode (exe under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`), next to the `.exe` in portable mode.
- Linux — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- macOS — `~/Library/Application Support/BatchMkvMerge/`.

Detection lives in `crates/bmm-core/src/config.rs::get_config_dir`. Old config files from earlier schema versions still load because every new field has `#[serde(default = "...")]`. Notable nested blocks:

- `config.externalTools` — MKVToolNix + BetterMediaInfo paths.
- `config.parser.timeoutMs` — per-file parse budget for the native parser (default 10000, clamped to 100–60000 ms by `ConfigParser::effective_timeout_ms`). Not exposed in the UI — edit the config file to change it.

### i18n

Nine locales (`de`, `en-US`, `es`, `fr`, `it`, `ja`, `zh-CN`, `zh-HK`, `zh-TW`). The gpui-kit app reads `crates/bmm-app/locales/*.json` (rust-i18n, nested keys, `%{name}` placeholders, converted once from the React files); `src/i18n/locales/` is frozen reference. When adding a user-facing string, add the key to **all nine** app files: the test `i18n::tests::every_locale_has_the_same_keys_and_placeholders_as_en_us` fails otherwise. A missing key would fall back to `en-US`, but that's not a design pattern to lean on.

### Native media-metadata parser

A pure-Rust header-only parser lives under `crates/bmm-core/src/media_metadata/`. It has fully replaced the `mkvmerge -J` subprocess shellout, the drag-drop filter is broadened to every extension `mkvmerge -J` recognises plus path-aware VobSub `.sub` inputs, and CI gates parser line coverage at ≥ 90 %. Delivery was split into 12 phases (Phase 1 = io/error/deadline foundations, Phase 2 = model + codec/language tables + Settings UI, Phase 3 = matroska reader + probe foundation, Phase 4 = MP4/QuickTime reader, Phase 5 = AVI + Ogg/OGM readers, Phase 6 = MPEG-TS + MPEG-PS readers, Phase 7 = 10 audio readers + CoreAudio, Phase 8 = elementary video streams (AVC + HEVC + AV1 OBU + MPEG + VC-1 + Dirac + DV), Phases 9-10 = subtitles + residuals, Phase 11 = Tauri command + frontend migration, Phase 12 = i18n widening + CI coverage gate). Each phase landed as one Conventional Commits commit on the `implement-parser` branch.

Layout (one module tree per format family — every file under 1000 LOC):

```
crates/bmm-core/src/media_metadata/
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
├── ogg/                # native Ogg/OGM reader (page, codecs/*, comments, identify, reader)
├── mpeg_ts/            # native MPEG-TS reader (packet, pat, pmt, pes, descriptors/*, stream_table, identify, reader)
├── mpeg_ps/            # native MPEG-PS reader (packet, pes, stream_map, identify, reader)
├── mpls/               # path-aware Blu-ray playlist parser + referenced M2TS aggregation
├── audio/              # audio-only readers (id3v2, mp3, aac, ac3, dts, flac, wav, truehd, tta, wavpack)
├── coreaudio/          # native CoreAudio CAF reader (caf, reader)
├── elementary/         # elementary video stream readers (avc/*, hevc/*, mpeg_video, vc1, dirac, dv, obu)
├── subtitles/          # subtitle readers (srt, ssa, webvtt, usf, microdvd, vobsub, pgs, hdmv_textst, vobbtn, encoding)
├── flv/                # Flash Video reader (header, tag, script_data AMF, reader)
├── realmedia/          # RealMedia reader (chunks, stream_props, reader)
└── ivf.rs              # IVF reader (AV1 / VP8 / VP9)
```

**Probe registry:** `probe::dispatch` walks `probe::registered_readers()` in priority order, calling `Reader::probe` on each. The first reader that claims the file is handed `read_headers`. Adding a new format reader is a one-line insert at the right priority level (see `probe/dispatch.rs::registered_readers`). The registry currently contains Matroska + AVI + Ogg + MP4 + MPEG-PS + MPEG-TS + FLV + RealMedia + IVF + magic-byte audio readers (FLAC / WAV / WAVPACK / TTA / CoreAudio / TrueHD) + elementary video readers (MPEG / VC-1 / Dirac / DV / AVC / HEVC / AV1 OBU) + subtitle readers (PGS / HDMV TextST / VobButton / VobSub / WebVTT / USF / SSA-ASS / SRT / MicroDVD) + frame-sync audio readers (AC-3 / DTS / MP3 / AAC). Path-dependent MPLS aggregation and VobSub `.sub` → sibling `.idx` resolution run immediately before this content-probe registry. FLV (`FLV` magic) / RealMedia (`.RMF` magic) / IVF (`DKIF` magic) slot in directly after the major containers so their unambiguous prefixes claim before any frame-sync probe. Subtitle readers fall between elementary-video and frame-sync-audio so unambiguous magic (PGS `PG`, HDMV `TextST`, VobButton `butonDVD`, VobSub `# VobSub index file, v…`) claims before frame-sync probes; text-based formats probe last so binary frames whose decoded UTF-8 happens to contain a timecode-shaped line don't false-positive.

**Matroska reader:** pure-Rust port of `mkvtoolnix/src/input/r_matroska.cpp` — no libebml/libmatroska dependency. The EBML walker (`matroska/ebml.rs`) is iterator-based (callers maintain their own container stack, so user-controlled nesting depth never blows the stack). All element IDs are in `matroska/ids.rs`. SeekHead-based dispatch mirrors mkvtoolnix's `m_deferred_l1_positions` bookkeeping. Cluster payloads are never entered — header-only.

**MP4 reader:** pure-Rust port of `mkvtoolnix/src/input/r_qtmp4.cpp` — header-only walk of the ISO BMFF / QuickTime box hierarchy. Supports 32-bit, 64-bit large-size, and size=0 (to-EOF) box forms. `ftyp` classifies QuickTime (`qt  `) vs MP4 brands into `ContainerFormat`; `moov` drives `mvhd` + per-`trak` walks (`tkhd`, `trak/udta/name`, `mdia → mdhd / hdlr / minf → stbl → stsd / stts`, `edts/elst`). The `hdlr` component name is retained only as handler metadata; it is not exposed as a user-facing track name. Codec-specific sub-boxes (`avcC`, `hvcC`, `esds`, `colr`, `pasp`, `dvcC` / `dvvC`) populate `VideoCodecConfig` / `AudioCodecConfig`. iTunes metadata (`udta → meta → ilst`) feeds container title / muxing app / date_utc; unknown tags land in `tags.global`. Fragmented MP4 (`mvex/trex` + `moof/traf/tfhd/trun`) sets `is_fragmented` and aggregates fragment sample counts into `num_index_entries`. Cluster-equivalent `mdat` payloads are never read.

**AVI reader:** pure-Rust port of `mkvtoolnix/src/input/r_avi.cpp` — walks the RIFF chunk hierarchy via a hand-rolled chunk walker (no `avilib` dependency). `RIFF/AVI ` is the entry point; `LIST/hdrl` hosts `avih` (MainAVIHeader → frame interval, total frames, dimensions, flags) and one `LIST/strl` per stream containing `strh` (kind + codec FOURCC + timebase) and `strf` (`BITMAPINFOHEADER` for video, `WAVEFORMATEX(TENSIBLE)` for audio). ODML's `LIST/odml/dmlh` provides the 32-bit total-frame count for files > 2 GB. Negative `BITMAPINFOHEADER` heights (top-down DIB) are flipped positive; WAVEFORMATEX `extra` bytes become `codec_private`.

**Ogg / OGM reader:** pure-Rust port of `mkvtoolnix/src/input/r_ogm.cpp`. Walks pages per RFC 3533 (no `ogg` crate dependency) — extracts `bitstream_serial`, `granule_position`, segment-table → packet boundaries. The first packet of each Beginning-Of-Stream page is fed to the codec sniffers under `ogg/codecs/`: Vorbis (`\x01vorbis`), Opus (`OpusHead`), Theora (`\x80theora` + KEYFRAME_GRANULE), FLAC-in-Ogg (`\x7FFLAC` + STREAMINFO), Speex (`Speex   ` 8-byte signature), Kate (`\x80kate\0\0\0`), and OGM legacy stream headers (`\x01video/audio/text...`). VorbisComment blocks on the second packet populate per-track tags + the container's `muxing_app`. Stops once every stream has a comment block to keep identification fast for huge files.

**MPEG-TS reader:** pure-Rust port of `mkvtoolnix/src/input/r_mpeg_ts.cpp`. Detects packet size by scanning the first 8 KB for the 0x47 sync byte at strides of 188 (standard), 192 (BD M2TS — 4-byte timecode prefix per packet) or 204 (FEC-extended). Reassembles PSI sections from `payload_unit_start=1` packets, parses PAT (PID 0) → list of `(program_number, pmt_pid)`, then PMT → per-stream `(stream_type, elementary_pid, descriptors)`. Eight descriptor decoders (ISO-639 language, AC-3, E-AC-3, DTS, teletext with BCD page number, HEVC, Dolby Vision profile, DVB service) enrich each stream. A canonical `stream_type → codec_id` table mirrors mkvtoolnix's `determine_codec_from_stream_type`; private-PES streams (0x06) are promoted to AC-3 / E-AC-3 / DTS / Teletext via descriptors. Multi-program TS files surface each program (with track-ID list + DVB service name) in `container.properties.programs`.

**MPEG-PS reader:** pure-Rust port of `mkvtoolnix/src/input/r_mpeg_ps.cpp`. Identifies the file via the pack-header start code (`0x000001BA`), then walks subsequent start codes collecting unique `stream_id` values. Audio range `0xC0..=0xDF` and video range `0xE0..=0xEF` produce one track per stream id; the private-stream-1 marker (`0xBD`) is classified as AC-3 (DVD-VOB convention). The `program_stream_map` decoder (`0xBC`) is available for files that carry one, though identification falls back to start-code enumeration when absent.

**Audio readers:** ten header-only ports under `audio/` and `coreaudio/`.

- **Magic-byte formats** sniff a fixed 4-8 byte signature at offset 0:
  - `flac.rs` — `fLaC` + STREAMINFO + VorbisComment (port of `r_flac.cpp`).
  - `wav.rs` — `RIFF/WAVE` and `RF64/WAVE` (`ds64` for >4 GB) + WAVEFORMATEX(TENSIBLE) (port of `r_wav.cpp`).
  - `wavpack.rs` — `wvpk` v4 frame header → sample rate index + bit-depth flags (port of `r_wavpack.cpp`).
  - `tta.rs` — `TTA1` 22-byte stream header (port of `r_tta.cpp`).
  - `truehd.rs` — Dolby TrueHD / MLP major-sync `F8 72 6F BB/BA` (port of `r_truehd.cpp`).
  - `coreaudio/caf.rs` + `reader.rs` — CAF `caff` magic + `desc/data` chunks (port of `r_coreaudio.cpp`).
- **Frame-sync formats** require 8 consecutive valid frame headers to confirm:
  - `mp3.rs` — MPEG-1/2/2.5 Layer III frame header (`0xFFE` sync) (port of `r_mp3.cpp`).
  - `aac.rs` — ADTS `0xFFF` sync + profile / sample-rate-index / channel-config (port of `r_aac.cpp`).
  - `ac3.rs` — AC-3 `0x0B77` sync + bsid byte switches between AC-3 (≤10) and E-AC-3 (≥11) decode paths (port of `r_ac3.cpp`).
  - `dts.rs` — four sync words (16-bit BE/LE + 14-bit BE/LE) + DTS-HD extension `0x64582025` (port of `r_dts.cpp`).
- **Shared:** `audio/id3v2.rs` skips ID3v2 headers + footers before the frame-sync scan; `payload_bounds` also strips ID3v1 trailers from the tail.

**Elementary video readers:** seven raw-bit-stream readers under `elementary/` that probe + decode without any container.

- **AVC/H.264** (`elementary/avc/`) — Annex B NAL walker with 3-/4-byte start-code detection + emulation-prevention-byte removal.  SPS decoder (port of `r_avc.cpp`) reads profile_idc, level_idc, chroma_format_idc, bit_depth_luma/chroma, pic_width/height, frame_mbs_only flag, and conformance-window cropping → `VideoCodecConfig` populated with profile name, level name (decimal-decoded), coded dimensions and display dimensions.
- **HEVC/H.265** (`elementary/hevc/`) — Two-byte NAL header walker; VPS-id extraction + SPS decoder (port of `r_hevc.cpp`) including the `profile_tier_level` block, `conformance_window` cropping, and the high-bit-depth + Main 10 path.  Level is decimal-decoded (`120 → "4.0"`).
- **MPEG-1/2 video ES** (`elementary/mpeg_video.rs`) — Sequence-header start code `0x000001B3` + 12+12+4+4 fixed bit-field layout for width / height / aspect-ratio / frame-rate-code.  Frame rate table covers the 8 documented codes (incl. 23.976, 29.97, 59.94 fractional rates).
- **VC-1** (`elementary/vc1.rs`) — Advanced-profile sequence start code `0x000001 0F` + bit-decoded macroblock dimensions.
- **Dirac** (`elementary/dirac.rs`) — `BBCD` parse-info magic + sequence-header parse-code (`0x00`).
- **DV** (`elementary/dv.rs`) — Header DIF block `0x1F 0x07 0x00` + the `dsf` flag for NTSC/PAL classification (720×480 / 720×576).
- **AV1 OBU** (`elementary/obu.rs`) — strict probe requires a `temporal_delimiter` OBU as the first byte (avoids collision with AC-3's `0x0B 0x77` first byte).  LEB128-decoded sizes; sequence_header OBU decode covers `seq_profile`, `max_frame_width/height`, `bit_depth` (8/10/12), `monochrome`, and chroma subsampling factors derived from the profile.

**Subtitle readers:** nine header-only ports under `subtitles/`, plus a shared `encoding` helper.

- **Text formats** decode the first 1-16 KB into UTF-8 (using `encoding_rs::Encoding::for_bom` for UTF-8 / UTF-16 LE / UTF-16 BE detection) and recognise format-specific line patterns:
  - `srt.rs` — `HH:MM:SS,mmm --> HH:MM:SS,mmm` timecode line (also accepts `.` and `:` as the ms separator; tolerates up to 4-digit hours).
  - `ssa.rs` — `[Script Info]` + `[V4+ Styles]` (ASS) / `[V4 Styles]` (SSA); `ScriptType: v4.00+/v4.00` lines disambiguate when no styles section is present.
  - `webvtt.rs` — leading `WEBVTT` followed by newline/tab/space/EOF (W3C-conformant; stricter than mkvtoolnix's `r_webvtt.cpp`).
  - `usf.rs` — root `<USFSubtitles` element, tolerating any number of `<?xml ?>` declarations and leading `<!-- ... -->` comments.
  - `microdvd.rs` — `{startFrame}{endFrame}text` line shape (digit-only frame numbers).
- **Image / segment formats** walk length-prefixed segment chains or fixed magic bytes:
  - `pgs.rs` — PGS `.sup` segment chain (`PG` magic + 13-byte header per segment, segment types 0x14/0x15/0x16/0x17/0x80).
  - `hdmv_textst.rs` — `TextST` 6-byte magic + Dialog Style (0x81) chain.
  - `vobsub.rs` — `.idx` manifest with `# VobSub index file, v…` magic (case-insensitive, tolerates BOM); parses `id: XX, index: N` per-language entries, records the sibling `.sub` path under `container.properties.otherFiles`.
  - `vobbtn.rs` — `butonDVD` magic (case-insensitive) + 4-byte PES `private_stream_2` start code at offset 0x10 + 3-byte tail at 0x14 (mirrors `r_vobbtn.cpp`'s structural probe).

**Residual readers** (Phase 10) — three pure-Rust ports under `flv/`, `realmedia/`, and `ivf.rs`.

- **FLV** (`flv/`) — port of `r_flv.cpp`. 9-byte file header (`FLV` magic + version + audio/video flag byte + 4-byte BE data offset), then a chain of 11-byte tag headers (audio 0x08, video 0x09, script 0x12). Walks tags within the first 1 MiB to harvest the codec FOURCC + sample rate / channels (audio) and the codec id + dims (video). Script-data tags are decoded by a hand-rolled AMF0 parser (`flv/script_data.rs`) that surfaces `width` / `height` / `framerate` from `onMetaData` — mirrors `common/amf.cpp`. Video codec ids 2-7 + 12 are recognised (Sorenson H.263, Screen, VP6, VP6+α, Screen v2, H.264, H.265); audio formats 0-14 cover PCM / ADPCM / MP3 / Nellymoser / G.711 / AAC / Speex.
- **RealMedia** (`realmedia/`) — port of `r_real.cpp` + `librmff`. Probes `.RMF` magic, then walks 10-byte chunk headers (`PROP`, `CONT`, `MDPR`, `DATA`). Per-track `type_specific_data` decoders (`stream_props.rs`) cover `real_video_props_t` (VIDO + FOURCC + width/height + Q16.16 fps) and the three audio header versions (v3 hardcoded 14_4 / 8 kHz / mono; v4 with Pascal-string FOURCC tail; v5 with explicit `fourcc3`). Audio FOURCCs map back to Matroska codec ids — `dnet → A_AC3`, `raac/racp → A_AAC`, `cook → A_REAL/COOK`, etc. Walk stops at the first `DATA` chunk — frame payloads never read.
- **IVF** (`ivf.rs`) — port of `r_ivf.cpp` + `common/ivf.{h,cpp}`. 32-byte fixed-layout header (DKIF magic, LE width/height, LE frame-rate-num/den). Only AV1 / VP8 / VP9 fourccs are accepted (matches mkvtoolnix). `default_duration_ns` is derived from `frame_rate_den / frame_rate_num`.

The sub-tree opts into `#![forbid(unsafe_code)]` at `media_metadata/mod.rs`.

**Wire-format invariants** (see [[feedback_protocol_shape]] memory):

- camelCase via `#[serde(rename_all = "camelCase")]` everywhere.
- Domain sub-trees (`video / audio / subtitle`) live as `Option<_>` on `TrackProperties` — never flattened into the parent.
- Fields containing digits use explicit `#[serde(rename = "...")]` because serde's `camelCase` rule mangles them (e.g. `iso639_2` → `iso6392` without the override).
- `u64` / `i64` fields **must** carry `#[specta(type = specta_typescript::Number)]` (or `Option<Number>` / `Vec<Number>`) — specta-typescript otherwise rejects them to prevent JS precision loss. Accept the loss explicitly via the override.
- `protocol_version: u32` (currently 1) is ours; bump it on breaking changes.

The TypeScript counterpart `src/protocol.generated.ts` was generated by specta from the model structs through `src-tauri/tests/protocol_typescript.rs`. That test no longer runs because `src-tauri` is not built, so the file is frozen until Phase 12 deletes it. Don't edit it by hand.

**Configurable timeout** (see [[feedback_parser_timeout]] memory): the per-file budget comes from `config.parser.timeoutMs` (default 10000, clamped 100–60000). It is not surfaced in the UI — edit the config file to change it. On expiry the parser returns `Err(ParseError::Timeout { budget_ms, stage })` immediately — no partial result. The `BMM_PARSER_BUDGET_MS` env var is kept as a dev-only override; the persisted config wins when both are set.

### External tools

Two optional external binaries are integrated, both opt-in via Settings:

- **MKVToolNix** (`mkvmerge`, `mkvmerge`) — required for the core merge flow; the app shells out to them.
- **BetterMediaInfo** — optional; when its path is configured, the "Open in BetterMediaInfo" entry becomes available on each card.

Both paths live under `config.externalTools` (`mkvToolNixPath`, `betterMediaInfoPath`). Backend probe commands return `MkvToolNixStatus` / `BetterMediaInfoStatus` (`crates/bmm-core/src/protocol.rs`) with auto-detected directories — see `crates/bmm-core/src/services.rs` (BetterMediaInfo) and `mkvtoolnix.rs` (MKVToolNix) for the per-OS search paths (Windows registry / `Program Files`, macOS `/Applications`, Linux `$PATH`). The shared **`src/components/settings/ExternalToolPathRow.tsx`** + **`useToolPathDetection.ts`** drive the Browse/Detect UI for both tools — change them once and both settings rows update.

## Project conventions

- **Copyright headers** use `2026` only (e.g. `Copyright (c) 2026. caoccao.com Sam Cao`, `© Copyright 2026`). Don't reintroduce ranges like `2024-2026` — this project started in 2026; the range is a template-copy artifact from other caoccao projects.
- Status labels in the UI use **PascalCase enum values** (`Waiting`, `Merging`, …). i18n keys that back them are lowercase (`queue.status.waiting`, …); the `statusLabel` helper in `Queue.tsx` bridges with `.toLowerCase()`.
- macOS `Stack` alignment must use `sx={{ alignItems: "center" }}`, not the `alignItems` prop — MUI v9 dropped the prop.
- When cards register their `handleMerge` into `fileMergeHandlers`, the signature is `() => Promise<void>`. The toolbar's Merge All awaits each handler sequentially so backend per-drive queue order matches the on-screen file order.
- All `if-else` must have braces.
- Commit messages follow **Conventional Commits** (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, `test:`, `build:`) with a detailed body. The parser delivery uses one commit per phase.
- When adding `u64` / `i64` fields to any `media_metadata::model` struct, annotate them with `#[specta(type = specta_typescript::Number)]` (or `Option<Number>` / `Vec<Number>` as appropriate); the model keeps its specta derives until Phase 12.
- Never edit `src/protocol.generated.ts` by hand — it was generated from the Rust model and is frozen until Phase 12.
