# Release Notes

## 1.0.0

- Rebuilt BatchMkvMerge as a native Rust application on [gpui-kit](https://crates.io/crates/gpui-kit), with the same features and layout; the Tauri shell, the WebView and the React frontend are gone. Configs written by 0.x load unchanged.
- The system notification and the topmost notification window now appear once, when the last queued merge finishes, with the numbers of completed, failed and cancelled files (one completed file still shows its name and time), instead of once per file. Enter also closes the topmost window.
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

## 0.2.0

- Added optional system and topmost notifications when queued merges complete.
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
