# Release Notes

## 0.1.0

- Initial release of BatchMkvMerge as a cross-platform desktop app (Windows, Linux, macOS) built with Tauri + React.
- Added batch merge workflows for MKVToolNix, including drag-and-drop file intake, queue management, live progress, and cancellation.
- Added per-drive parallel merge scheduling to improve throughput while preserving predictable queue behavior.
- Added profile-driven track selection with language filters and persisted app configuration.
- Introduced a native Rust media metadata parser (header-level) with broad format coverage across containers, audio, subtitles, and elementary streams.
- Added optional BetterMediaInfo integration and external tool path detection in Settings.
- Added multilingual UI support and continuous integration quality gates, including parser-focused test coverage enforcement.
