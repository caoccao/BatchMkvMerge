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

//! Video TrackEntry sub-tree.  Port of
//! `r_matroska.cpp::read_headers_track_video` (lines 1268-1350) plus the
//! Colour + Projection nested walks.
//!
//! The Matroska Video sub-element is the only place where most colour /
//! mastering metadata lives.  We preserve the full nested hierarchy in the
//! emitted `VideoTrackProperties` per [[feedback-protocol-shape]].

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::track_properties_video::{
  AlphaMode, Chromaticity, ColorMetadata, ColorRange, CropRect, Dimensions2D, DisplayUnit, FieldOrder, InterlaceFlag,
  MasterMetadata, ProjectionMetadata, ProjectionPose, ProjectionType, StereoMode, VideoTrackProperties,
};

use crate::media_metadata::matroska::ebml::{self, ChildAction, ElementHeader};
use crate::media_metadata::matroska::ids;

#[derive(Debug, Default)]
pub struct VideoBuilder {
  pub pixel_width: Option<u32>,
  pub pixel_height: Option<u32>,
  pub display_width: Option<u32>,
  pub display_height: Option<u32>,
  pub display_unit: Option<DisplayUnit>,
  pub crop_left: Option<u32>,
  pub crop_top: Option<u32>,
  pub crop_right: Option<u32>,
  pub crop_bottom: Option<u32>,
  pub alpha_mode: Option<AlphaMode>,
  pub field_order: Option<FieldOrder>,
  pub interlace: Option<InterlaceFlag>,
  pub stereo_mode: Option<StereoMode>,
  pub colour: Option<ColorMetadata>,
  /// Raw bytes of `KaxVideoColourSpace` (0x2EB524) when present — typically a
  /// 4-byte FOURCC for uncompressed video tracks (PARSER-068).
  pub color_space: Option<Vec<u8>>,
  pub projection: Option<ProjectionMetadata>,
  pub default_duration_ns: Option<u64>,
  pub frame_rate: Option<f64>,
}

