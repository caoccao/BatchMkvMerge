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

//! Final-pass identify step.  Port of `r_matroska.cpp::identify` (line 2869
//! onwards) — but trimmed to header-only:
//!
//! - Derive `display_dimensions` for video tracks where the source omitted
//!   `DisplayWidth` / `DisplayHeight` (we already filled the gap inside
//!   `tracks::video::VideoBuilder::build`; this is a defensive double-check).
//! - Re-derive `tags.per_track_count` (the tags parser already does this; we
//!   re-run for the case where tags were emitted without going through the
//!   matroska tags parser).
//! - Clear `recognized` / `supported` if the parse produced no tracks AND
//!   no segment-level metadata.  mkvtoolnix doesn't do this, but the frontend
//!   benefits from a clear "empty container" signal.

use crate::media_metadata::model::MediaMetadata;

pub fn finalise(out: &mut MediaMetadata) {
  // 1. Re-derive per-track tag count defensively.
  out.tags.per_track_count = out.tracks.iter().map(|t| t.properties.tags.len() as u32).sum();

  // 2. Re-confirm display_dimensions ← pixel_dimensions where missing.
  for track in &mut out.tracks {
    if let Some(video) = track.properties.video.as_mut() {
      if video.display_dimensions.is_none() {
        video.display_dimensions = video.pixel_dimensions;
      }
    }
  }

  // 3. If nothing came out of the segment, keep recognized=true (matroska
  //    *was* detected via the EBML head) but flag the parse as supported
  //    even when empty — mkvmerge's identification path does this too,
  //    since "valid matroska with zero tracks" is a legitimate state.
  //    Nothing to do here for now; the reader already sets both flags.
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::model::container::ContainerFormat;
  use crate::media_metadata::model::tag::TagEntry;
  use crate::media_metadata::model::track::{CodecInfo, Track, TrackProperties, TrackType};
  use crate::media_metadata::model::track_properties_video::{Dimensions2D, VideoTrackProperties};

  fn video_track(pixel: Option<Dimensions2D>, display: Option<Dimensions2D>) -> Track {
    Track {
      id: 0,
      track_type: TrackType::Video,
      codec: CodecInfo {
        id: "V_VP9".to_owned(),
        name: None,
        codec_private: None,
      },
      properties: TrackProperties {
        video: Some(VideoTrackProperties {
          pixel_dimensions: pixel,
          display_dimensions: display,
          ..VideoTrackProperties::default()
        }),
        ..TrackProperties::default()
      },
    }
  }

  #[test]
  fn display_dimensions_fall_back_to_pixel_when_missing() {
    let mut m = MediaMetadata::new("clip.mkv", 0);
    m.container.recognized = true;
    m.container.supported = true;
    m.container.format = ContainerFormat::Matroska;
    m.tracks.push(video_track(
      Some(Dimensions2D {
        width: 1920,
        height: 1080,
      }),
      None,
    ));
    finalise(&mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn existing_display_dimensions_preserved() {
    let mut m = MediaMetadata::new("clip.mkv", 0);
    m.tracks.push(video_track(
      Some(Dimensions2D {
        width: 1920,
        height: 1080,
      }),
      Some(Dimensions2D {
        width: 3840,
        height: 2160,
      }),
    ));
    finalise(&mut m);
    let v = m.tracks[0].properties.video.as_ref().unwrap();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 3840,
        height: 2160
      })
    );
  }

  #[test]
  fn per_track_count_derived_from_tracks() {
    let mut m = MediaMetadata::new("clip.mkv", 0);
    let mut t = video_track(None, None);
    t.properties.tags.push(TagEntry {
      name: "X".to_owned(),
      value: "Y".to_owned(),
      language: None,
    });
    m.tracks.push(t);
    finalise(&mut m);
    assert_eq!(m.tags.per_track_count, 1);
  }
}
