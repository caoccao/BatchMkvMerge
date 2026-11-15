# Release Notes

## 1.0.0

- Rebuilt BatchMkvMerge as a native Rust application on [gpui-kit](https://crates.io/crates/gpui-kit), with the same features and layout; the Tauri shell, the WebView and the React frontend are gone. Configs written by 0.1.0 load unchanged, from the same place.
- System requirements: macOS 12 Monterey or later; Windows 10 1903 or later with a DirectX 11 graphics driver (WebView2 is no longer needed); Linux with an X11 or Wayland session, a Vulkan driver (Mesa is enough) and glibc 2.35 or later (Ubuntu 22.04, Debian 12, Fedora 36 or newer). Older systems stay on 0.1.0.
- The same installers as 0.1.0 (`.dmg` for Intel and Apple silicon, the NSIS `.exe`, one `.msi` per language, `.deb`, `.rpm` and `.AppImage`), still unsigned; the installers and packages upgrade a 0.1.0 install in place.
- The Linux packages depend on the Vulkan loader, the Wayland client, EGL, fontconfig's configuration and Noto Color Emoji (for the track-type icons) instead of WebKitGTK. File dialogs on Linux use the XDG desktop portal.
- Added optional system and topmost notifications when queued merges finish: they appear once, when the last queued merge finishes, with the numbers of completed, failed and cancelled files (one completed file shows its name and time). Enter or Escape closes the topmost window.
- Fixed: on macOS, when the default MKVToolNix path has no `mkvmerge`, the newest `MKVToolNix-<version>.app` a merge finds stays in Settings > Integration; the next settings change used to drop it.
- Settings use a searchable sidebar: the inner tabs became titled groups, every page except Profiles has Reset All, and deleting or resetting a profile asks first. Languages and Track Names each have their own Video / Audio / Subtitles switch.
- The undetermined-language field of the profile automation is a searchable list; the language picker reorders by mouse only.
- File cards show the encoded date in local time (`2026-05-24 09:00:00`) and a chip with the file name while dragged, and a group card's messages stay 5 s like a file card's (3 s before); files dragged in from the system outline the list. Copy Command and the output dialog's default path are exactly what Merge writes.
- Default and forced flag edits are kept when a file joins or leaves a group card: the profile automation runs once per merge unit.
- Parse errors are translated when shown, so they follow a language switch.
- Queue: each drive's card is titled with the drive, a failed row shows its error as a tooltip, the table scrolls sideways when the file path gets too narrow, and the elapsed time and ETA update once a second.
- The track table's ↑ / ↓ / Space keys work on the File List tab only, without modifier keys.
- In every text field, Ctrl+Left/Right (Option on macOS), with or without Shift, and the word deletions stop at each part of a number on every platform: `Movie.2024.1080p` stops at 5, 6, 10 and 16 going right.
- Visuals follow gpui-kit with Lucide icons: headlines are solid instead of gradient text, glows and hover lifts are shadows.
- The window reopens at its saved position only when that position is on a connected display.
- Fixed default-track reset automation to honor user-reordered track order.
- Fixed chapter rows to show a localized chapter label and entry count in the proper columns.
- Added `--no-global-tags` to generated mkvmerge commands.
- Improved native metadata parity with MKVToolNix for MP4 track names, MPEG probing, and direct VobSub `.sub` inputs.

## 0.1.0

- Initial release of BatchMkvMerge as a cross-platform desktop app (Windows, Linux, macOS) built with Tauri + React.
- Added batch merge workflows for MKVToolNix, including drag-and-drop file intake, queue management, live progress, and cancellation.
- Added per-drive parallel merge scheduling to improve throughput while preserving predictable queue behavior.
- Added profile-driven track selection with language filters and persisted app configuration.
- Introduced a native Rust media metadata parser (header-level) with broad format coverage across containers, audio, subtitles, and elementary streams.
- Added optional BetterMediaInfo integration and external tool path detection in Settings.
- Added multilingual UI support and continuous integration quality gates, including parser-focused test coverage enforcement.
