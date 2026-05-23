# Release Notes

## 0.1.0

### Platforms

* Windows (x86_64)
* Linux (x86_64)
* macOS (x86_64 and arm64)

### Add files

* Drag and drop `.mkv` files or folders onto the window
* Launch the app with file or folder paths as arguments to pre-populate the list
* Integrate with [BetterMediaInfo](https://github.com/caoccao/BetterMediaInfo)

### Extract

* Extract one file from its card, or Extract All selected files at once (F3)
* Copy Command puts the equivalent `mkvmerge` command line on the clipboard
* On Windows, files on different drives extract in parallel; files on the same drive extract one after another
* On Linux and macOS, one file at a time
* Cancel any extraction from its card or from the Queue tab

### Queue tab

* Appears automatically when any file is queued
* Shows File Path, Status, Start, End, Elapsed, ETA for each item
* Statuses: Waiting, Extracting, Completed, Cancelled, Failed
* Completed, Cancelled, and Failed rows stay visible; clear them per drive with the Clear Completed button

### Profiles

* Switch profiles from the toolbar (F9)
* Each profile has its own filename templates for video, audio, and subtitle tracks
* Placeholders: `{file_name}`, `{track_id}`, `{track_number}`, `{language}`, `{codec_name}`, `{track_name}`
* Each profile chooses which track types get selected by default when a file is added
* The built-in `Default` profile selects subtitles
* Add, reset, and delete profiles from Settings

### Appearance and language

* Auto / Light / Dark display mode, 20 color themes
* 8 UI languages: English (US), Deutsch, Español, Français, 日本語, 简体中文, 繁體中文 (香港), 繁體中文 (台灣)
* Dark mode is applied throughout, including queued-card tint and progress bar

### Settings

* MKVToolNix path with live detection of `mkvmerge`; on macOS, the latest versioned install is auto-detected
* Your selected profile, templates, theme, language, and MKVToolNix path are remembered between runs
