# Development

## Tech Stack

### Frontend

TODO

### Backend

TODO

### Tauri plugins

TODO

### External tools

TODO

## Tooling

- [pnpm](https://pnpm.io/) — package manager
- [Deno](https://deno.com/) — scripts under `scripts/ts/` (e.g. `change-version.ts`)

## Repository layout

TODO

## Commands

```bash
pnpm install         # install JS dependencies
pnpm tauri dev       # run the app in development (hot-reload frontend + Rust)
pnpm tauri build     # build installers for the current platform
pnpm dev             # run only the Vite dev server (no Tauri window)
pnpm build           # type-check and build the frontend
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

TODO

### Events

TODO

## Concurrency

TODO

## Template engine

TODO

## CI

Three GitHub Actions workflows build release bundles on push/PR:

- `linux_build.yml` — `.deb`, `.rpm`, `.AppImage`
- `macos_build.yml` — `.dmg` (x86_64 and arm64 via matrix)
- `windows_build.yml` — `.msi`, `.exe` (NSIS)

Each installs Node 24, pnpm 11, and the stable Rust toolchain, then runs `cargo test -r` followed by `pnpm tauri build`.
