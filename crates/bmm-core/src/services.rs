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

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::config;
use crate::media_metadata;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::probe::extension_hint::is_supported_media_path;
use crate::media_metadata::{ParseError, ParseOptions};
use crate::protocol::BetterMediaInfoStatus;

fn better_media_info_exe_name() -> &'static str {
  if cfg!(target_os = "windows") {
    "BetterMediaInfo.exe"
  } else {
    "BetterMediaInfo"
  }
}

/// Whether a merge can write the output file `path`: a path
/// [`check_output_dir_writable`] accepts. An existing file is refused, as a
/// merge replaces no file (the app's `resolve_merge_output_for` fails the
/// merge instead). Nothing is changed.
pub fn check_output_file_writable(path: String) -> Result<bool> {
  if Path::new(&path).is_file() {
    return Ok(false);
  }
  check_output_dir_writable(path)
}

/// Whether a merge can write into the output directory `path`: its nearest
/// existing ancestor (the path itself, if it exists) is a directory a file
/// can be created in; mkvmerge creates the missing directories. An existing
/// file is not a directory. Nothing is changed.
pub fn check_output_dir_writable(path: String) -> Result<bool> {
  let mut current = PathBuf::from(&path);
  loop {
    if current.exists() {
      break;
    }
    let Some(parent) = current.parent() else {
      return Ok(false);
    };
    current = parent.to_path_buf();
  }
  if !current.is_dir() {
    return Ok(false);
  }
  let test_name = format!(".batchmkvmerge_writecheck_{}", std::process::id());
  let test_path = current.join(&test_name);
  match std::fs::File::create(&test_path) {
    Ok(_) => {
      let _ = std::fs::remove_file(&test_path);
      Ok(true)
    }
    Err(_) => Ok(false),
  }
}

fn common_better_media_info_dirs() -> Vec<PathBuf> {
  let mut dirs: Vec<PathBuf> = Vec::new();
  #[cfg(target_os = "windows")]
  {
    for env_var in ["LOCALAPPDATA"] {
      if let Ok(value) = std::env::var(env_var) {
        if !value.is_empty() {
          dirs.push(PathBuf::from(value).join("Programs").join("BetterMediaInfo"));
        }
      }
    }
    for env_var in ["ProgramFiles", "ProgramFiles(x86)"] {
      if let Ok(value) = std::env::var(env_var) {
        if !value.is_empty() {
          dirs.push(PathBuf::from(value).join("BetterMediaInfo"));
        }
      }
    }
  }
  #[cfg(target_os = "macos")]
  {
    dirs.push(PathBuf::from("/Applications/BetterMediaInfo.app/Contents/MacOS"));
  }
  #[cfg(target_os = "linux")]
  {
    dirs.push(PathBuf::from("/usr/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
  }
  dirs
}

pub fn detect_better_media_info(user_path: String, check_running: bool) -> Result<BetterMediaInfoStatus> {
  if check_running {
    if let Some(dir) = find_running_better_media_info_dir() {
      return Ok(BetterMediaInfoStatus {
        found: true,
        path: dir.to_string_lossy().to_string(),
      });
    }
  }
  let trimmed = user_path.trim();
  if !trimmed.is_empty() {
    if let Some(found) = find_better_media_info_dir(Path::new(trimmed)) {
      return Ok(BetterMediaInfoStatus {
        found: true,
        path: found.to_string_lossy().to_string(),
      });
    }
  }
  for dir in common_better_media_info_dirs() {
    if let Some(found) = find_better_media_info_dir(&dir) {
      return Ok(BetterMediaInfoStatus {
        found: true,
        path: found.to_string_lossy().to_string(),
      });
    }
  }
  Ok(BetterMediaInfoStatus {
    found: false,
    path: String::new(),
  })
}

fn find_better_media_info_dir(path: &Path) -> Option<PathBuf> {
  if !path.exists() {
    return None;
  }
  let exe_name = better_media_info_exe_name();
  if path.is_file() {
    let matches = path
      .file_name()
      .and_then(|n| n.to_str())
      .map(|n| n.eq_ignore_ascii_case(exe_name))
      .unwrap_or(false);
    if matches {
      return path.parent().map(|p| p.to_path_buf());
    }
    return None;
  }
  if path.is_dir() && path.join(exe_name).is_file() {
    return Some(path.to_path_buf());
  }
  None
}

#[cfg(target_os = "macos")]
fn find_macos_app_bundle(bin: &Path) -> Option<PathBuf> {
  // The Mach-O binary lives at `<bundle>.app/Contents/MacOS/BetterMediaInfo`,
  // so the `.app` ancestor is at most three levels up. Walk a bounded number
  // of parents instead of looping unbounded.
  let mut current = bin.parent()?;
  for _ in 0..4 {
    if current.extension().and_then(|s| s.to_str()) == Some("app") {
      return Some(current.to_path_buf());
    }
    current = current.parent()?;
  }
  None
}

fn find_running_better_media_info_dir() -> Option<PathBuf> {
  find_running_process_dir(better_media_info_exe_name())
}

pub fn find_running_process_dir(exe_name: &str) -> Option<PathBuf> {
  let sys = sysinfo::System::new_all();
  for process in sys.processes().values() {
    let name = process.name().to_string_lossy();
    if !name.eq_ignore_ascii_case(exe_name) {
      continue;
    }
    if let Some(exe) = process.exe() {
      if let Some(parent) = exe.parent() {
        return Some(parent.to_path_buf());
      }
    }
  }
  None
}

pub fn get_launch_args() -> Vec<String> {
  std::env::args().skip(1).collect()
}

/// What [`get_media_files`] found in the paths it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaFiles {
  /// The supported media files: a given file as it is, a folder's files
  /// sorted, in the order of the paths.
  pub files: Vec<String>,
  /// What could not be used: a path that does not exist (also what a path
  /// that is not valid UTF-8 becomes once converted), a folder that cannot
  /// be listed, and a file in a folder whose name is not valid UTF-8 (shown
  /// lossily). Files of other types are not counted; they are left out on
  /// purpose.
  pub skipped: Vec<String>,
}

/// The supported media files of dropped, chosen or launch-argument `paths`.
/// A folder contributes the supported files directly inside it (one level,
/// as 0.1.0 did: a subfolder is a separate drop). One path that cannot be
/// read never costs the others; it is listed in [`MediaFiles::skipped`].
pub fn get_media_files(paths: Vec<String>) -> MediaFiles {
  let mut found = MediaFiles::default();
  for input in paths {
    let path = Path::new(input.as_str());
    if !path.exists() {
      log::warn!("Skipped {input}: it does not exist");
      found.skipped.push(input);
      continue;
    }
    if path.is_dir() {
      let listing = match path.read_dir() {
        Ok(listing) => listing,
        Err(error) => {
          log::warn!("Skipped the folder {input}: {error}");
          found.skipped.push(input);
          continue;
        }
      };
      let mut entries: Vec<PathBuf> = listing
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && is_supported_media_path(p))
        .collect();
      entries.sort();
      for p in entries {
        match p.to_str() {
          Some(s) => found.files.push(s.to_owned()),
          None => {
            log::warn!("Skipped {}: its name is not valid UTF-8", p.display());
            found.skipped.push(p.to_string_lossy().into_owned());
          }
        }
      }
    } else if path.is_file() && is_supported_media_path(path) {
      found.files.push(input);
    }
  }
  found
}

