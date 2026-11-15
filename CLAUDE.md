# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build rules

- **Always build and run the release profile. Never build a debug build.**
  Run a release build to validate changes before handing them off.
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

## Common commands

```bash
cargo run --release -p BatchMkvMerge                   # run the app
cargo run --release -p BatchMkvMerge --example screenshots   # docs/screenshots/; `-- --review <dir>` for the visual review set (macOS)
cargo check --release --workspace                      # fast Rust type-check without linking
cargo test --release --workspace                       # run the whole test suite
cargo llvm-cov --release -p bmm-core --summary-only --fail-under-lines 90   # the CI coverage gate
cargo build --profile dist -p BatchMkvMerge            # installer build: release + thin LTO
cargo packager --profile dist -p BatchMkvMerge -f app,dmg   # or nsis,wix / deb,appimage (cargo-packager 0.11.8)
cargo generate-rpm --profile dist -p crates/bmm-app    # .rpm on Linux (cargo-generate-rpm 0.21.1)
```

To try the app by hand without touching your own settings, start it with a scratch home: `HOME=/tmp/bmm-home cargo run --release -p BatchMkvMerge` (on macOS and Linux the config directory follows `$HOME`; on Linux only while `XDG_CONFIG_HOME` is unset).

Version bumps are done with a Deno script — edit the two arguments at the bottom of `scripts/ts/change-version.ts` and run:

```bash
cd scripts/ts && deno task change-version
```

That rewrites `crates/bmm-app/Cargo.toml`, `crates/bmm-core/Cargo.toml`, `crates/bmm-platform/Cargo.toml`, all three `.github/workflows/*.yml` files and the `CFBundleVersion` of `crates/bmm-app/packaging/macos/Info.plist`. It then runs `cargo update -w` (so `Cargo.lock` records the new workspace versions) and warns when `docs/release_notes.md` has no `## <version>` section yet. Before a release, run the manual checklist in `docs/development.md` ("Release checklist") on the `dist` installers from CI.

Versions up to 0.1.0 were a different implementation; its source is in the git history (tag `0.1.0`). Every 1.0.0 installer upgrades a 0.1.0 install in place, and a config written by 0.1.0 loads unchanged.

### Rust toolchain

The toolchain is pinned in `rust-toolchain.toml` at the repository root (currently **1.100.0**); gpui-kit needs 1.92 or newer. The GitHub Actions workflows use `actions-rust-lang/setup-rust-toolchain@v1` **without** a `toolchain:` input, so the action installs the pinned toolchain; with `toolchain: stable` it would also run `rustup override set stable` on the repository root, and that directory override outranks the toolchain file. They also clear `rustflags`: the action's default `-D warnings` would turn every rustc warning into a CI failure, so CI fails on errors and failing checks only.

### Tests

Unit tests are inline (`#[cfg(test)] mod tests {}`); the parser fixture tests are `crates/bmm-core/tests/*_fixtures.rs`. The golden test `crates/bmm-core/tests/merge_args_golden.rs` parses the synthetic files of `tests/golden/fixtures.rs` and checks the rows, `mkvmerge` arguments and copyable command of every `tests/golden/*.json` case; the expected values were recorded from the implementation before 1.0.0, so change them only when the merge command is meant to change. `cargo test --release --workspace` runs all of them, the app's headless UI tests included. Coverage is measured with `cargo-llvm-cov`: the whole `bmm-core` crate is held to ≥ 90 % line coverage, enforced by the Linux CI workflow (`.github/workflows/linux_build.yml`) via `cargo llvm-cov --release -p bmm-core --summary-only --fail-under-lines 90`. Scoping by package keeps the UI crates out and every new core file in, with no file-name exclusion list; local baseline is ~93 % for the crate (~96 % for the parser alone). `merge.rs` is tested through a fake process launcher (`MergeQueue::new`), never a real `mkvmerge`. How to test the UI is under "GPUI and gpui-kit notes" below.

## Architecture

### Workspace

The Rust code is a Cargo workspace (root `Cargo.toml`, one `Cargo.lock`, one `target/`) with three members:

