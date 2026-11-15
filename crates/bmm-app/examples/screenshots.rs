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

//! Renders the screenshots of `docs/screenshots/` from synthetic files:
//!
//! ```bash
//! cargo run --release -p BatchMkvMerge --example screenshots
//! cargo run --release -p BatchMkvMerge --example screenshots -- --out <dir>
//! ```
//!
//! With `--review <dir>` it renders the visual-pass set instead: every locale
//! through every tab, the toolbar menus, a dialog and every settings page at
//! the 600×450 minimum window size, and every theme in light and dark.
//!
//! The app's root view runs in a real window (GPUI draws only for a window)
//! and each picture is the window's frame from `render_to_image` (macOS; the
//! other platforms report that they cannot render offscreen). The files are
//! never read: a stand-in parser serves their metadata, merges are recorded
//! instead of run, and the config lives in memory, so the user's config file
//! is never read or written.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use batch_mkvmerge::merge_events::MergeBackend;
use batch_mkvmerge::platform::taskbar::Taskbar;
use batch_mkvmerge::state::ui::ToolbarMenu;
use batch_mkvmerge::state::{MetadataSource, Persistence, Services, Stores, TabType};
use batch_mkvmerge::tools::{ToolDetector, ToolStatus};
use batch_mkvmerge::ui::root::AppView;
use bmm_core::config::{Config, DisplayMode, GroupMode, Language};
use bmm_core::media_metadata::language::Language as TrackLanguage;
use bmm_core::media_metadata::model::attachment::Attachment;
use bmm_core::media_metadata::model::metadata::MediaMetadata;
use bmm_core::media_metadata::model::tag::TagEntry;
use bmm_core::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
use bmm_core::media_metadata::model::track_properties_audio::AudioTrackProperties;
use bmm_core::media_metadata::model::track_properties_common::TrackFlag;
use bmm_core::media_metadata::model::track_properties_subtitle::SubtitleTrackProperties;
use bmm_core::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};
use bmm_core::merge::MergeEvent;
use gpui_kit::test::TestWindowExt;
use gpui_kit::*;

/// Where the synthetic files live; shown on the cards and in the queue.
const MEDIA_DIR: &str = "/Volumes/Media";
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB: f64 = 1024.0 * 1024.0;
/// 23.976 fps.
const FILM_FRAME_NS: u64 = 41_708_333;

fn media(name: &str) -> String {
  format!("{MEDIA_DIR}/{name}")
}

fn stats(bytes: f64, bits_per_second: f64) -> Vec<TagEntry> {
  [
    ("BPS", format!("{}", bits_per_second.round())),
    ("NUMBER_OF_BYTES", format!("{}", bytes.round())),
  ]
  .into_iter()
  .map(|(name, value)| TagEntry {
    name: name.to_string(),
    value,
    language: None,
  })
  .collect()
}

fn track(
  id: i64,
  track_type: TrackType,
  (codec_id, codec_name): (&str, &str),
  (language, name, default): (&str, &str, bool),
  tags: Vec<TagEntry>,
) -> Track {
  let mut properties = TrackProperties::default();
  properties.common.number = Some(id as u64 + 1);
  properties.common.language = Some(TrackLanguage::from_iso_639_2(language));
  properties.common.track_name = Some(name.to_string());
  properties.common.default = if default { TrackFlag::True } else { TrackFlag::False };
  properties.tags = tags;
  Track {
    id,
    track_type,
    codec: CodecInfo {
      id: codec_id.to_string(),
      name: Some(codec_name.to_string()),
      codec_private: None,
    },
    properties,
  }
}

fn hevc(id: i64, (width, height): (u32, u32), tags: Vec<TagEntry>) -> Track {
  let mut track = track(
    id,
    TrackType::Video,
    ("V_MPEGH/ISO/HEVC", "HEVC/H.265/MPEG-H p2"),
    ("eng", "English", true),
    tags,
  );
  track.properties.video = Some(VideoTrackProperties {
    pixel_dimensions: Some(Dimensions2D { width, height }),
    default_duration_ns: Some(FILM_FRAME_NS),
    ..VideoTrackProperties::default()
  });
  track
}

fn audio(id: i64, codec: (&str, &str), channels: u32, language: (&str, &str, bool), tags: Vec<TagEntry>) -> Track {
  let mut track = track(id, TrackType::Audio, codec, language, tags);
  track.properties.audio = Some(AudioTrackProperties {
    sampling_frequency: Some(48_000.0),
    channels: Some(channels),
    ..AudioTrackProperties::default()
  });
  track
}