/// Opens `paths` in the BetterMediaInfo installed in `dir` (the directory
/// [`detect_better_media_info`] reports).
pub fn launch_better_media_info_from(dir: &str, paths: Vec<String>) -> Result<()> {
  let exe = Path::new(dir.trim()).join(better_media_info_exe_name());
  if !exe.is_file() {
    anyhow::bail!("BetterMediaInfo executable not found at {}", exe.display());
  }

  // On macOS, going through `open` lets Launch Services activate the bundle
  // cleanly — spawning the bundle's Mach-O binary directly causes a brief
  // window flash because the child never goes through proper app activation.
  // BetterMediaInfo reads its files from its launch arguments only, and
  // `open --args` passes them only to an instance it starts, so `-n` starts
  // a new one each time, as spawning the executable does elsewhere.
  #[cfg(target_os = "macos")]
  {
    if let Some(app_bundle) = find_macos_app_bundle(&exe) {
      let mut cmd = std::process::Command::new("/usr/bin/open");
      cmd.args(open_args(&app_bundle, &paths));
      cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
      let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to launch BetterMediaInfo via open: {}", e))?;
      reap(child);
      return Ok(());
    }
  }

  let mut cmd = std::process::Command::new(&exe);
  cmd
    .args(&paths)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
  #[cfg(target_os = "windows")]
  {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
  }
  let child = cmd
    .spawn()
    .map_err(|e| anyhow::anyhow!("Failed to launch BetterMediaInfo: {}", e))?;
  reap(child);
  Ok(())
}