- `crates/bmm-core` — everything UI-free: the parser sub-tree `media_metadata/`, the config file, the per-drive merge queue and its `MergeEvent`s, `services` (file intake, BetterMediaInfo), `mkvtoolnix`, the update check, the result types of `protocol.rs`, and the table rows (`media_track`), the `mkvmerge` command line with the profile's auto-selection (`merge_args`) and the MKV language list with its lookups (`languages`). No GUI toolkit and no async runtime: long-running work runs on plain threads.
- `crates/bmm-platform` — per-OS code GPUI lacks: the taskbar / Dock / launcher progress (`ITaskbarList3`, `NSDockTile`, the `com.canonical.Unity.LauncherEntry` D-Bus signal) and the display a window is on (Windows; GPUI's own lags at mixed display scales).
- `crates/bmm-app` — the gpui-kit application, package `BatchMkvMerge`: a library `batch_mkvmerge` with all code and tests, and a three-line binary that calls `batch_mkvmerge::run()` (`#![windows_subsystem = "windows"]` unconditionally, there is no debug build). The app identity lives in `lib.rs` constants: `APP_IDENTIFIER` (`com.caoccao.batch-mkvmerge`, also the Windows AppUserModelID), `DESKTOP_ID` (`BatchMkvMerge`, the Linux desktop file basename, Wayland app id and launcher-progress URI).

### Application startup

`lib.rs::run` reads the config, layers gpui-kit's own strings from the locale files (`i18n::install_component_strings`, before `gpui_kit::init`), sets the app identity, the locale (`i18n.rs`; `t!` through the crate-local `tr!` macro that returns a `SharedString`) and the theme (`theme.rs`); creates the merge-event channel and the channel for the MKVToolNix directory a merge finds by itself; builds `Services` and `Stores`; starts the merge-event bridge; binds the shortcuts and the global action handlers (`ui/actions.rs`, so they work without focus); installs the macOS menus; opens the main window (`platform/window.rs`); installs the Quit handler behind the close guard; takes the launch arguments; watches BetterMediaInfo; and starts the update check. The app quits when the main window closes, even with the topmost window open.

### State (`state/`)

Every view gets the stores as `Stores` (`config`, `files`, `queue`, `ui`, plus the merge backend and the taskbar). `Stores::new` takes `Services` — config `Persistence`, a `MetadataSource` for parsing, a `MergeBackend` for enqueue and cancel, the `Taskbar`, the `ToolDetector` — so tests pass fixtures and recorders.

- `ConfigStore` (`state/config.rs`): optimistic edits, background writes with a revision so an older write never lands last, a synchronous flush on quit; the profile edits `add_profile`, `delete_active_profile`, `set_active_profile`, `reset_active_profile`, `update_active_profile` (a new profile is `ConfigProfile::named`). Every config write of the app goes through it: settings, window geometry, the update check's `lastChecked` / `lastVersion` (`update::record_check`), tool paths, and the MKVToolNix directory a merge auto-detects. The core never writes the config file.
- `FileStore` (`state/files/`): the files, parsed metadata and table rows, selections, row edits, output paths, manual merges and detaches; it derives the card units and the file list's entries after every change. `load.rs` parses every file once on the background executor (in-flight guard) and runs the profile's automation once per unit membership. `table.rs` holds `TableScope` (a single card's unit, or a group card's units whose edits fan out to the same member position) and the edits behind each table control.
- `QueueStore` (`state/queue.rs`): driven by `MergeEvent`s; ticks every second while a merge runs. A file removed while the merge queue has it stays hidden but active until its `Finished`. It owns the `RunningOutputs` (`state/outputs.rs`). It tracks a batch from the moment an item waits or merges until none does and emits `QueueEvent::Drained(QueueBatch)` once.
- `UiState` (`state/ui.rs`): the tabs, toasts (`show_toast`, `show_snackbar`), the queue's System Notification and Topmost Notification toggles (session-only), the open toolbar menu (`open_menu`), the version the update banner announces, and the closing flag (`set_closing`).

A store notifies only on structural changes (files or queue items added or removed, a status, units or groups, the first or last selection); every change is also emitted as `FileEvent::Changed` / `QueueEvent::ItemChanged` naming the files, for per-card repaints.

### Event flow

- **Merges**: `merge_events.rs` installs the core's sink (`merge::init`). The worker threads push `MergeEvent`s into an `async-channel`; one foreground task applies them to `QueueStore` in the order they were sent. Nothing polls.
- **Enqueue and the other actions that span stores** (`actions.rs` at the crate root): `MergeRequest` (read on the foreground and handed to the merge queue at once, in order), cancel, clear, remove, the per-drive Resume All / Cancel All / Clear Completed, Copy Command and Copy All Commands, Open in BetterMediaInfo and its detection. Merge All enqueues the cards' units in on-screen order, so the per-drive queue order matches the file list. The output is chosen just before `mkvmerge` starts (`MergeArgs::AtLaunch`, `resolve_merge_output_for`): a name the app picks gets " (1)", " (2)", … while a file is there or a running merge claims it (`RunningOutputs`, compared by `output_key`); a file path the user set is never replaced, the merge fails instead. Copy Command, Copy All Commands and the output dialog's default path follow the same rules. Delete and Clear All do nothing while a member of the unit merges.
- **Notifications** (`notifications.rs`): every finished file gets a top-centre toast (`QueueEvent::Finished`); a drained batch becomes a `CompletionNotice`. **The OS notification and the topmost window are sent once per drained batch, not per file**, only when a merge of the batch completed or failed, and never while the window closes (`UiState::set_closing`, set by the exit dialog's Yes). A batch of one completed file names the file with "Completed in"; any other shows the counts and "Finished in" from the first start to the last end. The root view delivers it: `Notification::…system()` (the OS notification centre only; an unbundled `cargo run` on macOS drops it silently) and the topmost window.
- **Taskbar progress**: `platform::taskbar::Taskbar` (a service: `bmm_platform::TaskbarProgress` in the app, a recorder in tests) is set from `QueueSummary` on every queue change while a merge is active and hidden otherwise.
- **Update check**: `updates.rs` runs `bmm_core::update::startup_check` (an `UpdateSource` that is GitHub in the app) on the background executor and records the check through the config store, so a later settings edit never writes an older `update` block; `ui/update_banner.rs` shows a newer version above the toolbar with Skip this version.
- **External tools**: `tools.rs` is the `ToolDetector` service behind the Integration settings (core `mkvtoolnix::detect_mkvtoolnix` and `services::detect_better_media_info`; a stand-in in tests) and the relay of the MKVToolNix directory a merge finds by itself (`mkvtoolnix::init_auto_detected` → `tools::store_auto_detected` → `ConfigStore`).

### Views (`ui/`)

- `root.rs` is the window content: update banner, toolbar, tabs and footer, plus what belongs to the whole window (OS file drop, toasts, geometry persistence, the title `BatchMkvMerge v{ver}` with ` - done/total` while merging, the taskbar progress, the completion notice, live language and theme changes, the close request). `toolbar.rs` has the icon buttons with tooltips and the group-mode and profile menus: gpui-kit `PopupMenu`s in a controlled `Popover` whose open state is `UiState::open_menu`, so F8 / F9 open them too. `main_content.rs` has the tabs (File List, Queue, and the closable Settings and About; closing the active one selects its left neighbour). `welcome.rs` (fixed blues in light and dark), `about.rs` and `footer.rs` complete the shell.
- **File list** (`file_list.rs`) and **cards** (`cards/`): `FileCard` (status icon, root path with the child-count badge whose popover lists the other members with a remove button, the summary, Set Output Path, Open in BetterMediaInfo, Copy Command, Merge, Delete, the drag handle, the progress row with `elapsed / ETA` and Cancel while active, the track table) and `GroupCard` (the directory and summary, output directory, BetterMediaInfo, Copy All Commands, Merge All, Clear All, the roots with status and progress on the left of a 6 px splitter, min 160 px, at least 240 px left for the table). The file list keeps cards and tables by the card's first root, makes a card again around the same table when it changes kind, and hit-tests a card drag (`CardDrag`, GPUI `on_drag` from the handle with a file-name chip) against the cards' bounds: a file card other than the source is the drop target (copy cursor, dashed outline), a group card or the source refuses it. Files dragged in from the system outline the list. Card messages are bottom-centre toasts (`UiState::show_snackbar`). The encoded date is shown in local time as `2026-05-24 09:00:00` (`format::format_encoded_date`).
- **Track table** (`track_table/`): `TrackTable` is built on gpui-base's interactive `Table` primitives, because gpui-kit's composable `Table` rows take no mouse handlers. The language and track-name cells are `AutocompleteCell`s (`autocomplete.rs`: every option listed, preferred languages first with a divider, the match highlighted and centred, ↓/↑/Enter/Escape as in a combo box, an alias such as `ger` normalised to `de` on Enter). The tables are kept by the card's first root so that typing (which can change a group key) keeps the views. Row selection, the keyboard cursor and the drag state live in the table view; a click in a card makes it the active card. The table keys (↑/↓, Space, Ctrl/Cmd+A, Ctrl/Cmd+Shift+A) go through a keystroke interceptor on the file list: it acts on the active card's table on the File List tab, unless a text field has the focus or another binding claims the key, and Space only when nothing or that table has the focus (a focused button or checkbox takes Space). A row drag starts after 5 px of pointer travel, through the table's own pointer state machine: GPUI starts its own drag after a hard-coded 2 px and then drops the click.
- **Output path dialog** (`output_dialog.rs`, `OutputPathDialog`): directory or file mode, a 300 ms debounced "will be created" warning, a writable check on OK; also the toolbar's global output path.
- **Queue** (`queue/`): one card per drive key titled with the drive (`C:`, `\\SERVER\SHARE`; "Queue" elsewhere and for the empty card) with the two notification toggles, Resume All, Cancel All and Clear Completed, and a table whose rows are `QueueRow` entities drawn as cached views at their measured height, so only the merging row repaints; a failed status shows the error as a tooltip; below 200 px for File Path the table scrolls sideways.
- **Topmost window** (`topmost.rs`, `TopmostWindow`): a 440×200 `PopUp` window centred on the main window's display work area, reused while open; Escape, Enter and Close close it. On Wayland it is an overlay layer-shell surface, falling back to `PopUp` without layer-shell.
- **Settings** (`settings/`): gpui-kit's `setting::Settings` component with six pages (Appearance, Formatting, Group, Integration, Profiles, Update) and their groups listed under each page in the sidebar; gpui-kit's per-page Reset All goes back to `Config::default()` on every page but Profiles, which resets the active profile instead. Every control writes its config field at once (tests compare the serialised config). The view is made when the tab opens and dropped when it closes; while another tab shows, it is laid out hidden under the same element path (`panel-settings`), because gpui-kit's settings keep their page and search in `use_keyed_state`, which lives only while rendered under the same global id. Profiles: Delete and Reset ask first; `LanguagePicker` (two panes, GPUI drag with a `+N` chip, the Preferred pane computes the insert position from `on_drag_move`; no keyboard drag), `TrackNamesEditor` (filterable list with ↑/↓, a textarea per language) and the und language as a searchable `Select` (`language_select.rs`). Integration: one `ToolPathRow` drives Browse / Detect for both tools; it checks the text 250 ms after the last edit, stores it on blur, and Detect also looks for a running app. A row of a text field and buttons wraps the buttons below the field under `ui::settings::FIELD_MIN_WIDTH`.
- **Numeric word navigation** (`ui/word_navigation.rs`): one app-wide keystroke interceptor (installed by `ui::actions::install`). It looks the key up with `cx.key_bindings().borrow().bindings_for_input(...)`, because an interceptor gets `action: None`, and acts only when the winning binding is one of gpui-kit's word actions (`MoveToPreviousWord` / `MoveToNextWord`, `SelectToPreviousWordStart` / `SelectToNextWordEnd`, `DeleteToPreviousWordStart` / `DeleteToNextWordEnd`) and the focused input (`WindowExt::focused_input`: every `Input` and `Textarea`, gpui-kit's own and those in dialogs) is not masked or composing. Element action handlers cannot do it: the input's own handler stops propagation, and dialogs are not under `AppView`. The stops come from `domain::word_navigation::word_command`, one rule on every OS fitted to gpui-kit's jumps. gpui-kit jumps the same way on every platform: forward to a word's end, backward to a word's start, across line breaks, and a jump that starts beside punctuation stops on its other side (`stock_word_jump` copies it, and a test checks the copy against the real input). With the rule, each part of a number is a word: forward jumps end at a part's end, backward jumps at its start, and the separator has no stop of its own. Text without digits is untouched. A word deletion selects its range and lets gpui-kit's own deletion remove it, so undo brings the part back selected.

### Platform (`platform/`)

Window geometry and title (`window.rs`), the close guard (`close_guard.rs`), the macOS menus (`menus.rs`), the file dialogs (`dialogs.rs`), the taskbar progress (`taskbar.rs`), and the launch arguments and the Quit handler (`mod.rs`).

- Window geometry: GPUI's macOS window opens with a *content* size but reports its *frame* (title bar included), so the saved size is `window.viewport_size()`; a maximized or fullscreen window is not saved, so its restore geometry survives. Saves are debounced 500 ms and repeated on close and Quit. The geometry is saved with the display's UUID (`config.window.display`), as positions alone cannot tell displays apart (mixed scales on Windows, every display at the origin on macOS). The window reopens on that display when it holds the window's centre or origin, else on the display under them, with its centre moved onto the display (GPUI's Windows backend drops bounds whose centre is off it); the smallest window is 600 × 450.
- Close guard: closing the main window, Cmd/Ctrl+Q or Quit while a merge waits or runs asks first. The exit dialog has its own footer so Enter and Escape answer No (gpui-kit dialogs bind Enter to their OK button); Yes cancels the merges, then quits.
- File dialogs on Linux go through the XDG desktop portal (`ashpd`), which desktops provide.

