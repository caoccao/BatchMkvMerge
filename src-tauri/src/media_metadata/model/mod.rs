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

//! Wire-format data model for the media-metadata parser.
//!
//! Every struct here is camelCase on the wire and preserves its nested
//! hierarchy — never flattened.  See [[feedback-protocol-shape]] and plan §4.

pub mod attachment;
pub mod chapter;
pub mod container;
pub mod duration;
pub mod metadata;
pub mod playlist;
pub mod program;
pub mod tag;
pub mod track;
pub mod track_properties_audio;
pub mod track_properties_common;
pub mod track_properties_subtitle;
pub mod track_properties_video;
pub mod warning;

pub use attachment::Attachment;
pub use chapter::ChapterSummary;
pub use container::{Container, ContainerFormat, ContainerProperties};
pub use duration::DurationValue;
pub use metadata::{MediaMetadata, PARSER_PROTOCOL_VERSION};
pub use playlist::PlaylistInfo;
pub use program::Program;
pub use tag::{TagEntry, TagsBundle};
pub use track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
pub use track_properties_audio::{
    AudioCodecConfig, AudioEmphasis, AudioTrackProperties, ChannelLayout, ChannelLayoutKind,
};
pub use track_properties_common::{CommonTrackProperties, TrackFlag};
pub use track_properties_subtitle::SubtitleTrackProperties;
pub use track_properties_video::{
    AlphaMode, ChromaFormat, ChromaSiting, ChromaSubsampling, Chromaticity, ColorMetadata,
    ColorRange, CropRect, Dimensions2D, DisplayUnit, FieldOrder, HevcTier, InterlaceFlag,
    MasterMetadata, ProjectionMetadata, ProjectionPose, ProjectionType, SampleAspectRatio,
    StereoMode, VideoCodecConfig, VideoTrackProperties,
};
pub use warning::{Warning, WarningCategory};