/// The arguments of `/usr/bin/open` that start a new instance of `bundle`
/// with `paths` as its launch arguments.
#[cfg(any(target_os = "macos", test))]
fn open_args(bundle: &Path, paths: &[String]) -> Vec<std::ffi::OsString> {
  let mut args: Vec<std::ffi::OsString> = vec!["-n".into(), "-a".into(), bundle.as_os_str().to_owned()];
  if !paths.is_empty() {
    args.push("--args".into());
    args.extend(paths.iter().map(Into::into));
  }
  args
}

/// Waits for a launched process on a thread of its own, so an ended one does
/// not stay behind as a zombie until the app exits.
fn reap(mut child: std::process::Child) {
  std::thread::spawn(move || {
    let _ = child.wait();
  });
}

/// Whether the exact directory `path` already exists. Used by the output-path
/// dialog to warn that a non-existent path will be created at merge time.
pub fn output_path_exists(path: String) -> Result<bool> {
  Ok(Path::new(&path).is_dir())
}

/// Build the per-invocation [`ParseOptions`] from the persisted config. The
/// persisted setting **wins** over the legacy `BMM_PARSER_BUDGET_MS` env
/// override; the env var is only consulted when the user has not pinned a
/// value through Settings (i.e. when `parser.timeoutMs` matches the default).
/// See [[feedback-parser-timeout]].
pub fn parser_options_from_config(cfg: &config::Config) -> ParseOptions {
  let timeout_ms = if cfg.parser.timeout_ms == config::ConfigParser::DEFAULT_TIMEOUT_MS {
    std::env::var("BMM_PARSER_BUDGET_MS")
      .ok()
      .and_then(|v| v.parse::<u64>().ok())
      // The same range as the config value: 0 would time every parse out,
      // a huge value would switch the budget off.
      .map(|ms| {
        ms.clamp(
          config::ConfigParser::MIN_TIMEOUT_MS,
          config::ConfigParser::MAX_TIMEOUT_MS,
        )
      })
      .unwrap_or_else(|| cfg.parser.effective_timeout_ms())
  } else {
    cfg.parser.effective_timeout_ms()
  };
  ParseOptions {
    timeout_ms,
    subtitle_charset: cfg.parser.subtitle_charset.clone(),
    ..ParseOptions::default()
  }
}

/// Resolve the file's media metadata using the native parser with explicit
/// options, so unit tests can exercise it without touching the config file.
pub fn read_media_metadata(file: String, options: ParseOptions) -> Result<MediaMetadata, ParseError> {
  media_metadata::parse(Path::new(&file), options)
}

/// Resolve a non-colliding merge output path: `<output_dir>/<stem>.mkv`,
/// appending " (1)", " (2)", … when the candidate already exists.  Merging in
/// place collides on the base name (it is the source file), so the increment
/// kicks in automatically.
pub fn resolve_merge_output_path(output_dir: String, source_file: String) -> String {
  resolve_merge_output_path_avoiding(output_dir, source_file, |_| false)
}

/// [`resolve_merge_output_path`] that also passes over every candidate
/// `taken` claims: the outputs of merges that are running but may not have
/// written them yet, which the file system does not show.
pub fn resolve_merge_output_path_avoiding(
  output_dir: String,
  source_file: String,
  taken: impl Fn(&str) -> bool,
) -> String {
  let stem = Path::new(&source_file)
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or("output");
  resolve_named_output_path_avoiding(output_dir, format!("{stem}.mkv"), taken)
}

/// A free path for `file_name` in `output_dir`: the name itself, else with
/// " (1)", " (2)", … before its extension, passing over existing files and
/// every path `taken` claims.
pub fn resolve_named_output_path_avoiding(
  output_dir: String,
  file_name: String,
  taken: impl Fn(&str) -> bool,
) -> String {
  let dir = Path::new(&output_dir);
  let name = Path::new(&file_name);
  let stem = name.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
  let extension = name
    .extension()
    .and_then(|s| s.to_str())
    .map(|extension| format!(".{extension}"))
    .unwrap_or_default();
  let mut counter: u32 = 0;
  loop {
    let name = if counter == 0 {
      format!("{stem}{extension}")
    } else {
      format!("{stem} ({counter}){extension}")
    };
    let candidate = dir.join(&name);
    let path = candidate.to_string_lossy().into_owned();
    if !candidate.exists() && !taken(&path) {
      return path;
    }
    counter += 1;
  }
}