fn pgs(id: i64, language: (&str, &str, bool), tags: Vec<TagEntry>) -> Track {
  let mut track = track(id, TrackType::Subtitles, ("S_HDMV/PGS", "HDMV PGS"), language, tags);
  track.properties.subtitle = Some(SubtitleTrackProperties {
    text_subtitles: false,
    variant: Some("PGS".to_string()),
    ..SubtitleTrackProperties::default()
  });
  track
}

fn metadata(name: &str, tracks: Vec<Track>, attachments: Vec<Attachment>) -> MediaMetadata {
  let size: u64 = tracks
    .iter()
    .flat_map(|track| &track.properties.tags)
    .filter(|tag| tag.name == "NUMBER_OF_BYTES")
    .filter_map(|tag| tag.value.parse::<u64>().ok())
    .sum();
  let mut metadata = MediaMetadata::new(name, size + 8 * 1024 * 1024);
  metadata.container.recognized = true;
  metadata.container.supported = true;
  metadata.container.properties.date_utc = Some("2026-06-06T08:16:22Z".to_string());
  metadata.tracks = tracks;
  metadata.attachments = attachments;
  metadata
}

/// A film with eight tracks and an attachment, on its own card.
fn feature() -> MediaMetadata {
  metadata(
    "test.01.mkv",
    vec![
      hevc(0, (3840, 2076), stats(25.6 * GIB, 25.28e6)),
      audio(
        1,
        ("A_DTS", "DTS"),
        2,
        ("eng", "English", true),
        stats(1.71 * GIB, 1.69e6),
      ),
      audio(
        2,
        ("A_AC3", "AC-3"),
        1,
        ("fre", "French", false),
        stats(189.9 * MIB, 187.5e3),
      ),
      pgs(3, ("eng", "English", true), stats(52.19 * MIB, 52.28e3)),
      pgs(4, ("fre", "French", false), stats(36.95 * MIB, 37.15e3)),
      pgs(5, ("ger", "German", false), stats(42.94 * MIB, 43.17e3)),
      pgs(6, ("spa", "Spanish", false), stats(45.26 * MIB, 45.5e3)),
      pgs(7, ("chi", "Simplified Chinese", false), stats(35.73 * MIB, 35.91e3)),
    ],
    vec![Attachment {
      id: 0,
      file_name: "notes.xml".to_string(),
      mime_type: Some("text/xml".to_string()),
      description: None,
      size: 2_048,
      uid_hex: None,
    }],
  )
}

/// Two episodes with the same tracks: one group card.
fn episode(name: &str, scale: f64) -> MediaMetadata {
  metadata(
    name,
    vec![
      hevc(0, (1920, 1080), stats(627.95 * MIB * scale, 10.54e6 * scale)),
      audio(
        1,
        ("A_AC3", "AC-3"),
        2,
        ("eng", "English", true),
        stats(10.9 * MIB, 187.5e3),
      ),
      pgs(2, ("eng", "English", true), stats(4.02 * MIB * scale, 69.99e3 * scale)),
      pgs(3, ("fre", "French", false), stats(3.69 * MIB * scale, 63.92e3 * scale)),
      pgs(4, ("ger", "German", false), stats(3.76 * MIB * scale, 65.41e3 * scale)),
      pgs(5, ("spa", "Spanish", false), stats(3.6 * MIB * scale, 62.65e3 * scale)),
    ],
    Vec::new(),
  )
}

fn files() -> Vec<(String, MediaMetadata)> {
  vec![
    (media("test.01.mkv"), feature()),
    (media("test.02.mkv"), episode("test.02.mkv", 1.0)),
    (media("test.03.mkv"), episode("test.03.mkv", 0.97)),
  ]
}

/// English, light Ocean; every track selected, nothing automated.
fn base_config(language: Language) -> Config {
  let mut config = Config {
    language,
    display_mode: DisplayMode::Light,
    group_mode: GroupMode::TrackCountAndLanguage,
    group_by_file_name: false,
    ..Config::default()
  };
  let profile = &mut config.profiles[0];
  profile.select_video = true;
  profile.select_audio = true;
  profile.select_subtitle = true;
  profile.select_attachments = true;
  profile.video_languages_for_track_selection.clear();
  profile.audio_languages_for_track_selection.clear();
  profile.subtitle_languages_for_track_selection.clear();
  config
}