### Domain logic

`bmm-core` has `media_track` (the selection table's rows from a parsed file), `merge_args` (the `mkvmerge` command line, profile auto-selection, path helpers) and `languages` (the MKV language list, alias and short-code lookups, autocomplete options); `bmm-app::domain` has `file_tree` (merge trees, manual merges, detaches, combined track order, flag automation), `grouping` (group-card keys), `format`, `status` (queue item states from `MergeEvent`s), `metadata_error` and `word_navigation`. Numbers, strings and rounding follow Rust: `str::parse::<f64>`, `format!("{:.*}")` (exact ties round to even: 1.125 → `1.12`), byte-wise string order and length, `char::is_whitespace`. A combined track id joins file and key with a NUL. Word navigation works on UTF-8 byte offsets.

### Themes

`theme::THEMES` holds the twenty config themes with their main colours; each `themes/<name>.json` (light and dark) is derived from it by fixed rules (hover and active = the main colour × 0.7; dark-mode link hover = the main colour lightened by 20 %; foreground white or 87 % black by the WCAG contrast ratio 3), and `every_theme_file_is_derived_from_its_main_colour` checks the files (`BMM_WRITE_THEMES=1` rewrites them). Light, dark and auto (following the OS appearance, live) are supported. `examples/screenshots.rs` renders `docs/screenshots/` and, with `--review`, every locale through every tab at 600 × 450 and every theme. Layout rules from that review: a row of a text field and buttons wraps the buttons below the field, the queue's fixed columns fit the longest translation on one line, and a truncated language-picker row has a tooltip.

### File intake and parsing

`bmm_core::services::get_media_files(paths)` walks dropped or launch-argument paths recursively, filtered by `media_metadata::probe::extension_hint::is_supported_media_path`: every extension `mkvmerge -J` recognises, plus a VobSub `.sub` data file when the sibling `.idx` manifest is present (mkvtoolnix's path-aware VobSub open flow). The app parses each file once on the background executor with `services::read_media_metadata` and `ParseOptions` from `services::parser_options_from_config`: the persisted `config.parser.timeoutMs` wins over the legacy `BMM_PARSER_BUDGET_MS` env override, which applies only while the config holds the default timeout. A failure is kept per file as a `protocol::MediaMetadataErrorPayload` (`Io | UnexpectedEof | Unrecognised | Timeout | Malformed | OversizedElement | Internal`); `domain::metadata_error` maps each variant to a `merge.error.parser.*` i18n key when it is shown.

### Per-drive merge queue

`crates/bmm-core/src/merge.rs` owns all live merge state in a `MergeQueue`. One process-wide instance sits behind the free functions `enqueue` / `cancel`; tests build their own with a fake launcher.

- `tasks: HashMap<file, TaskState>` — metadata (status, progress, args, cancel flag).
- `drives: HashMap<drive_key, DriveState>` — per-drive `merging + queued`.
- `children: HashMap<file, Arc<Mutex<Box<dyn MergeProcessControl>>>>` — kept separately so `cancel()` can `kill()` from a different call site.

`get_drive_key` returns the Windows path `Prefix` component (`C:`, `\\server\share`) uppercased, else `"default"`. When a file is enqueued and the drive's `merging` slot is free, it's promoted immediately and a worker is `std::thread::spawn`'d. A worker builds arguments given as `MergeArgs::AtLaunch` just before launching `mkvmerge`; an `Err` ends the merge as `Failed`. When a worker finishes, `finish` drops the task, sends `MergeEvent::Finished`, then frees the drive and pops its next file, so a file enqueued again in answer to `Finished` is a new task.

Workers push `MergeEvent`s into the sink installed with `merge::init`: `Started`, `Progress` (only when the percentage changes), and `Finished` with `Completed | Cancelled | Failed`. Every enqueued file gets exactly one `Finished` — also a file cancelled while still waiting, and a worker that panics (`Failed`). The app's sink (`merge_events.rs`) relays them through a channel to `QueueStore`; nothing polls.

### Profiles

Profiles live in the persisted config (one `ConfigProfile` per entry, with a shared `activeProfile` pointer). Each profile carries per-track-type auto-select flags plus language filters (video / audio / subtitle). The `Default` profile auto-selects subtitle tracks. (The per-stream filename templates were removed — the app merges files rather than naming per-track outputs.)

The active profile is consumed both on track auto-select (once per unit membership, `state/files/load.rs`) and at merge time (`merge_args::TrackSelector`, the `mkvmerge` command line).

### Config file location

Config is stored per-OS:
- Windows — `%APPDATA%\BatchMkvMerge\` in installed mode (exe under `%LOCALAPPDATA%` / `%ProgramFiles%` / `%ProgramFiles(x86)%`), next to the `.exe` in portable mode.
- Linux — `$XDG_CONFIG_HOME/BatchMkvMerge/`, else `$HOME/.config/BatchMkvMerge/`.
- macOS — `~/Library/Application Support/BatchMkvMerge/`.

Detection lives in `crates/bmm-core/src/config.rs::get_config_dir`. Old config files from earlier schema versions still load because every new field has `#[serde(default = "...")]`. Notable nested blocks:

- `config.externalTools` — MKVToolNix + BetterMediaInfo paths.
- `config.parser.timeoutMs` — per-file parse budget for the native parser (default 10000, clamped to 100–60000 ms by `ConfigParser::effective_timeout_ms`). Not exposed in the UI — edit the config file to change it.

### i18n

Nine locales (`de`, `en-US`, `es`, `fr`, `it`, `ja`, `zh-CN`, `zh-HK`, `zh-TW`) in `crates/bmm-app/locales/*.json` (rust-i18n, nested keys, `%{name}` placeholders). When adding a user-facing string, add the key to **all nine** files: the test `i18n::tests::every_locale_has_the_same_keys_and_placeholders_as_en_us` fails otherwise, and `every_literal_key_in_the_sources_exists_in_en_us` scans `src/` for `tr!` / `rust_i18n::t!` keys. A missing key would fall back to `en-US`, but that's not a design pattern to lean on. gpui-kit's own strings the app can show are in the `gpui_component` part of every locale file and layered over gpui-kit's with `i18n::install_component_strings` (rust-i18n accepts one extension per process, so it is once-guarded; tests may call it); keep that part's key set equal in all nine files, like every other key. The rust-i18n locale is process-wide (see the test notes below).

### Native media-metadata parser

A pure-Rust header-only parser lives under `crates/bmm-core/src/media_metadata/`. It replaces a `mkvmerge -J` subprocess, the drag-drop filter accepts every extension `mkvmerge -J` recognises plus path-aware VobSub `.sub` inputs, and CI gates `bmm-core` line coverage at ≥ 90 %. Each format reader is a port of the matching `mkvtoolnix/src/input/r_*.cpp` reader; `docs/parsers/` documents coverage and behaviour per format.

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

**Residual readers** — three pure-Rust ports under `flv/`, `realmedia/`, and `ivf.rs`.

- **FLV** (`flv/`) — port of `r_flv.cpp`. 9-byte file header (`FLV` magic + version + audio/video flag byte + 4-byte BE data offset), then a chain of 11-byte tag headers (audio 0x08, video 0x09, script 0x12). Walks tags within the first 1 MiB to harvest the codec FOURCC + sample rate / channels (audio) and the codec id + dims (video). Script-data tags are decoded by a hand-rolled AMF0 parser (`flv/script_data.rs`) that surfaces `width` / `height` / `framerate` from `onMetaData` — mirrors `common/amf.cpp`. Video codec ids 2-7 + 12 are recognised (Sorenson H.263, Screen, VP6, VP6+α, Screen v2, H.264, H.265); audio formats 0-14 cover PCM / ADPCM / MP3 / Nellymoser / G.711 / AAC / Speex.
- **RealMedia** (`realmedia/`) — port of `r_real.cpp` + `librmff`. Probes `.RMF` magic, then walks 10-byte chunk headers (`PROP`, `CONT`, `MDPR`, `DATA`). Per-track `type_specific_data` decoders (`stream_props.rs`) cover `real_video_props_t` (VIDO + FOURCC + width/height + Q16.16 fps) and the three audio header versions (v3 hardcoded 14_4 / 8 kHz / mono; v4 with Pascal-string FOURCC tail; v5 with explicit `fourcc3`). Audio FOURCCs map back to Matroska codec ids — `dnet → A_AC3`, `raac/racp → A_AAC`, `cook → A_REAL/COOK`, etc. Walk stops at the first `DATA` chunk — frame payloads never read.
- **IVF** (`ivf.rs`) — port of `r_ivf.cpp` + `common/ivf.{h,cpp}`. 32-byte fixed-layout header (DKIF magic, LE width/height, LE frame-rate-num/den). Only AV1 / VP8 / VP9 fourccs are accepted (matches mkvtoolnix). `default_duration_ns` is derived from `frame_rate_den / frame_rate_num`.

The sub-tree opts into `#![forbid(unsafe_code)]` at `media_metadata/mod.rs`.

**Wire-format invariants** (see [[feedback_protocol_shape]] memory):

- camelCase via `#[serde(rename_all = "camelCase")]` everywhere.
- Domain sub-trees (`video / audio / subtitle`) live as `Option<_>` on `TrackProperties` — never flattened into the parent.
- Fields containing digits use explicit `#[serde(rename = "...")]` because serde's `camelCase` rule mangles them (e.g. `iso639_2` → `iso6392` without the override).
- `protocol_version: u32` (currently 1) is ours; bump it on breaking changes.

**Configurable timeout** (see [[feedback_parser_timeout]] memory): the per-file budget comes from `config.parser.timeoutMs` (default 10000, clamped 100–60000). It is not surfaced in the UI — edit the config file to change it. On expiry the parser returns `Err(ParseError::Timeout { budget_ms, stage })` immediately — no partial result. The `BMM_PARSER_BUDGET_MS` env var is kept as a dev-only override; the persisted config wins when both are set.

### External tools

Two optional external binaries are integrated, both opt-in via Settings:

- **MKVToolNix** (`mkvmerge`) — required for the core merge flow; the app shells out to it.
- **BetterMediaInfo** — optional; when it is found, the "Open in BetterMediaInfo" entry becomes available on each card.

Both paths live under `config.externalTools` (`mkvToolNixPath`, `betterMediaInfoPath`). The core's detection (`mkvtoolnix::detect_mkvtoolnix`, `services::detect_better_media_info`) returns `MkvToolNixStatus` / `BetterMediaInfoStatus` (`crates/bmm-core/src/protocol.rs`) with auto-detected directories and never writes the config; neither does a merge that finds MKVToolNix by itself (on macOS, the newest `MKVToolNix-<version>.app` when the default path has no `mkvmerge`): it reports the directory to the sink installed with `mkvtoolnix::init_auto_detected`, and the app stores it through `ConfigStore` (`tools::store_auto_detected`), so the Integration row shows it. Detect looks at a running `mkvtoolnix-gui` / BetterMediaInfo first, then the configured path (MKVToolNix: default `C:\Program Files\MKVToolNix`, `/Applications/MKVToolNix.app/Contents/MacOS`, `/usr/bin`); BetterMediaInfo also tries `%LOCALAPPDATA%\Programs`, `%ProgramFiles%` and `%ProgramFiles(x86)%`, `/Applications` and `/usr/bin`, `/usr/local/bin` (`crates/bmm-core/src/services.rs`, `mkvtoolnix.rs`). There is no registry or `$PATH` search. One `ToolPathRow` (`ui/settings/integration.rs`) drives the Browse/Detect UI for both tools, so a change there updates both settings rows.

## GPUI and gpui-kit notes

### Versions and assets

- `gpui-kit` is an exact pin (`=0.9.1`). Its `test-support` feature is a dev-dependency only, so it never reaches the shipped binary. Because Cargo resolves weak `dep?/feature` entries into the lockfile, `test-support` makes `Cargo.lock` carry the `gpui-fast*` crates and their exact pins; they are never compiled. If a resolution conflict appears after a gpui-kit bump, re-resolve the gpui graph in one go instead of downgrading crates one by one. After a bump, also re-derive the Linux package dependencies (`docs/development.md`).
- Icons: `gpui_kit::assets::IconName` is the full Lucide catalog; `gpui_kit::component::IconName` is a small legacy subset. Only 106 SVGs are embedded by default; `src/assets.rs` adds the others the UI uses through `icon_assets!` and serves the app's own images (`images/icon.png`).
- `KeyBinding::new` cannot parse `secondary-…` (it needs the OS); bind `cmd-…` / `ctrl-…` per platform.

### Layout and rendering

- An `absolute()` element needs explicit insets (`top_0().left_0()`); without them taffy places it at its static position after its siblings.
- Cached views: the file list draws each card, and the queue each row, with `Entity::cached` at the height it measured in its last render (`CardHeight`, a canvas probe that asks for one more frame when the height changes), so a progress event or tick re-renders only that card. A cached view is laid out from the given style, never measured from its content, and it re-renders only when it or a descendant view is notified, when the window refreshes, or when its bounds move: a card growing a progress row moves the cards below it, which then render once. Reading a store during render does not invalidate a cached view; cards subscribe to the events of their own files. Every interactive element still works inside a cached view (hover and tooltips notify the view).
- GPUI does not replay a reused cached view's accessibility nodes, so a cached card or queue row drops out of the accessibility tree on every frame that reuses it: both lists draw through `CardHeight::view(view, cache)` with `cache = !window.is_a11y_active()`, uncached while a screen reader or other assistive client is connected. The test platform cannot turn accessibility on.
- `ButtonCustomVariant` paints its colour at 20 % strength in gpui-kit 0.9.1; a solid button adds `.bg(...)`. A disabled gpui-kit button keeps an icon colour set with `Icon::text_color`, so a coloured icon is applied only while the button is enabled.
- gpui-kit's scrollbar paints its 16 px track over the right edge of a scrolled view until it fades out; that overlap is not a layout fault. gpui-kit's `Scrollable` installs its own scroll handle; add a scrollbar to a tracked handle with `ScrollableElement::vertical_scrollbar(&handle)` instead. A scroll container without `restrict_scroll_to_axis()` turns a wheel delta of the other axis into its own (a vertical wheel over a sideways-scrolling table slides it).
- gpui-kit's `Checkbox` has no indeterminate state; gpui-base's has (and reports it as mixed). `ui/checkbox.rs` draws one like gpui-kit's with a minus mark for the select-all headers.
- GPUI draws colour emoji on Linux only from the font named `NotoColorEmoji`, which is why the Linux packages require Noto Color Emoji.

### Input, keys and dialogs

- A key binding beats an element's key-down listeners and text input, so a plain `space` binding would steal Space from focused buttons and from text fields; a keystroke interceptor (`cx.intercept_keystrokes`) sees the context stack and the window first and consumes only what it handles.
- An action dispatched by a key press runs while its window is being updated, so a global handler cannot `update` that window: defer with `cx.defer` (the Quit handler does).
- GPUI does not draw before dispatching a mouse event, so listeners registered in a paint (`window.on_mouse_event`) miss a release that comes before the next frame; the track table also listens with `on_mouse_up` / `on_mouse_up_out`.
- `InputState::set_value` emits no `Change`, and a focused cell is never overwritten from the store: its own edits arrive only after the keystroke's effects run.
- A gpui-kit dialog binds Enter to its Confirm action, which closes it without calling a form's own handler: route `Dialog::on_ok` to the form and return `false` until the form closes the dialog itself.

### Tests

- UI tests use `#[gpui_kit::test]` and `gpui_kit::test::TestWindowExt` (`click`, `drag`, `input`, `press`, `find`). Mark elements a test must find with `.test_support()` (`gpui_kit::base::TestSupportExt`; a no-op without the feature). In a test module that sees `gpui_kit::*`, add `use core::prelude::v1::test;`, because the glob also exports GPUI's `test` macro and `#[test]` would recurse.
- GPUI delivers focus events only to an active window and through deferred effects: call `window.activate_window()` once and `cx.run_until_parked()` after a click before typing.
- Test queries see only elements marked `.test_support()` and gpui-kit's own components; icon-only buttons need `.accessibility_label(...)` (the tooltip text) to be found by label, which also names them for screen readers. Text components record their text for tests (`gpui_kit::base::TestTextExt::test_text`, read back with `gpui_kit::base::test_support::texts(window)`); the cards mark their texts so tests can read what is shown. `.test_support()` wraps the element type under the test feature, so a helper returns `AnyElement`.
- gpui-kit findings: `RadioGroup` renames its radios by index (find them by label); a `Select`'s options and a `ButtonGroup` itself are not test-visible (drive the select by keyboard); the settings sidebar entries are `0-<page>`.
- `TestWindowExt::render_frame` refreshes the window and so bypasses caching; a test that counts renders draws with `window.draw(cx).clear(cx)`.
- The rust-i18n locale is process-wide: a test that renders the shell or switches the language holds `i18n::tests::LOCALE_LOCK` (the shell test helper does), or text width changes under another test's click. `Config::default()` detects the system language; tests set `language` explicitly.
- GPUI's test scheduler panics when another thread wakes a foreground task: a test that runs the core's merge queue relays the worker events through the test thread.
- Tests read system notifications with `TestAppContext::shown_system_notifications()`, which records only after `cx.set_app_identity(...)`.
- Tests that press editing shortcuts use the platform's key (Select All is Cmd+A on macOS, Ctrl+A elsewhere), and tests that compare checked-in text files normalise line endings (Windows checks them out with CRLF).

### Live checks on macOS

- Run the release app with a scratch `HOME`; the config directory follows `$HOME`, so the real config stays untouched.
- System Events can drive the app through its accessibility tree: buttons by their title, `enabled`, table rows, checkboxes and text fields with their values. Read the tree only after the views were drawn uncached (an assistive client turns caching off; before that, a 1 px window resize redraws everything). accesskit reports a mixed checkbox as `true` on macOS, so the indeterminate select-all box is checked by a test instead.
- System Events' `click at` is an accessibility press, which does not focus a GPUI input; `set focused of text field N of group 1 of window 1 to true` does. A field's accessibility `value` follows the text about a frame late, so wait about a second before reading it.
- Real clicks, drags and a Finder drop onto the window need posted mouse events (a small `CGEvent` helper). `screencapture` needs screen-recording permission.

## Packaging and CI

- Packaging metadata is in `crates/bmm-app/Cargo.toml` (`[package.metadata.packager]`, `[package.metadata.generate-rpm]`). Every installer identity matches the 0.1.0 release: deb and rpm package `batch-mkv-merge`, bundle id, NSIS per-user install with publisher `caoccao`, and the MSI UpgradeCode, which `packaging/wix/main.wxs` pins because cargo-packager would derive a different one. `packaging/macos/Info.plist` is merged over the generated `Info.plist` and sets `CFBundleVersion` to the version, as 0.1.0 had, instead of cargo-packager's build timestamp. There is no `license-file`: cargo-packager would turn it into a license agreement before the `.dmg` opens and a license page in the NSIS and MSI installers, which 0.1.0 did not have. cargo-packager rejects unknown keys (NSIS takes `installMode`, not `install-mode`).
- Linux dependencies: the `.deb` `depends` are `dpkg-shlibdeps`'s list for the `ubuntu-22.04` `dist` binary plus what GPUI loads at run time (Vulkan loader, Wayland client, EGL and wayland-egl for wgpu's GL fallback, `fontconfig-config` for `/etc/fonts/fonts.conf`, which a pure-Rust parser reads, and Noto Color Emoji); the `.rpm` gets the linked libraries from rpm's own soname scan and lists the rest as capabilities (`libvulkan.so.1()(64bit)`, `font(notocoloremoji)`, …). Re-derive both after a gpui-kit bump (`docs/development.md`). File dialogs need an XDG desktop portal, which desktops provide; it is not a dependency.
- CI: three workflows, `linux_build.yml` (`ubuntu-22.04` for a low glibc floor; GPUI's build dependencies and Mesa's Vulkan drivers, the headless UI tests, the coverage gate, `.deb`, `.AppImage`, `.rpm`), `macos_build.yml` (`.dmg` for x86_64 on `macos-15-intel` and arm64 on `macos-15`) and `windows_build.yml` (one `.msi` per language and the NSIS `.exe`). Each runs `cargo test --release --workspace --locked`, builds with `--profile dist` and uploads seven artifacts in all, named `BatchMkvMerge-<os>-<arch>-<version>`, with `-<format>` on Linux and Windows (the names of the 0.1.0 artifacts). Installers are unsigned (an ad-hoc signature on macOS).

## Platform-specific notes

- Development here happens on Windows with Git Bash. Use forward-slash paths (`C:/…` not `C:\\…`), `/dev/null` not `NUL`, and avoid `cd <project>` before `git` commands — git already runs against the working tree and prepending `cd` triggers an extra permission prompt.
- MKVToolNix (`mkvmerge`) must be installed separately — the app shells out to it.

## Project conventions

- **Copyright headers** use `2026` only (e.g. `Copyright (c) 2026. caoccao.com Sam Cao`, `© Copyright 2026`). Don't reintroduce ranges like `2024-2026` — this project started in 2026; the range is a template-copy artifact from other caoccao projects.
- Queue status labels are the `QueueItemStatus` variants (`Waiting`, `Merging`, …); their i18n keys are lowercase (`queue.status.waiting`, …, `QueueItemStatus::label_key`).
- Merge All enqueues the cards' units in on-screen order (`actions::merge_all`), so the per-drive queue order matches the file list.
- All `if-else` must have braces.
- Commit messages follow **Conventional Commits** (`feat:`, `fix:`, `chore:`, `refactor:`, `docs:`, `test:`, `build:`) with a detailed body.