/// The directory a user-set output path names, if it names one (it exists as
/// a directory, or ends with a path separator): the merge writes the input's
/// file name there. `None` when the path names the output file itself.
pub fn override_directory(output_path: &str) -> Option<&str> {
  let names_dir = Path::new(output_path).is_dir() || output_path.ends_with('/') || output_path.ends_with('\\');
  names_dir.then_some(output_path)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cfg_with_timeout(ms: u64) -> config::Config {
    let mut cfg = config::Config::default();
    cfg.parser.timeout_ms = ms;
    cfg
  }

  /// Serialises the tests that read/write the process-global
  /// `BMM_PARSER_BUDGET_MS` env var so they cannot race each other under
  /// cargo's parallel test runner. Poison from a panicking test is tolerated.
  static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

  #[test]
  fn open_starts_a_new_instance_with_the_files_as_arguments() {
    let bundle = Path::new("/Applications/BetterMediaInfo.app");
    let args = |paths: &[&str]| -> Vec<String> {
      let paths: Vec<String> = paths.iter().map(|path| path.to_string()).collect();
      open_args(bundle, &paths)
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
    };
    assert_eq!(
      args(&["/m/a.mkv", "/m/b c.mkv"]),
      [
        "-n",
        "-a",
        "/Applications/BetterMediaInfo.app",
        "--args",
        "/m/a.mkv",
        "/m/b c.mkv"
      ]
    );
    assert_eq!(args(&[]), ["-n", "-a", "/Applications/BetterMediaInfo.app"]);
  }

  #[cfg(unix)]
  #[test]
  fn a_reaped_child_leaves_no_zombie() {
    let child = std::process::Command::new("/bin/sh")
      .arg("-c")
      .arg("exit 0")
      .spawn()
      .unwrap();
    let pid = child.id().to_string();
    reap(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
      let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid])
        .output()
        .unwrap();
      let state = String::from_utf8_lossy(&state.stdout).trim().to_owned();
      if state.is_empty() {
        break;
      }
      assert!(
        std::time::Instant::now() < deadline,
        "process {pid} is still listed as {state:?}"
      );
      std::thread::sleep(std::time::Duration::from_millis(20));
    }
  }

  #[test]
  fn one_unreadable_path_does_not_cost_the_others() {
    let dir = std::env::temp_dir().join(format!("bmm_media_files_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ok")).unwrap();
    std::fs::create_dir_all(dir.join("locked")).unwrap();
    std::fs::write(dir.join("ok").join("b.mkv"), b"").unwrap();
    std::fs::write(dir.join("ok").join("a.srt"), b"").unwrap();
    std::fs::write(dir.join("ok").join("notes.txt"), b"").unwrap();
    std::fs::write(dir.join("locked").join("c.mkv"), b"").unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    // `DirEntry::path` joins with the platform's separator (`\` on Windows).
    let listed = |folder: &str, name: &str| dir.join(folder).join(name).to_string_lossy().into_owned();
    #[cfg(unix)]
    let locked = {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
      // Root reads any folder; then there is nothing to skip.
      std::fs::read_dir(dir.join("locked")).is_err()
    };
    // A folder cannot be made unreadable this way on Windows.
    #[cfg(not(unix))]
    let locked = false;
    let found = get_media_files(vec![path("locked"), path("ok"), path("missing.mkv")]);
    let mut files = vec![listed("ok", "a.srt"), listed("ok", "b.mkv")];
    let mut skipped = vec![path("missing.mkv")];
    if locked {
      skipped.insert(0, path("locked"));
    } else {
      files.insert(0, listed("locked", "c.mkv"));
    }
    assert_eq!(found.files, files);
    assert_eq!(found.skipped, skipped);
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(dir.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn an_output_file_or_directory_is_writable_where_a_merge_can_write_it() {
    let dir = std::env::temp_dir().join("bmm_output_writable");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
    for check in [check_output_file_writable, check_output_dir_writable] {
      assert!(check(path("")).unwrap(), "the directory");
      assert!(check(path("new/sub/Movie.mkv")).unwrap(), "created");
    }
    std::fs::write(dir.join("Movie.mkv"), b"merged").unwrap();
    assert!(
      !check_output_file_writable(path("Movie.mkv")).unwrap(),
      "a merge replaces no file"
    );
    assert_eq!(
      std::fs::read(dir.join("Movie.mkv")).unwrap(),
      b"merged",
      "left as it was"
    );
    assert!(
      !check_output_dir_writable(path("Movie.mkv")).unwrap(),
      "a file is no directory"
    );
    for check in [check_output_file_writable, check_output_dir_writable] {
      assert!(!check(path("Movie.mkv/inside.mkv")).unwrap(), "under a file");
    }
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn a_merge_output_passes_over_existing_and_taken_names() {
    let dir = std::env::temp_dir().join("bmm_output_avoiding");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.to_string_lossy().into_owned();
    let name = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let source = "/inputs/Episode.srt".to_owned();
    assert_eq!(
      resolve_merge_output_path(out.clone(), source.clone()),
      name("Episode.mkv")
    );
    std::fs::write(dir.join("Episode.mkv"), b"").unwrap();
    let taken = name("Episode (1).mkv");
    assert_eq!(
      resolve_merge_output_path_avoiding(out.clone(), source.clone(), |path| path == taken),
      name("Episode (2).mkv"),
      "on disk, then taken by a queued merge"
    );
    assert_eq!(resolve_merge_output_path(out, source), name("Episode (1).mkv"));
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn an_override_names_a_directory_when_it_is_one_or_ends_with_a_separator() {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    assert_eq!(override_directory(&dir), Some(dir.as_str()));
    // A path that does not exist names a directory only with a separator.
    let base = std::env::temp_dir().join("bmm_override_does_not_exist");
    let with_sep = format!("{}{}", base.to_string_lossy(), std::path::MAIN_SEPARATOR);
    assert_eq!(override_directory(&with_sep), Some(with_sep.as_str()));
    assert_eq!(override_directory("/nonexistent_dir_bmm/custom.mkv"), None);
  }

  #[test]
  fn a_named_output_is_numbered_before_its_extension_while_taken() {
    let dir = std::env::temp_dir().join("bmm_output_named");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.to_string_lossy().into_owned();
    let name = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let resolve = |taken: &str| {
      let taken = taken.to_string();
      resolve_named_output_path_avoiding(out.clone(), "Movie.en.srt".to_owned(), move |path| path == taken)
    };
    assert_eq!(resolve(""), name("Movie.en.srt"));
    std::fs::write(dir.join("Movie.en.srt"), b"").unwrap();
    assert_eq!(resolve(&name("Movie.en (1).srt")), name("Movie.en (2).srt"));
    let bare = resolve_named_output_path_avoiding(out, "README".to_owned(), |_| false);
    assert_eq!(bare, name("README"));
    let _ = std::fs::remove_dir_all(&dir);
  }

  #[test]
  fn parser_options_from_config_clamps_pinned_value() {
    let cfg = cfg_with_timeout(1);
    let opts = parser_options_from_config(&cfg);
    assert_eq!(opts.timeout_ms, config::ConfigParser::MIN_TIMEOUT_MS);
  }

  #[test]
  fn parser_options_from_config_default_uses_default_timeout() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = cfg_with_timeout(config::ConfigParser::DEFAULT_TIMEOUT_MS);
    unsafe {
      std::env::remove_var("BMM_PARSER_BUDGET_MS");
    }
    let opts = parser_options_from_config(&cfg);
    assert_eq!(opts.timeout_ms, config::ConfigParser::DEFAULT_TIMEOUT_MS);
  }

  #[test]
  fn parser_options_from_config_honours_pinned_value_over_env() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = cfg_with_timeout(2500);
    unsafe {
      std::env::set_var("BMM_PARSER_BUDGET_MS", "9999");
    }
    let opts = parser_options_from_config(&cfg);
    // Always restore so other tests see a clean environment.
    unsafe {
      std::env::remove_var("BMM_PARSER_BUDGET_MS");
    }
    assert_eq!(opts.timeout_ms, 2500);
  }

  #[test]
  fn the_env_override_is_clamped_like_the_config_value() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = cfg_with_timeout(config::ConfigParser::DEFAULT_TIMEOUT_MS);
    let with_env = |value: &str| {
      unsafe {
        std::env::set_var("BMM_PARSER_BUDGET_MS", value);
      }
      let timeout = parser_options_from_config(&cfg).timeout_ms;
      unsafe {
        std::env::remove_var("BMM_PARSER_BUDGET_MS");
      }
      timeout
    };
    assert_eq!(with_env("0"), config::ConfigParser::MIN_TIMEOUT_MS);
    assert_eq!(with_env("999999999"), config::ConfigParser::MAX_TIMEOUT_MS);
    assert_eq!(with_env("2500"), 2500);
    assert_eq!(with_env("soon"), config::ConfigParser::DEFAULT_TIMEOUT_MS);
  }

  #[test]
  fn read_media_metadata_propagates_io_error_for_missing_file() {
    let opts = ParseOptions::default();
    let err = read_media_metadata("this-file-does-not-exist-12345.mkv".to_owned(), opts).unwrap_err();
    assert!(matches!(err, ParseError::Io { .. }));
  }

  #[test]
  fn launching_better_media_info_from_a_directory_without_it_fails() {
    let dir = std::env::temp_dir().join("bmm_no_better_media_info_here");
    let err = launch_better_media_info_from(&dir.to_string_lossy(), vec!["/media/a.mkv".to_owned()]).unwrap_err();
    assert!(err.to_string().contains("executable not found"), "{err}");
  }
}