/// The stand-in services: no file is read, no merge runs, no config is saved.
fn services() -> Services {
  let fixtures = files();
  Services {
    persistence: Persistence::new(|_| {}),
    metadata: MetadataSource::new(move |file, _| {
      fixtures
        .iter()
        .find(|(path, _)| path == file)
        .map(|(_, metadata)| metadata.clone())
        .ok_or(bmm_core::media_metadata::ParseError::Unrecognised)
    }),
    merges: MergeBackend::new(|_, _| Ok(()), |_| Ok(())),
    taskbar: Taskbar::none(),
    tools: ToolDetector::new(|_, path, _| ToolStatus {
      found: true,
      path: path.to_string(),
    }),
  }
}

/// A window of `size` showing the app's root view for `config`.
fn open(config: &Config, size: Size<Pixels>, cx: &mut App) -> (AnyWindowHandle, Stores) {
  batch_mkvmerge::i18n::set_language(config.language);
  batch_mkvmerge::theme::apply(config, cx.window_appearance(), cx);
  let stores = Stores::new(config.clone(), services(), cx);
  let options = WindowOptions {
    window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(40.0), px(40.0)), size))),
    ..Default::default()
  };
  let view_stores = stores.clone();
  let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
    cx.new(|cx| AppView::new(view_stores, window, cx))
  })
  .expect("a window");
  (handle, stores)
}

struct Shooter {
  dir: PathBuf,
  written: usize,
}

impl Shooter {
  /// Renders the window and saves it as `<dir>/<name>.png`.
  fn shot(&mut self, handle: AnyWindowHandle, name: &str, cx: &mut AsyncApp) {
    let path = self.dir.join(format!("{name}.png"));
    let saved = handle.update(cx, |_, window, cx| {
      // Cached cards measure their height in one frame and draw at it in the
      // next.
      window.render_frame(cx);
      window.render_frame(cx);
      window.render_frame(cx);
      window
        .render_to_image()
        .map_err(|error| error.to_string())
        .and_then(|image| image.save(&path).map_err(|error| error.to_string()))
    });
    match saved {
      Ok(Ok(())) => self.written += 1,
      Ok(Err(error)) => eprintln!("{name}: {error}"),
      Err(error) => eprintln!("{name}: {error}"),
    }
  }
}

async fn pause(cx: &mut AsyncApp, millis: u64) {
  cx.background_executor().timer(Duration::from_millis(millis)).await;
}

/// Adds the three files and waits until they are parsed.
async fn add_files(stores: &Stores, cx: &mut AsyncApp) {
  cx.update(|cx| {
    stores.files.update(cx, |store, cx| {
      store.add_files(files().into_iter().map(|(path, _)| path), cx)
    })
  });
  pause(cx, 400).await;
}

/// Merge All, then the first file merging for 55 s at 17 % (the others wait).
async fn start_merges(stores: &Stores, cx: &mut AsyncApp) {
  cx.update(|cx| batch_mkvmerge::actions::merge_all(stores, cx));
  pause(cx, 200).await;
  let file = media("test.01.mkv");
  let now = SystemTime::now();
  cx.update(|cx| {
    stores.queue.update(cx, |queue, cx| {
      let started = MergeEvent::Started {
        file: file.clone(),
        at: now - Duration::from_secs(55),
      };
      queue.apply_event(&started, now, cx);
      queue.apply_event(&MergeEvent::Progress { file, percent: 17 }, now, cx);
    })
  });
  pause(cx, 200).await;
}

/// Clicks the settings sidebar entry of page `index` (the entries have no
/// label a query can find; the id is gpui-kit's `0-<page>`).
fn settings_page(handle: AnyWindowHandle, index: usize, cx: &mut AsyncApp) {
  settings_entry(handle, &format!("0-{index}"), cx);
}

/// Clicks a settings sidebar entry: `0-<page>`, or `0-<page>-<group>` for a
/// titled group, which scrolls the page to it.
fn settings_entry(handle: AnyWindowHandle, id: &str, cx: &mut AsyncApp) {
  let name = SharedString::from(id.to_string());
  let _ = handle.update(cx, |_, window, cx| {
    window.render_frame(cx);
    match window.within("settings").try_find(name) {
      // Left of the centre: a page entry's chevron toggles its groups instead.
      Some(entry) => click(window, entry.bounds().center() - point(px(40.0), px(0.0)), cx),
      None => eprintln!("no settings entry {id}"),
    }
  });
}

