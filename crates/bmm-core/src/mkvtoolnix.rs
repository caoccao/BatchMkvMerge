/*
*   Copyright (c) 2026. caoccao.com Sam Cao
*   All rights reserved.

*   Licensed under the Apache License, Version 2.0 (the "License");
*   you may not use this file except in compliance with the License.
*   You may obtain a copy of the License at

*   http://www.apache.org/licenses/LICENSE-2.0

*   Unless required by applicable law or agreed to in writing, software
*   distributed under the License is distributed on an "AS IS" BASIS,
*   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
*   See the License for the specific language governing permissions and
*   limitations under the License.
*/

#[cfg(target_os = "macos")]
use std::cmp::Ordering;
#[cfg(target_os = "macos")]
use std::fs;
use std::io::{BufReader, PipeReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::OnceLock;

use crate::config;
use crate::merge::MergeError;
use crate::protocol::MkvToolNixStatus;

/// The one MKVToolNix tool the app runs; finding it is finding MKVToolNix.
const MKVMERGE: &str = "mkvmerge";

/// Receives the MKVToolNix directory a merge found by itself (on macOS, the
/// newest `MKVToolNix-<version>.app` when the default path has no
/// `mkvmerge`). Called from a merge worker thread; it must not panic and
/// should return quickly, e.g. by sending into a channel.
pub type AutoDetectedSink = Box<dyn Fn(String) + Send + Sync>;

static AUTO_DETECTED_SINK: OnceLock<AutoDetectedSink> = OnceLock::new();

/// Install the sink for auto-detected MKVToolNix directories. The core never
/// writes the config itself: the application stores the path through its own
/// config store, so a later settings write keeps it. Only the first call
/// counts; without a sink the directory is found again at every merge.
pub fn init_auto_detected(sink: AutoDetectedSink) {
  if AUTO_DETECTED_SINK.set(sink).is_err() {
    log::warn!("mkvtoolnix::init_auto_detected called more than once; keeping the first sink");
  }
}

/// One line of `mkvmerge` output.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MkvmergeLine<'a> {
  /// A percentage, 0–100.
  Progress(u32),
  Warning(&'a str),
  Error(&'a str),
  /// Anything else: informational text, or what the process wrote to stderr.
  Other(&'a str),
}

/// Classifies a line of `mkvmerge --gui-mode` output. In GUI mode progress,
/// warnings and errors are `#GUI#progress 42%`, `#GUI#warning …` and
/// `#GUI#error …` whatever the interface language, while the plain-text
/// `Progress:` / `Warning:` / `Error:` prefixes are translated; those are
/// still recognised in English. Other `#GUI#` markers (playlist scanning)
/// are dropped.
pub(crate) fn parse_mkvmerge_line(line: &str) -> Option<MkvmergeLine<'_>> {
  let trimmed = line.trim();
  if trimmed.is_empty() {
    return None;
  }
  let progress = trimmed
    .strip_prefix("#GUI#progress")
    .or_else(|| trimmed.strip_prefix("Progress:"));
  if let Some(percent) = progress.and_then(|rest| rest.trim().trim_end_matches('%').trim().parse::<u32>().ok()) {
    return Some(MkvmergeLine::Progress(percent.min(100)));
  }
  let message = |prefixes: [&str; 2]| {
    prefixes
      .iter()
      .find_map(|prefix| trimmed.strip_prefix(prefix))
      .map(str::trim)
  };
  if let Some(text) = message(["#GUI#warning", "Warning:"]) {
    Some(MkvmergeLine::Warning(text))
  } else if let Some(text) = message(["#GUI#error", "Error:"]) {
    Some(MkvmergeLine::Error(text))
  } else if trimmed.starts_with("#GUI#") {
    None
  } else {
    Some(MkvmergeLine::Other(trimmed))
  }
}

pub(crate) fn read_mkvmerge_output<F>(reader: impl Read, mut on_line: F)
where
  F: FnMut(&str),
{
  let mut buf_reader = BufReader::new(reader);
  let mut current_line = Vec::new();
  let mut byte = [0u8; 1];
  loop {
    match buf_reader.read(&mut byte) {
      Ok(0) => break,
      Ok(_) => {
        if byte[0] == b'\r' || byte[0] == b'\n' {
          if !current_line.is_empty() {
            let line = String::from_utf8_lossy(&current_line);
            on_line(&line);
            current_line.clear();
          }
        } else {
          current_line.push(byte[0]);
        }
      }
      Err(_) => break,
    }
  }
  if !current_line.is_empty() {
    let line = String::from_utf8_lossy(&current_line);
    on_line(&line);
  }
}

struct MkvToolNixResolution {
  path: PathBuf,
  auto_detected: bool,
}

fn get_tool_path(path: &Path, tool: &str) -> PathBuf {
  #[cfg(target_os = "windows")]
  {
    let exe_path = path.join(format!("{}.exe", tool));
    if exe_path.exists() && exe_path.is_file() {
      return exe_path;
    }
  }
  path.join(tool)
}

fn resolve_mkvtoolnix_tool_path(tool: &str) -> PathBuf {
  let cfg = config::get_config();
  let configured = &cfg.external_tools.mkv_toolnix_path;
  let resolution = resolve_mkvtoolnix(configured, tool);
  report_auto_detected(&resolution, configured, |path| {
    if let Some(sink) = AUTO_DETECTED_SINK.get() {
      sink(path);
    }
  });
  get_tool_path(&resolution.path, tool)
}

fn create_mkvtoolnix_command(tool: &str) -> (PathBuf, std::process::Command) {
  let tool_path = resolve_mkvtoolnix_tool_path(tool);
  #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
  let mut cmd = std::process::Command::new(&tool_path);
  #[cfg(target_os = "windows")]
  {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
  }
  (tool_path, cmd)
}

fn has_tool(path: &Path, tool: &str) -> bool {
  let tool_path = path.join(tool);
  if tool_path.exists() && tool_path.is_file() {
    return true;
  }
  #[cfg(target_os = "windows")]
  {
    let tool_exe_path = path.join(format!("{}.exe", tool));
    if tool_exe_path.exists() && tool_exe_path.is_file() {
      return true;
    }
  }
  false
}

#[cfg(target_os = "macos")]
fn compare_version_parts(left: &[u32], right: &[u32]) -> Ordering {
  let len = left.len().max(right.len());
  for i in 0..len {
    let l = left.get(i).copied().unwrap_or(0);
    let r = right.get(i).copied().unwrap_or(0);
    match l.cmp(&r) {
      Ordering::Equal => continue,
      non_eq => return non_eq,
    }
  }
  Ordering::Equal
}

#[cfg(target_os = "macos")]
fn parse_version_parts(version: &str) -> Vec<u32> {
  version
    .split('.')
    .filter_map(|part| {
      let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
      if digits.is_empty() {
        None
      } else {
        digits.parse::<u32>().ok()
      }
    })
    .collect()
}

#[cfg(target_os = "macos")]
fn is_default_macos_mkvtoolnix_path(path: &str) -> bool {
  path.trim().trim_end_matches('/') == "/Applications/MKVToolNix.app/Contents/MacOS"
}

#[cfg(target_os = "macos")]
fn find_latest_versioned_macos_mkvtoolnix_path(tool: &str) -> Option<PathBuf> {
  let entries = fs::read_dir("/Applications").ok()?;
  let mut latest: Option<(Vec<u32>, PathBuf)> = None;
  for entry in entries.flatten() {
    let file_name = entry.file_name();
    let app_name = match file_name.to_str() {
      Some(value) => value,
      None => continue,
    };
    if !app_name.starts_with("MKVToolNix-") || !app_name.ends_with(".app") {
      continue;
    }
    let version = &app_name["MKVToolNix-".len()..app_name.len() - ".app".len()];
    let version_parts = parse_version_parts(version);
    if version_parts.is_empty() {
      continue;
    }
    let mkvtoolnix_path = entry.path().join("Contents").join("MacOS");
    if !has_tool(&mkvtoolnix_path, tool) {
      continue;
    }
    match &latest {
      None => latest = Some((version_parts, mkvtoolnix_path)),
      Some((latest_version, _)) => {
        if compare_version_parts(&version_parts, latest_version) == Ordering::Greater {
          latest = Some((version_parts, mkvtoolnix_path));
        }
      }
    }
  }
  latest.map(|(_, path)| path)
}

fn resolve_mkvtoolnix(path: &str, tool: &str) -> MkvToolNixResolution {
  let trimmed_path = path.trim();
  let configured_path = PathBuf::from(trimmed_path);
  if has_tool(&configured_path, tool) {
    return MkvToolNixResolution {
      path: configured_path,
      auto_detected: false,
    };
  }
  #[cfg(target_os = "macos")]
  {
    if is_default_macos_mkvtoolnix_path(trimmed_path) {
      if let Some(latest_path) = find_latest_versioned_macos_mkvtoolnix_path(tool) {
        return MkvToolNixResolution {
          path: latest_path,
          auto_detected: true,
        };
      }
    }
  }
  MkvToolNixResolution {
    path: configured_path,
    auto_detected: false,
  }
}

/// The `mkvmerge` a merge with the MKVToolNix directory `mkv_toolnix_path`
/// runs, found the way [`spawn_mkvmerge`] finds it, without reporting an
/// auto-detected directory. For the copyable command.
pub(crate) fn mkvmerge_executable(mkv_toolnix_path: &str) -> PathBuf {
  get_tool_path(&resolve_mkvtoolnix(mkv_toolnix_path, MKVMERGE).path, MKVMERGE)
}

/// Hands an auto-detected directory that differs from the configured path
/// to `report`.
fn report_auto_detected(resolution: &MkvToolNixResolution, configured: &str, report: impl FnOnce(String)) {
  if !resolution.auto_detected {
    return;
  }
  let path = resolution.path.to_string_lossy().to_string();
  if path != configured {
    report(path);
  }
}

/// Starts `mkvmerge` for the source `file` with the complete argument list
/// `args` (`-o <output>`, the per-file options, the inputs; `file` is only
/// checked here, not passed again). Returns the process and one pipe that
/// carries both its stdout and its stderr, so neither can fill up unread and
/// whatever it prints can be shown. `--gui-mode` keeps its progress, warning
/// and error lines recognisable in every interface language
/// ([`parse_mkvmerge_line`]); the copyable command does not need it.
pub fn spawn_mkvmerge(file: &str, args: &[String]) -> Result<(Child, PipeReader), MergeError> {
  if !Path::new(file).is_file() {
    return Err(MergeError::SourceMissing { path: file.to_owned() });
  }
  let (mkvmerge_path, mut cmd) = create_mkvtoolnix_command(MKVMERGE);
  let pipe_error = |error: std::io::Error| MergeError::Other(format!("Couldn't create the output pipe: {error}"));
  let (output, stdout) = std::io::pipe().map_err(pipe_error)?;
  let stderr = stdout.try_clone().map_err(pipe_error)?;
  cmd
    .arg("--gui-mode")
    .args(args)
    .stdin(Stdio::null())
    .stdout(stdout)
    .stderr(stderr);
  let child = cmd.spawn().map_err(|error| MergeError::NotAvailable {
    path: mkvmerge_path.display().to_string(),
    detail: error.to_string(),
  })?;
  // The command holds the parent's copies of the pipe's write end; reading
  // reaches EOF only once they are closed and the process has exited.
  drop(cmd);
  Ok((child, output))
}

fn mkvtoolnix_gui_process_name() -> &'static str {
  if cfg!(target_os = "windows") {
    "mkvtoolnix-gui.exe"
  } else {
    "mkvtoolnix-gui"
  }
}

/// Finds MKVToolNix without touching the config: with `check_running`, the
/// directory of a running `mkvtoolnix-gui` first, then `path` (on macOS the
/// default path also finds the newest `MKVToolNix-<version>.app`). The
/// application stores the returned path through its own config store.
pub fn detect_mkvtoolnix(path: &str, check_running: bool) -> MkvToolNixStatus {
  if check_running
    && let Some(dir) = crate::services::find_running_process_dir(mkvtoolnix_gui_process_name())
    && has_tool(&dir, MKVMERGE)
  {
    return MkvToolNixStatus {
      found: true,
      mkv_toolnix_path: dir.to_string_lossy().to_string(),
    };
  }
  let trimmed_path = path.trim();
  if trimmed_path.is_empty() {
    return MkvToolNixStatus {
      found: false,
      mkv_toolnix_path: String::new(),
    };
  }
  let resolution = resolve_mkvtoolnix(trimmed_path, MKVMERGE);
  MkvToolNixStatus {
    found: has_tool(&resolution.path, MKVMERGE),
    mkv_toolnix_path: resolution.path.to_string_lossy().to_string(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// A fresh directory under the system temp dir, removed when dropped.
  struct Dir(PathBuf);

  impl Dir {
    fn new(name: &str) -> Self {
      let path = std::env::temp_dir().join(format!("bmm_mkvtoolnix_{name}_{}", std::process::id()));
      let _ = std::fs::remove_dir_all(&path);
      std::fs::create_dir_all(&path).unwrap();
      Self(path)
    }
  }

  impl Drop for Dir {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  fn reported(auto_detected: bool, path: &str, configured: &str) -> Option<String> {
    let resolution = MkvToolNixResolution {
      path: PathBuf::from(path),
      auto_detected,
    };
    let mut reported = None;
    report_auto_detected(&resolution, configured, |path| reported = Some(path));
    reported
  }

  #[test]
  fn only_an_auto_detected_directory_other_than_the_configured_one_is_reported() {
    let detected = "/Applications/MKVToolNix-98.0.app/Contents/MacOS";
    let default = "/Applications/MKVToolNix.app/Contents/MacOS";
    assert_eq!(reported(true, detected, default).as_deref(), Some(detected));
    assert_eq!(reported(true, detected, detected), None, "already configured");
    assert_eq!(reported(false, default, default), None, "the configured path itself");
  }

  #[test]
  fn detection_finds_mkvmerge_in_the_given_directory() {
    let dir = Dir::new("found");
    let tool = if cfg!(target_os = "windows") {
      "mkvmerge.exe"
    } else {
      "mkvmerge"
    };
    std::fs::write(dir.0.join(tool), b"").unwrap();
    let path = dir.0.to_string_lossy().to_string();
    let status = detect_mkvtoolnix(&format!("  {path} "), false);
    assert!(status.found);
    assert_eq!(status.mkv_toolnix_path, path);
  }

  #[test]
  fn detection_reports_a_directory_without_mkvmerge_as_not_found() {
    let dir = Dir::new("missing");
    let path = dir.0.to_string_lossy().to_string();
    let status = detect_mkvtoolnix(&path, false);
    assert!(!status.found);
    assert_eq!(status.mkv_toolnix_path, path);
  }

  #[test]
  fn detection_of_an_empty_path_finds_nothing() {
    let status = detect_mkvtoolnix("   ", false);
    assert!(!status.found);
    assert!(status.mkv_toolnix_path.is_empty());
  }

  #[test]
  fn gui_mode_and_english_lines_are_classified() {
    use MkvmergeLine::*;
    assert_eq!(parse_mkvmerge_line("#GUI#progress 42%"), Some(Progress(42)));
    assert_eq!(parse_mkvmerge_line("Progress: 7%"), Some(Progress(7)));
    assert_eq!(parse_mkvmerge_line("#GUI#progress 120%"), Some(Progress(100)));
    assert_eq!(
      parse_mkvmerge_line("#GUI#warning 'a.mkv': track 7 not found."),
      Some(Warning("'a.mkv': track 7 not found."))
    );
    assert_eq!(parse_mkvmerge_line("Warning: plain"), Some(Warning("plain")));
    assert_eq!(
      parse_mkvmerge_line("#GUI#error The file 'x' could not be opened."),
      Some(Error("The file 'x' could not be opened."))
    );
    assert_eq!(parse_mkvmerge_line("Error: plain"), Some(Error("plain")));
    assert_eq!(parse_mkvmerge_line("Fortschritt: 25%"), Some(Other("Fortschritt: 25%")));
    assert_eq!(parse_mkvmerge_line("#GUI#begin_scanning_playlists"), None);
    assert_eq!(parse_mkvmerge_line("   "), None);
  }

  #[test]
  fn output_is_split_at_carriage_returns_and_newlines() {
    let mut lines = Vec::new();
    read_mkvmerge_output(&b"Progress: 25%\rProgress: 100%\r\nDone.\n\nlast"[..], |line| {
      lines.push(line.to_owned())
    });
    assert_eq!(lines, ["Progress: 25%", "Progress: 100%", "Done.", "last"]);
  }

  #[test]
  fn a_missing_source_is_reported_before_mkvmerge_is_looked_for() {
    match spawn_mkvmerge("/definitely/not/here.mkv", &[]) {
      Err(MergeError::SourceMissing { path }) => assert_eq!(path, "/definitely/not/here.mkv"),
      other => panic!("expected SourceMissing, got {:?}", other.map(|_| ())),
    }
  }
}
