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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

/// Video-track-only properties.  Populated only on tracks whose `trackType` is
/// `Video`.  All sub-domains stay nested — see [[feedback-protocol-shape]].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VideoTrackProperties {
    /// Encoded pixel dimensions (the raster).
    pub pixel_dimensions: Option<Dimensions2D>,
    /// Intended display dimensions (PixelWidth/Height after PAR adjustment).
    pub display_dimensions: Option<Dimensions2D>,
    /// DisplayUnit — Matroska element 0x54B2.  Most files leave it implicit.
    pub display_unit: Option<DisplayUnit>,
    pub crop: Option<CropRect>,
    pub color: Option<ColorMetadata>,
    /// Matroska `KaxVideoColourSpace` (0x2EB524) — raw FOURCC for uncompressed
    /// tracks.  Hex-encoded so non-printable bytes round-trip safely.
    pub color_space_hex: Option<String>,
    pub projection: Option<ProjectionMetadata>,
    pub stereo_mode: Option<StereoMode>,
    pub alpha_mode: Option<AlphaMode>,
    pub field_order: Option<FieldOrder>,
    pub interlace: Option<InterlaceFlag>,
    /// Frame duration in nanoseconds.  When known we expose it as a typed u64;
    /// frontend derives the implied frame rate.
    #[specta(type = Option<Number>)]
    pub default_duration_ns: Option<u64>,
    pub codec_config: Option<VideoCodecConfig>,
    /// Display rotation, in degrees (0/90/180/270).  Derived from the MP4
    /// tkhd display matrix when present; mirrors mkvtoolnix's
    /// `mtx::qtmp4::compute_rotation_from_matrix`.  PARSER-069.
    pub rotation_degrees: Option<u32>,
    /// `true` when the source container signals a horizontal flip.  Derived
    /// from a negative-determinant tkhd matrix.
    pub flipped: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Dimensions2D {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CropRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DisplayUnit {
    Pixels,
    Centimetres,
    Inches,
    DisplayAspectRatio,
    Unknown,
}

/// Decoded `Colour` element (Matroska) / `colr` box (MP4) / colr-related H.273
/// signalling — typed sub-fields, never flattened into the parent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ColorMetadata {
    pub matrix_coefficients: Option<u32>,
    pub transfer_characteristics: Option<u32>,
    pub primaries: Option<u32>,
    pub range: Option<ColorRange>,
    pub bits_per_channel: Option<u32>,
    pub chroma_subsampling: Option<ChromaSubsampling>,
    pub cb_subsampling: Option<ChromaSubsampling>,
    pub chroma_siting: Option<ChromaSiting>,
    pub max_cll: Option<u32>,
    pub max_fall: Option<u32>,
    pub master: Option<MasterMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ColorRange {
    Unspecified,
    Broadcast,
    Full,
    MatrixDerived,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChromaSubsampling {
    pub horizontal: u32,
    pub vertical: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChromaSiting {
    pub horizontal: u32,
    pub vertical: u32,
}

/// CIE 1931 (x,y) chromaticity coordinate.  Range [0, 1].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Chromaticity {
    pub x: f64,
    pub y: f64,
}

/// SMPTE ST 2086 mastering display metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MasterMetadata {
    pub primary_r: Option<Chromaticity>,
    pub primary_g: Option<Chromaticity>,
    pub primary_b: Option<Chromaticity>,
    pub white_point: Option<Chromaticity>,
    /// Maximum luminance in cd/m² (nits).
    pub luminance_max: Option<f64>,
    pub luminance_min: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionMetadata {
    pub kind: Option<ProjectionType>,
    pub pose: Option<ProjectionPose>,
    /// Codec-private projection bytes, hex-encoded.
    pub private_hex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ProjectionType {
    Rectangular,
    Equirectangular,
    Cubemap,
    Mesh,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionPose {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StereoMode {
    Mono,
    SideBySideLeftFirst,
    SideBySideRightFirst,
    TopBottomLeftFirst,
    TopBottomRightFirst,
    Checkerboard,
    RowInterleaved,
    ColumnInterleaved,
    AnaglyphCyanRed,
    AnaglyphGreenMagenta,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AlphaMode {
    None,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FieldOrder {
    Progressive,
    Tff,
    Bff,
    Undetermined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum InterlaceFlag {
    Progressive,
    Interlaced,
    Unknown,
}

/// Decoded codec-private blob for video tracks.  Fields are optional because
/// not every codec populates every slot; the protocol shape is stable as we
/// add codec-specific work in later phases.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VideoCodecConfig {
    pub profile_name: Option<String>,
    pub profile_idc: Option<u32>,
    pub level_name: Option<String>,
    pub level_idc: Option<u32>,
    pub tier: Option<HevcTier>,
    pub bit_depth_luma: Option<u32>,
    pub bit_depth_chroma: Option<u32>,
    pub chroma_format: Option<ChromaFormat>,
    pub coded_dimensions: Option<Dimensions2D>,
    pub sample_aspect_ratio: Option<SampleAspectRatio>,
    /// Original codec-private bytes, hex-encoded.  Kept for transparency in
    /// downstream tooling.
    pub raw_hex: Option<String>,
    /// `true` when the source provided a raw elementary stream (no container
    /// wrap, e.g. `.h264` / `.265` / `.av1` file), `false` for in-container
    /// payloads.  Mirrors mkvmerge's `mpeg4_p10_es_video` / `mpegh_p2_es_video`.
    pub is_elementary_stream: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum HevcTier {
    Main,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChromaFormat {
    Monochrome,
    Yuv420,
    Yuv422,
    Yuv444,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SampleAspectRatio {
    pub num: u32,
    pub den: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_none() {
        let v = VideoTrackProperties::default();
        assert!(v.pixel_dimensions.is_none());
        assert!(v.color.is_none());
        assert!(v.codec_config.is_none());
    }

    #[test]
    fn dimensions_round_trip() {
        let d = Dimensions2D {
            width: 3840,
            height: 2160,
        };
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(s, "{\"width\":3840,\"height\":2160}");
        let back: Dimensions2D = serde_json::from_str(&s).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn crop_round_trip() {
        let c = CropRect {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        };
        let back: CropRect = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn color_metadata_round_trip_with_master() {
        let color = ColorMetadata {
            matrix_coefficients: Some(9),
            transfer_characteristics: Some(16),
            primaries: Some(9),
            range: Some(ColorRange::Full),
            bits_per_channel: Some(10),
            chroma_subsampling: Some(ChromaSubsampling {
                horizontal: 2,
                vertical: 2,
            }),
            cb_subsampling: None,
            chroma_siting: None,
            max_cll: Some(1000),
            max_fall: Some(400),
            master: Some(MasterMetadata {
                primary_r: Some(Chromaticity { x: 0.708, y: 0.292 }),
                primary_g: Some(Chromaticity { x: 0.170, y: 0.797 }),
                primary_b: Some(Chromaticity { x: 0.131, y: 0.046 }),
                white_point: Some(Chromaticity {
                    x: 0.3127,
                    y: 0.329,
                }),
                luminance_max: Some(1000.0),
                luminance_min: Some(0.0001),
            }),
        };
        let s = serde_json::to_string(&color).unwrap();
        assert!(s.contains("\"matrixCoefficients\":9"));
        assert!(s.contains("\"range\":\"full\""));
        assert!(s.contains("\"chromaSubsampling\":{\"horizontal\":2,\"vertical\":2}"));
        assert!(s.contains("\"master\":{"));
        let back: ColorMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back, color);
    }

    #[test]
    fn projection_round_trip() {
        let p = ProjectionMetadata {
            kind: Some(ProjectionType::Equirectangular),
            pose: Some(ProjectionPose {
                yaw: -90.0,
                pitch: 0.0,
                roll: 0.0,
            }),
            private_hex: Some("00112233".to_owned()),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"kind\":\"equirectangular\""));
        let back: ProjectionMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn stereo_mode_round_trip() {
        for mode in [
            StereoMode::Mono,
            StereoMode::SideBySideLeftFirst,
            StereoMode::SideBySideRightFirst,
            StereoMode::TopBottomLeftFirst,
            StereoMode::TopBottomRightFirst,
            StereoMode::Checkerboard,
            StereoMode::RowInterleaved,
            StereoMode::ColumnInterleaved,
            StereoMode::AnaglyphCyanRed,
            StereoMode::AnaglyphGreenMagenta,
            StereoMode::Other,
        ] {
            let back: StereoMode =
                serde_json::from_str(&serde_json::to_string(&mode).unwrap()).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn codec_config_round_trip() {
        let cfg = VideoCodecConfig {
            profile_name: Some("High".to_owned()),
            profile_idc: Some(100),
            level_name: Some("4.0".to_owned()),
            level_idc: Some(40),
            tier: None,
            bit_depth_luma: Some(8),
            bit_depth_chroma: Some(8),
            chroma_format: Some(ChromaFormat::Yuv420),
            coded_dimensions: Some(Dimensions2D {
                width: 1920,
                height: 1088,
            }),
            sample_aspect_ratio: Some(SampleAspectRatio { num: 1, den: 1 }),
            raw_hex: Some("0164001fffe10018".to_owned()),
            is_elementary_stream: Some(false),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        assert!(s.contains("\"profileName\":\"High\""));
        assert!(s.contains("\"chromaFormat\":\"yuv420\""));
        assert!(s.contains("\"isElementaryStream\":false"));
        let back: VideoCodecConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn display_unit_serializes_camel_case() {
        let s = serde_json::to_string(&DisplayUnit::DisplayAspectRatio).unwrap();
        assert_eq!(s, "\"displayAspectRatio\"");
    }

    #[test]
    fn field_order_round_trip() {
        for f in [
            FieldOrder::Progressive,
            FieldOrder::Tff,
            FieldOrder::Bff,
            FieldOrder::Undetermined,
        ] {
            let back: FieldOrder =
                serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
            assert_eq!(back, f);
        }
    }

    #[test]
    fn hevc_tier_round_trip() {
        let s = serde_json::to_string(&HevcTier::High).unwrap();
        assert_eq!(s, "\"high\"");
        let back: HevcTier = serde_json::from_str(&s).unwrap();
        assert_eq!(back, HevcTier::High);
    }
}