/// Clicks the element with `id`, if it is shown.
fn click_id(handle: AnyWindowHandle, id: &'static str, cx: &mut AsyncApp) {
  let _ = handle.update(cx, |_, window, cx| {
    window.render_frame(cx);
    if window.try_find(id).is_some() {
      window.click(id, cx);
    } else {
      eprintln!("no element {id}");
    }
  });
}

/// Closes and reopens the Settings tab: a fresh view shows the first page
/// with every sidebar entry collapsed, so each entry is in reach even at the
/// minimum window size.
async fn reopen_settings(stores: &Stores, cx: &mut AsyncApp) {
  cx.update(|cx| stores.ui.update(cx, |ui, cx| ui.close_tab(TabType::Settings, cx)));
  pause(cx, 200).await;
  set_tab(stores, TabType::Settings, cx);
  pause(cx, 400).await;
}

fn click(window: &mut Window, position: Point<Pixels>, cx: &mut App) {
  window.dispatch_event(
    PlatformInput::MouseDown(MouseDownEvent {
      button: MouseButton::Left,
      position,
      modifiers: Modifiers::none(),
      click_count: 1,
      first_mouse: false,
    }),
    cx,
  );
  window.dispatch_event(
    PlatformInput::MouseUp(MouseUpEvent {
      button: MouseButton::Left,
      position,
      modifiers: Modifiers::none(),
      click_count: 1,
    }),
    cx,
  );
}

fn update_config(stores: &Stores, cx: &mut AsyncApp, edit: impl FnOnce(&mut Config)) {
  cx.update(|cx| stores.config.update(cx, |store, cx| store.update(cx, edit)));
}

fn set_tab(stores: &Stores, tab: TabType, cx: &mut AsyncApp) {
  cx.update(|cx| {
    stores.ui.update(cx, |ui, cx| match tab {
      TabType::Settings => ui.open_settings(cx),
      TabType::About => ui.open_about(cx),
      tab => ui.set_active_tab(tab, cx),
    })
  });
}