impl VideoBuilder {
  pub fn build(self) -> VideoTrackProperties {
    let pixel_dimensions = match (self.pixel_width, self.pixel_height) {
      (Some(w), Some(h)) => Some(Dimensions2D { width: w, height: h }),
      _ => None,
    };
    // Display dimensions default to pixel dimensions when absent — matches
    // mkvtoolnix's `find_child_value(..., track->v_width)` behaviour.
    let (raw_dw, raw_dh) = (
      self.display_width.or(self.pixel_width),
      self.display_height.or(self.pixel_height),
    );
    // PARSER-066: certain muxers abuse DisplayWidth/Height to carry just
    // an aspect ratio (e.g. 16/9).  Port
    // `kax_track_t::fix_display_dimension_parameters` in
    // `r_matroska.cpp:283-300` to rescale those values back to the
    // pixel-dimension range when the heuristic matches.
    let (fixed_dw, fixed_dh) = fix_display_dimensions(
      self.pixel_width,
      self.pixel_height,
      self.display_width,
      self.display_height,
      self.display_unit,
    );
    let display_dimensions = match (fixed_dw.or(raw_dw), fixed_dh.or(raw_dh)) {
      (Some(w), Some(h)) => Some(Dimensions2D { width: w, height: h }),
      _ => None,
    };
    let crop =
      if self.crop_left.is_some() || self.crop_top.is_some() || self.crop_right.is_some() || self.crop_bottom.is_some()
      {
        Some(CropRect {
          left: self.crop_left.unwrap_or(0),
          top: self.crop_top.unwrap_or(0),
          right: self.crop_right.unwrap_or(0),
          bottom: self.crop_bottom.unwrap_or(0),
        })
      } else {
        None
      };

    // Derive default_duration from FrameRate if the explicit
    // DefaultDuration was absent (mkvtoolnix r_matroska.cpp:1341-1343).
    let default_duration_ns = self.default_duration_ns.or_else(|| {
      self
        .frame_rate
        .filter(|f| *f > 0.0)
        .map(|f| (1_000_000_000.0 / f) as u64)
    });

    VideoTrackProperties {
      pixel_dimensions,
      display_dimensions,
      display_unit: self.display_unit,
      crop,
      color: self.colour,
      color_space_hex: self.color_space.as_deref().map(hex_encode),
      projection: self.projection,
      stereo_mode: self.stereo_mode,
      alpha_mode: self.alpha_mode,
      field_order: self.field_order,
      interlace: self.interlace,
      default_duration_ns,
      codec_config: None,
      rotation_degrees: None,
      flipped: None,
      block_addition_mappings: Vec::new(),
    }
  }
}

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  builder: &mut VideoBuilder,
) -> Result<(), ParseError> {
  ebml::walk_children(src, parent, "matroska::track_video", deadline, |src, child| {
    match child.id {
      ids::VIDEO_PIXEL_WIDTH => {
        builder.pixel_width = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PIXEL_HEIGHT => {
        builder.pixel_height = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_DISPLAY_WIDTH => {
        builder.display_width = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_DISPLAY_HEIGHT => {
        builder.display_height = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_DISPLAY_UNIT => {
        builder.display_unit = Some(classify_display_unit(ebml::read_uint(src, child)?));
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PIXEL_CROP_LEFT => {
        builder.crop_left = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PIXEL_CROP_RIGHT => {
        builder.crop_right = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PIXEL_CROP_TOP => {
        builder.crop_top = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PIXEL_CROP_BOTTOM => {
        builder.crop_bottom = Some(ebml::read_uint(src, child)? as u32);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_ALPHA_MODE => {
        let v = ebml::read_uint(src, child)?;
        builder.alpha_mode = Some(if v != 0 { AlphaMode::Present } else { AlphaMode::None });
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_FLAG_INTERLACED => {
        let v = ebml::read_uint(src, child)?;
        builder.interlace = Some(match v {
          1 => InterlaceFlag::Interlaced,
          2 => InterlaceFlag::Progressive,
          _ => InterlaceFlag::Unknown,
        });
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_FIELD_ORDER => {
        let v = ebml::read_uint(src, child)?;
        builder.field_order = Some(match v {
          0 => FieldOrder::Progressive,
          1 | 9 => FieldOrder::Tff,
          6 | 14 => FieldOrder::Bff,
          _ => FieldOrder::Undetermined,
        });
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_STEREO_MODE => {
        builder.stereo_mode = Some(classify_stereo_mode(ebml::read_uint(src, child)?));
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_FRAME_RATE => {
        builder.frame_rate = Some(ebml::read_float(src, child)?);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_COLOUR => {
        builder.colour = Some(parse_colour(src, child, deadline)?);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_COLOR_SPACE => {
        // PARSER-068: raw colour-space identifier (FOURCC for
        // uncompressed video).  PARSER-319: mkvtoolnix clones the whole
        // element payload, so use the shared parser element-size budget
        // instead of a local cap.
        let bytes = ebml::read_binary(src, child, deadline.max_element_size())?;
        builder.color_space = Some(bytes);
        Ok(ChildAction::Consumed)
      }
      ids::VIDEO_PROJECTION => {
        builder.projection = Some(parse_projection(src, child, deadline)?);
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    }
  })
}

fn parse_colour(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
) -> Result<ColorMetadata, ParseError> {
  let mut colour = ColorMetadata::default();
  ebml::walk_children(src, parent, "matroska::colour", deadline, |src, child| {
    match child.id {
      ids::VIDEO_COLOUR_MATRIX => {
        colour.matrix_coefficients = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_BITS_PER_CHANNEL => {
        colour.bits_per_channel = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_CHROMA_SUBSAMP_HORZ => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.chroma_subsampling.unwrap_or_default();
        s.horizontal = v;
        colour.chroma_subsampling = Some(s);
      }
      ids::VIDEO_CHROMA_SUBSAMP_VERT => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.chroma_subsampling.unwrap_or_default();
        s.vertical = v;
        colour.chroma_subsampling = Some(s);
      }
      ids::VIDEO_CB_SUBSAMP_HORZ => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.cb_subsampling.unwrap_or_default();
        s.horizontal = v;
        colour.cb_subsampling = Some(s);
      }
      ids::VIDEO_CB_SUBSAMP_VERT => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.cb_subsampling.unwrap_or_default();
        s.vertical = v;
        colour.cb_subsampling = Some(s);
      }
      ids::VIDEO_CHROMA_SIT_HORZ => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.chroma_siting.unwrap_or_default();
        s.horizontal = v;
        colour.chroma_siting = Some(s);
      }
      ids::VIDEO_CHROMA_SIT_VERT => {
        let v = ebml::read_uint(src, child)? as u32;
        let mut s = colour.chroma_siting.unwrap_or_default();
        s.vertical = v;
        colour.chroma_siting = Some(s);
      }
      ids::VIDEO_COLOUR_RANGE => {
        let raw = ebml::read_uint(src, child)? as u32;
        colour.range_raw = Some(raw);
        colour.range = match raw {
          0 => Some(ColorRange::Unspecified),
          1 => Some(ColorRange::Broadcast),
          2 => Some(ColorRange::Full),
          3 => Some(ColorRange::MatrixDerived),
          _ => None,
        };
      }
      ids::VIDEO_COLOUR_TRANSFER_CHARACTER => {
        colour.transfer_characteristics = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_COLOUR_PRIMARIES => {
        colour.primaries = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_COLOUR_MAX_CLL => {
        colour.max_cll = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_COLOUR_MAX_FALL => {
        colour.max_fall = Some(ebml::read_uint(src, child)? as u32);
      }
      ids::VIDEO_COLOUR_MASTER_META => {
        colour.master = Some(parse_mastering_metadata(src, child, deadline)?);
        return Ok(ChildAction::Consumed);
      }
      _ => return Ok(ChildAction::Skip),
    }
    Ok(ChildAction::Consumed)
  })?;
  Ok(colour)
}

fn parse_mastering_metadata(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
) -> Result<MasterMetadata, ParseError> {
  let mut m = MasterMetadata::default();
  let mut r = Chromaticity::default();
  let mut g = Chromaticity::default();
  let mut b = Chromaticity::default();
  let mut wp = Chromaticity::default();
  let mut have_r = false;
  let mut have_g = false;
  let mut have_b = false;
  let mut have_wp = false;
  ebml::walk_children(src, parent, "matroska::master_meta", deadline, |src, child| {
    match child.id {
      ids::VIDEO_R_CHROMA_X => {
        r.x = ebml::read_float(src, child)?;
        have_r = true;
      }
      ids::VIDEO_R_CHROMA_Y => {
        r.y = ebml::read_float(src, child)?;
        have_r = true;
      }
      ids::VIDEO_G_CHROMA_X => {
        g.x = ebml::read_float(src, child)?;
        have_g = true;
      }
      ids::VIDEO_G_CHROMA_Y => {
        g.y = ebml::read_float(src, child)?;
        have_g = true;
      }
      ids::VIDEO_B_CHROMA_X => {
        b.x = ebml::read_float(src, child)?;
        have_b = true;
      }
      ids::VIDEO_B_CHROMA_Y => {
        b.y = ebml::read_float(src, child)?;
        have_b = true;
      }
      ids::VIDEO_WHITE_POINT_CHROMA_X => {
        wp.x = ebml::read_float(src, child)?;
        have_wp = true;
      }
      ids::VIDEO_WHITE_POINT_CHROMA_Y => {
        wp.y = ebml::read_float(src, child)?;
        have_wp = true;
      }
      ids::VIDEO_LUMINANCE_MAX => {
        m.luminance_max = Some(ebml::read_float(src, child)?);
      }
      ids::VIDEO_LUMINANCE_MIN => {
        m.luminance_min = Some(ebml::read_float(src, child)?);
      }
      _ => return Ok(ChildAction::Skip),
    }
    Ok(ChildAction::Consumed)
  })?;
  if have_r {
    m.primary_r = Some(r);
  }
  if have_g {
    m.primary_g = Some(g);
  }
  if have_b {
    m.primary_b = Some(b);
  }
  if have_wp {
    m.white_point = Some(wp);
  }
  Ok(m)
}

fn parse_projection(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
) -> Result<ProjectionMetadata, ParseError> {
  let mut p = ProjectionMetadata::default();
  let mut pose = ProjectionPose::default();
  let mut have_pose = false;
  ebml::walk_children(src, parent, "matroska::projection", deadline, |src, child| {
    match child.id {
      ids::VIDEO_PROJECTION_TYPE => {
        let raw = ebml::read_uint(src, child)? as u32;
        p.kind_raw = Some(raw);
        p.kind = match raw {
          0 => Some(ProjectionType::Rectangular),
          1 => Some(ProjectionType::Equirectangular),
          2 => Some(ProjectionType::Cubemap),
          3 => Some(ProjectionType::Mesh),
          _ => None,
        };
      }
      ids::VIDEO_PROJECTION_POSE_YAW => {
        pose.yaw = ebml::read_float(src, child)?;
        have_pose = true;
      }
      ids::VIDEO_PROJECTION_POSE_PITCH => {
        pose.pitch = ebml::read_float(src, child)?;
        have_pose = true;
      }
      ids::VIDEO_PROJECTION_POSE_ROLL => {
        pose.roll = ebml::read_float(src, child)?;
        have_pose = true;
      }
      ids::VIDEO_PROJECTION_PRIVATE => {
        let bytes = ebml::read_binary(src, child, deadline.max_element_size())?;
        p.private_hex = Some(hex_encode(&bytes));
      }
      _ => return Ok(ChildAction::Skip),
    }
    Ok(ChildAction::Consumed)
  })?;
  if have_pose {
    p.pose = Some(pose);
  }
  Ok(p)
}

/// Port of `kax_track_t::fix_display_dimension_parameters` in
/// `r_matroska.cpp:283-300`.  Returns `(fixed_dw, fixed_dh)` when the rescue
/// heuristic matches, else `(None, None)`.  Gates:
///
/// - DisplayUnit must be absent or 0 (pixels).  Anything else is a real
///   display-unit and must be preserved.
/// - Both pixel and display dimensions must be present.
/// - `8 * dwidth <= pixel_width` *and* `8 * dheight <= pixel_height` — i.e. the
///   declared dimensions are at least 8× smaller than the raster.
/// - `gcd(dwidth, dheight) == 1` — the values look like a coprime aspect.
///
/// When all checks pass we expand back into the pixel-dimension range while
/// preserving the aspect ratio.  We deliberately port the literal C++ logic
/// (including the order-of-assignment in the else branch).
fn fix_display_dimensions(
  pixel_width: Option<u32>,
  pixel_height: Option<u32>,
  display_width: Option<u32>,
  display_height: Option<u32>,
  display_unit: Option<DisplayUnit>,
) -> (Option<u32>, Option<u32>) {
  let pw = match pixel_width {
    Some(v) if v > 0 => v as u64,
    _ => return (None, None),
  };
  let ph = match pixel_height {
    Some(v) if v > 0 => v as u64,
    _ => return (None, None),
  };
  let dw = match display_width {
    Some(v) if v > 0 => v as u64,
    _ => return (None, None),
  };
  let dh = match display_height {
    Some(v) if v > 0 => v as u64,
    _ => return (None, None),
  };
  // Display unit absent or explicit Pixels (= 0).  Mkvtoolnix gates on
  // `v_display_unit == 0`, treating absence as the default.
  match display_unit {
    None | Some(DisplayUnit::Pixels) => {}
    _ => return (None, None),
  }
  if 8 * dw > pw || 8 * dh > ph {
    return (None, None);
  }
  if gcd(dw, dh) != 1 {
    return (None, None);
  }
  if dw > dh {
    if (ph * dw) % dh == 0 {
      return (Some((ph * dw / dh) as u32), Some(ph as u32));
    }
  } else if (pw * dh) % dw == 0 {
    // Faithful port of `r_matroska.cpp:296-299`.  The original C++
    // assigns `v_dwidth = v_width` *before* computing `v_dheight =
    // v_width * v_dheight / v_dwidth`, which makes the dheight formula
    // collapse to its input.  We replicate the visible outcome:
    // `dwidth` jumps to `pixel_width`, `dheight` is unchanged.
    let _ = (pw * dh) / dw; // formula left here for documentation
    return (Some(pw as u32), Some(dh as u32));
  }
  (None, None)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
  while b != 0 {
    let t = a % b;
    a = b;
    b = t;
  }
  a
}

fn classify_display_unit(v: u64) -> DisplayUnit {
  match v {
    0 => DisplayUnit::Pixels,
    1 => DisplayUnit::Centimetres,
    2 => DisplayUnit::Inches,
    3 => DisplayUnit::DisplayAspectRatio,
    _ => DisplayUnit::Unknown,
  }
}

fn classify_stereo_mode(v: u64) -> StereoMode {
  match v {
    0 => StereoMode::Mono,
    1 => StereoMode::SideBySideLeftFirst,
    2 => StereoMode::TopBottomRightFirst,
    3 => StereoMode::TopBottomLeftFirst,
    4 => StereoMode::Checkerboard,
    5 | 6 => StereoMode::RowInterleaved,
    7 | 8 => StereoMode::ColumnInterleaved,
    9 => StereoMode::AnaglyphCyanRed,
    10 => StereoMode::SideBySideRightFirst,
    11 => StereoMode::AnaglyphGreenMagenta,
    _ => StereoMode::Other,
  }
}

fn hex_encode(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{:02x}", b));
  }
  s
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_float, encode_element_uint};
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_video(payload: Vec<u8>) -> (Vec<u8>, ElementHeader, FileSource) {
    let bytes = encode_element(ids::TRACK_VIDEO, 1, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    let header = ebml::read_element_header(&mut s).unwrap();
    (bytes, header, s)
  }

  #[test]
  fn pixel_dimensions_set_when_both_present() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920));
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(
      v.pixel_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
    // Display defaults to pixel when display_* absent
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn display_dimensions_override_pixel() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1440));
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_WIDTH, 2, 1920));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_HEIGHT, 2, 1080));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  // ---- PARSER-066: rescue aspect-ratio-style display dimensions -------

  #[test]
  fn display_dimensions_expanded_when_used_as_aspect_ratio() {
    // 1920x1080 raster with DisplayWidth/Height = 16/9 → rescaled to
    // (1920, 1080) (mkvtoolnix `fix_display_dimension_parameters`).
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920));
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_WIDTH, 2, 16));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_HEIGHT, 2, 9));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn display_dimensions_unchanged_when_explicit_unit_is_dar() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920));
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_WIDTH, 2, 16));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_HEIGHT, 2, 9));
    // DisplayUnit = 3 (DisplayAspectRatio) — fix-up must NOT touch us.
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_UNIT, 2, 3));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(v.display_dimensions, Some(Dimensions2D { width: 16, height: 9 }));
  }

  #[test]
  fn display_dimensions_preserved_when_not_aspect_ratio_shaped() {
    // 1440x1080 raster with DisplayWidth/Height = 1920/1080 — explicit
    // anamorphic display, gcd != 1.  Leave alone.
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1440));
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_HEIGHT, 1, 1080));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_WIDTH, 2, 1920));
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_HEIGHT, 2, 1080));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(
      v.display_dimensions,
      Some(Dimensions2D {
        width: 1920,
        height: 1080
      })
    );
  }

  #[test]
  fn frame_rate_derives_default_duration_when_absent() {
    let mut payload = Vec::new();
    payload.extend(encode_element_float(ids::VIDEO_FRAME_RATE, 3, 24.0));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(v.default_duration_ns, Some(41_666_666));
  }

  #[test]
  fn crop_emits_struct_when_any_field_set() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_PIXEL_CROP_LEFT, 2, 8));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    assert_eq!(
      v.crop,
      Some(CropRect {
        left: 8,
        top: 0,
        right: 0,
        bottom: 0
      })
    );
  }

  #[test]
  fn colour_extracts_matrix_and_range() {
    let mut colour_payload = Vec::new();
    colour_payload.extend(encode_element_uint(ids::VIDEO_COLOUR_MATRIX, 2, 9));
    colour_payload.extend(encode_element_uint(ids::VIDEO_COLOUR_RANGE, 2, 2));
    colour_payload.extend(encode_element_uint(ids::VIDEO_BITS_PER_CHANNEL, 2, 10));
    let colour = encode_element(ids::VIDEO_COLOUR, 2, &colour_payload);
    let mut payload = Vec::new();
    payload.extend(colour);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    let c = v.color.unwrap();
    assert_eq!(c.matrix_coefficients, Some(9));
    assert_eq!(c.range, Some(ColorRange::Full));
    assert_eq!(c.bits_per_channel, Some(10));
  }

  #[test]
  fn colour_master_metadata_populated_when_present() {
    let mut m_payload = Vec::new();
    m_payload.extend(encode_element_float(ids::VIDEO_R_CHROMA_X, 2, 0.708));
    m_payload.extend(encode_element_float(ids::VIDEO_R_CHROMA_Y, 2, 0.292));
    m_payload.extend(encode_element_float(ids::VIDEO_LUMINANCE_MAX, 2, 1000.0));
    let master = encode_element(ids::VIDEO_COLOUR_MASTER_META, 2, &m_payload);
    let colour = encode_element(ids::VIDEO_COLOUR, 2, &master);
    let (_b, h, mut s) = build_video(colour);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let m = builder.build().color.unwrap().master.unwrap();
    assert!(m.primary_r.is_some());
    assert_eq!(m.luminance_max, Some(1000.0));
  }

  #[test]
  fn projection_type_decoded() {
    let mut p_payload = Vec::new();
    p_payload.extend(encode_element_uint(ids::VIDEO_PROJECTION_TYPE, 2, 1));
    p_payload.extend(encode_element_float(ids::VIDEO_PROJECTION_POSE_YAW, 2, -90.0));
    let proj = encode_element(ids::VIDEO_PROJECTION, 2, &p_payload);
    let (_b, h, mut s) = build_video(proj);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let p = builder.build().projection.unwrap();
    assert_eq!(p.kind, Some(ProjectionType::Equirectangular));
    assert_eq!(p.pose.unwrap().yaw, -90.0);
  }

  #[test]
  fn stereo_mode_byte_decodes() {
    assert_eq!(classify_stereo_mode(0), StereoMode::Mono);
    assert_eq!(classify_stereo_mode(1), StereoMode::SideBySideLeftFirst);
    assert_eq!(classify_stereo_mode(10), StereoMode::SideBySideRightFirst);
    assert_eq!(classify_stereo_mode(99), StereoMode::Other);
  }

  #[test]
  fn display_unit_byte_decodes() {
    assert_eq!(classify_display_unit(0), DisplayUnit::Pixels);
    assert_eq!(classify_display_unit(3), DisplayUnit::DisplayAspectRatio);
    assert_eq!(classify_display_unit(42), DisplayUnit::Unknown);
  }

  #[test]
  fn field_order_decoded() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_FIELD_ORDER, 1, 1));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().field_order, Some(FieldOrder::Tff));
  }

  #[test]
  fn interlace_flag_decoded() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_FLAG_INTERLACED, 1, 1));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().interlace, Some(InterlaceFlag::Interlaced));
  }

  #[test]
  fn interlace_flag_two_is_progressive() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_FLAG_INTERLACED, 1, 2));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().interlace, Some(InterlaceFlag::Progressive));
  }

  #[test]
  fn interlace_flag_unknown_value_classified_as_unknown() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_FLAG_INTERLACED, 1, 42));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().interlace, Some(InterlaceFlag::Unknown));
  }

  #[test]
  fn alpha_mode_decoded() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_ALPHA_MODE, 2, 1));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().alpha_mode, Some(AlphaMode::Present));
  }

  #[test]
  fn alpha_mode_zero_is_none() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_ALPHA_MODE, 2, 0));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().alpha_mode, Some(AlphaMode::None));
  }

  #[test]
  fn field_order_variants_decoded() {
    for (raw, expected) in [
      (0u64, FieldOrder::Progressive),
      (1, FieldOrder::Tff),
      (9, FieldOrder::Tff),
      (6, FieldOrder::Bff),
      (14, FieldOrder::Bff),
      (12, FieldOrder::Undetermined),
    ] {
      let payload = encode_element_uint(ids::VIDEO_FIELD_ORDER, 1, raw);
      let (_b, h, mut s) = build_video(payload);
      let mut builder = VideoBuilder::default();
      parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
      assert_eq!(builder.build().field_order, Some(expected), "raw={raw}");
    }
  }

  #[test]
  fn colour_full_chroma_and_cb_subsampling() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_CHROMA_SUBSAMP_HORZ, 2, 2));
    payload.extend(encode_element_uint(ids::VIDEO_CHROMA_SUBSAMP_VERT, 2, 2));
    payload.extend(encode_element_uint(ids::VIDEO_CB_SUBSAMP_HORZ, 2, 1));
    payload.extend(encode_element_uint(ids::VIDEO_CB_SUBSAMP_VERT, 2, 1));
    payload.extend(encode_element_uint(ids::VIDEO_CHROMA_SIT_HORZ, 2, 1));
    payload.extend(encode_element_uint(ids::VIDEO_CHROMA_SIT_VERT, 2, 1));
    payload.extend(encode_element_uint(ids::VIDEO_COLOUR_PRIMARIES, 2, 9));
    payload.extend(encode_element_uint(ids::VIDEO_COLOUR_TRANSFER_CHARACTER, 2, 16));
    payload.extend(encode_element_uint(ids::VIDEO_COLOUR_MAX_CLL, 2, 1000));
    payload.extend(encode_element_uint(ids::VIDEO_COLOUR_MAX_FALL, 2, 400));
    let colour = encode_element(ids::VIDEO_COLOUR, 2, &payload);
    let (_b, h, mut s) = build_video(colour);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let c = builder.build().color.unwrap();
    assert_eq!(c.chroma_subsampling.unwrap().horizontal, 2);
    assert_eq!(c.chroma_subsampling.unwrap().vertical, 2);
    assert_eq!(c.cb_subsampling.unwrap().horizontal, 1);
    assert_eq!(c.cb_subsampling.unwrap().vertical, 1);
    assert_eq!(c.chroma_siting.unwrap().horizontal, 1);
    assert_eq!(c.chroma_siting.unwrap().vertical, 1);
    assert_eq!(c.primaries, Some(9));
    assert_eq!(c.transfer_characteristics, Some(16));
    assert_eq!(c.max_cll, Some(1000));
    assert_eq!(c.max_fall, Some(400));
  }

  #[test]
  fn colour_range_variants_decoded() {
    for (raw, expected) in [
      (0u64, Some(ColorRange::Unspecified)),
      (1, Some(ColorRange::Broadcast)),
      (2, Some(ColorRange::Full)),
      (3, Some(ColorRange::MatrixDerived)),
      (99, None),
    ] {
      let payload = encode_element_uint(ids::VIDEO_COLOUR_RANGE, 2, raw);
      let colour = encode_element(ids::VIDEO_COLOUR, 2, &payload);
      let (_b, h, mut s) = build_video(colour);
      let mut builder = VideoBuilder::default();
      parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
      let color = builder.build().color.unwrap();
      assert_eq!(color.range, expected);
      assert_eq!(color.range_raw, Some(raw as u32));
    }
  }

  #[test]
  fn master_metadata_full_set_of_primaries() {
    let mut m_payload = Vec::new();
    m_payload.extend(encode_element_float(ids::VIDEO_R_CHROMA_X, 2, 0.708));
    m_payload.extend(encode_element_float(ids::VIDEO_R_CHROMA_Y, 2, 0.292));
    m_payload.extend(encode_element_float(ids::VIDEO_G_CHROMA_X, 2, 0.170));
    m_payload.extend(encode_element_float(ids::VIDEO_G_CHROMA_Y, 2, 0.797));
    m_payload.extend(encode_element_float(ids::VIDEO_B_CHROMA_X, 2, 0.131));
    m_payload.extend(encode_element_float(ids::VIDEO_B_CHROMA_Y, 2, 0.046));
    m_payload.extend(encode_element_float(ids::VIDEO_WHITE_POINT_CHROMA_X, 2, 0.3127));
    m_payload.extend(encode_element_float(ids::VIDEO_WHITE_POINT_CHROMA_Y, 2, 0.329));
    m_payload.extend(encode_element_float(ids::VIDEO_LUMINANCE_MIN, 2, 0.0001));
    let master = encode_element(ids::VIDEO_COLOUR_MASTER_META, 2, &m_payload);
    let colour = encode_element(ids::VIDEO_COLOUR, 2, &master);
    let (_b, h, mut s) = build_video(colour);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let m = builder.build().color.unwrap().master.unwrap();
    assert!(m.primary_r.is_some());
    assert!(m.primary_g.is_some());
    assert!(m.primary_b.is_some());
    assert!(m.white_point.is_some());
    assert_eq!(m.luminance_min, Some(0.0001));
  }

  #[test]
  fn projection_types_and_pose_full_set() {
    for (raw, expected) in [
      (0u64, Some(ProjectionType::Rectangular)),
      (1, Some(ProjectionType::Equirectangular)),
      (2, Some(ProjectionType::Cubemap)),
      (3, Some(ProjectionType::Mesh)),
      (99, None),
    ] {
      let mut p = Vec::new();
      p.extend(encode_element_uint(ids::VIDEO_PROJECTION_TYPE, 2, raw));
      p.extend(encode_element_float(ids::VIDEO_PROJECTION_POSE_PITCH, 2, 45.0));
      p.extend(encode_element_float(ids::VIDEO_PROJECTION_POSE_ROLL, 2, 12.0));
      p.extend(encode_element(ids::VIDEO_PROJECTION_PRIVATE, 2, &[0xAA, 0xBB]));
      let proj = encode_element(ids::VIDEO_PROJECTION, 2, &p);
      let (_b, h, mut s) = build_video(proj);
      let mut builder = VideoBuilder::default();
      parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
      let p = builder.build().projection.unwrap();
      assert_eq!(p.kind, expected);
      assert_eq!(p.kind_raw, Some(raw as u32));
      assert!(p.pose.is_some());
      assert_eq!(p.private_hex.as_deref(), Some("aabb"));
    }
  }

  #[test]
  fn stereo_mode_full_table() {
    for (raw, expected) in [
      (0u64, StereoMode::Mono),
      (1, StereoMode::SideBySideLeftFirst),
      (2, StereoMode::TopBottomRightFirst),
      (3, StereoMode::TopBottomLeftFirst),
      (4, StereoMode::Checkerboard),
      (5, StereoMode::RowInterleaved),
      (6, StereoMode::RowInterleaved),
      (7, StereoMode::ColumnInterleaved),
      (8, StereoMode::ColumnInterleaved),
      (9, StereoMode::AnaglyphCyanRed),
      (10, StereoMode::SideBySideRightFirst),
      (11, StereoMode::AnaglyphGreenMagenta),
    ] {
      assert_eq!(classify_stereo_mode(raw), expected, "raw={raw}");
    }
  }

  #[test]
  fn display_unit_full_table() {
    for (raw, expected) in [
      (0u64, DisplayUnit::Pixels),
      (1, DisplayUnit::Centimetres),
      (2, DisplayUnit::Inches),
      (3, DisplayUnit::DisplayAspectRatio),
    ] {
      assert_eq!(classify_display_unit(raw), expected);
    }
  }

  #[test]
  fn pixel_dimensions_none_when_only_one_present() {
    let payload = encode_element_uint(ids::VIDEO_PIXEL_WIDTH, 1, 1920);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let v = builder.build();
    // pixel_dimensions requires both width AND height
    assert!(v.pixel_dimensions.is_none());
  }

  #[test]
  fn explicit_default_duration_takes_precedence_over_frame_rate() {
    // Builder is set both directly and via frame rate — the explicit
    // value wins (mkvtoolnix behaviour: KaxTrackDefaultDuration is read
    // before fix_display_dimension_parameters).
    let mut builder = VideoBuilder::default();
    builder.default_duration_ns = Some(123_456);
    builder.frame_rate = Some(24.0);
    let v = builder.build();
    assert_eq!(v.default_duration_ns, Some(123_456));
  }

  #[test]
  fn stereo_mode_serialized_through_full_parse() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_STEREO_MODE, 2, 1));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().stereo_mode, Some(StereoMode::SideBySideLeftFirst));
  }

  #[test]
  fn display_unit_value_from_element_propagates() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::VIDEO_DISPLAY_UNIT, 2, 3));
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().display_unit, Some(DisplayUnit::DisplayAspectRatio));
  }

  // ---- PARSER-068: KaxVideoColourSpace ------------------------------

  #[test]
  fn colour_space_fourcc_round_trips() {
    // VIDEO_COLOR_SPACE id 0x2EB524 needs width=3 encoding.
    let payload = encode_element(ids::VIDEO_COLOR_SPACE, 3, b"YV12");
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().color_space_hex.as_deref(), Some("59563132"));
  }

  #[test]
  fn colour_space_uses_shared_video_binary_cap() {
    let payload = encode_element(ids::VIDEO_COLOR_SPACE, 3, &[0xAB; 32]);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(builder.build().color_space_hex.as_ref().unwrap().len(), 64);
  }

  #[test]
  fn projection_private_uses_shared_video_binary_cap() {
    let large_private = vec![0xCD; 4 * 1024 * 1024 + 1];
    let private = encode_element(ids::VIDEO_PROJECTION_PRIVATE, 2, &large_private);
    let proj = encode_element(ids::VIDEO_PROJECTION, 2, &private);
    let (_b, h, mut s) = build_video(proj);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    assert_eq!(
      builder.build().projection.unwrap().private_hex.unwrap().len(),
      large_private.len() * 2
    );
  }

  #[test]
  fn crop_right_individually() {
    let payload = encode_element_uint(ids::VIDEO_PIXEL_CROP_RIGHT, 2, 16);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let c = builder.build().crop.unwrap();
    assert_eq!(c.right, 16);
  }

  #[test]
  fn crop_top_individually() {
    let payload = encode_element_uint(ids::VIDEO_PIXEL_CROP_TOP, 2, 8);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let c = builder.build().crop.unwrap();
    assert_eq!(c.top, 8);
  }

  #[test]
  fn crop_bottom_individually() {
    let payload = encode_element_uint(ids::VIDEO_PIXEL_CROP_BOTTOM, 2, 4);
    let (_b, h, mut s) = build_video(payload);
    let mut builder = VideoBuilder::default();
    parse(&mut s, &h, &no_deadline(), &mut builder).unwrap();
    let c = builder.build().crop.unwrap();
    assert_eq!(c.bottom, 4);
  }
}