fn close(handle: AnyWindowHandle, cx: &mut AsyncApp) {
  let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// The four pictures of `docs/screenshots/`, at twice the window size.
async fn docs(shooter: &mut Shooter, cx: &mut AsyncApp) {
  let config = base_config(Language::EnUS);
  let (handle, stores) = cx.update(|cx| open(&config, size(px(1200.0), px(800.0)), cx));
  pause(cx, 500).await;
  add_files(&stores, cx).await;
  shooter.shot(handle, "card_view_grouping", cx);
  start_merges(&stores, cx).await;
  shooter.shot(handle, "card_view_merging", cx);
  let _ = handle.update(cx, |_, window, _| window.resize(size(px(1200.0), px(330.0))));
  set_tab(&stores, TabType::Queue, cx);
  pause(cx, 300).await;
  shooter.shot(handle, "queue", cx);
  // The automation options the screenshot has always shown.
  update_config(&stores, cx, |config| {
    let automation = &mut config.profiles[0].automation;
    automation.reset_und_language.enabled = true;
    automation.reset_und_language.language = "en".to_string();
    automation.set_track_name.enabled = true;
    automation.reset_default_track.enabled = true;
    automation.reset_forced_display.enabled = true;
  });
  let _ = handle.update(cx, |_, window, _| window.resize(size(px(1200.0), px(480.0))));
  set_tab(&stores, TabType::Settings, cx);
  pause(cx, 400).await;
  settings_page(handle, 4, cx);
  pause(cx, 400).await;
  // Profiles → Automation, the fourth titled group.
  settings_entry(handle, "0-4-3", cx);
  // Long enough for the scrollbar shown by the jump to fade out.
  pause(cx, 2500).await;
  shooter.shot(handle, "settings_profile_automation", cx);
  close(handle, cx);
}

/// Every locale through every tab at the minimum window size, then every
/// theme in light and dark.
async fn review(shooter: &mut Shooter, cx: &mut AsyncApp) {
  let languages = [
    Language::EnUS,
    Language::De,
    Language::Es,
    Language::Fr,
    Language::It,
    Language::Ja,
    Language::ZhCN,
    Language::ZhHK,
    Language::ZhTW,
  ];
  let pages = ["appearance", "formatting", "group", "integration", "profiles", "update"];
  for language in languages {
    let code = batch_mkvmerge::i18n::locale_code(language);
    let config = base_config(language);
    let (handle, stores) = cx.update(|cx| open(&config, size(px(600.0), px(450.0)), cx));
    pause(cx, 500).await;
    shooter.shot(handle, &format!("{code}-00-welcome"), cx);
    add_files(&stores, cx).await;
    shooter.shot(handle, &format!("{code}-01-files"), cx);
    for (menu, name) in [
      (ToolbarMenu::GroupMode, "group-menu"),
      (ToolbarMenu::Profile, "profile-menu"),
    ] {
      cx.update(|cx| stores.ui.update(cx, |ui, cx| ui.show_menu(menu, cx)));
      pause(cx, 300).await;
      shooter.shot(handle, &format!("{code}-02-{name}"), cx);
      cx.update(|cx| stores.ui.update(cx, |ui, cx| ui.hide_menu(menu, cx)));
      pause(cx, 200).await;
    }
    start_merges(&stores, cx).await;
    shooter.shot(handle, &format!("{code}-03-merging"), cx);
    set_tab(&stores, TabType::Queue, cx);
    pause(cx, 300).await;
    shooter.shot(handle, &format!("{code}-04-queue"), cx);
    for (index, page) in pages.iter().enumerate() {
      reopen_settings(&stores, cx).await;
      settings_page(handle, index, cx);
      pause(cx, 400).await;
      shooter.shot(handle, &format!("{code}-1{index}-{page}"), cx);
    }
    // Profiles' four titled groups, through their entries.
    reopen_settings(&stores, cx).await;
    settings_page(handle, 4, cx);
    pause(cx, 400).await;
    for group in 0..4 {
      settings_entry(handle, &format!("0-4-{group}"), cx);
      pause(cx, 400).await;
      shooter.shot(handle, &format!("{code}-14-profiles-{group}"), cx);
    }
    reopen_settings(&stores, cx).await;
    settings_page(handle, 4, cx);
    pause(cx, 400).await;
    click_id(handle, "settings-profile-reset", cx);
    pause(cx, 300).await;
    shooter.shot(handle, &format!("{code}-16-reset-question"), cx);
    let _ = handle.update(cx, |_, window, cx| window.press("escape", cx));
    pause(cx, 200).await;
    set_tab(&stores, TabType::About, cx);
    pause(cx, 300).await;
    shooter.shot(handle, &format!("{code}-20-about"), cx);
    close(handle, cx);
    pause(cx, 200).await;
  }

  let config = base_config(Language::EnUS);
  let (handle, stores) = cx.update(|cx| open(&config, size(px(1010.0), px(660.0)), cx));
  pause(cx, 500).await;
  add_files(&stores, cx).await;
  for spec in &batch_mkvmerge::theme::THEMES {
    for display_mode in [DisplayMode::Light, DisplayMode::Dark] {
      update_config(&stores, cx, |config| {
        config.theme = spec.theme;
        config.display_mode = display_mode;
      });
      pause(cx, 300).await;
      let mode = if display_mode == DisplayMode::Dark {
        "dark"
      } else {
        "light"
      };
      shooter.shot(handle, &format!("theme-{}-{mode}", spec.name.to_lowercase()), cx);
    }
  }
  close(handle, cx);
}

fn main() {
  let args: Vec<String> = std::env::args().skip(1).collect();
  let (review_mode, dir) = match args.as_slice() {
    [] => (
      false,
      Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots"),
    ),
    [flag, dir] if flag == "--out" => (false, PathBuf::from(dir)),
    [flag, dir] if flag == "--review" => (true, PathBuf::from(dir)),
    _ => {
      eprintln!("usage: screenshots [--out <dir> | --review <dir>]");
      std::process::exit(2);
    }
  };
  std::fs::create_dir_all(&dir).expect("the output directory");
  let dir = dir.canonicalize().unwrap_or(dir);
  gpui_kit::application()
    .with_assets(batch_mkvmerge::assets::AppAssets)
    .run(move |cx| {
      batch_mkvmerge::i18n::install_component_strings();
      gpui_kit::init(cx);
      batch_mkvmerge::theme::install(cx);
      cx.activate(true);
      cx.spawn(async move |cx| {
        let mut shooter = Shooter { dir, written: 0 };
        if review_mode {
          review(&mut shooter, cx).await;
        } else {
          docs(&mut shooter, cx).await;
        }
        println!("{} pictures in {}", shooter.written, shooter.dir.display());
        cx.update(|cx| cx.quit());
      })
      .detach();
    });
}
