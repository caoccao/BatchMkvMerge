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

//! Matroska / EBML element IDs.
//!
//! IDs are encoded VINTs but for header-only parsing we only ever need to
//! recognise the canonical 1-/2-/3-/4-byte forms. We stash them as `u32`
//! constants (keeping the marker bit on, mirroring mkvtoolnix's
//! `libebml::EbmlId`).
//!
//! Source-of-truth tables (the names here are the ones libmatroska / the
//! Matroska spec use):
//! - EBML head: <https://datatracker.ietf.org/doc/html/rfc8794>
//! - Matroska: <https://www.matroska.org/technical/elements.html>
//! - libmatroska headers: `vendor/libmatroska/matroska/Kax*.h`
//!
//! Adding a new ID? Keep the section comments aligned with the spec so
//! reviewers can spot omissions by reading the spec in order.

#![allow(dead_code)] // Several IDs are reserved for later phases (chapters,
// tags, cues …). Suppress unused warnings until those
// call sites land.

// =============================================================================
//   EBML header (top-level)
// =============================================================================

pub const EBML: u32 = 0x1A45_DFA3;
pub const EBML_VERSION: u32 = 0x4286;
pub const EBML_READ_VERSION: u32 = 0x42F7;
pub const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
pub const DOC_TYPE: u32 = 0x4282;
pub const DOC_TYPE_VERSION: u32 = 0x4287;
pub const DOC_TYPE_READ_VERSION: u32 = 0x4285;

// Generic EBML elements that may appear at any level.
pub const VOID: u32 = 0xEC;
pub const CRC32: u32 = 0xBF;

// =============================================================================
//   Segment + SeekHead
// =============================================================================

pub const SEGMENT: u32 = 0x1853_8067;
pub const SEEK_HEAD: u32 = 0x114D_9B74;
pub const SEEK: u32 = 0x4DBB;
pub const SEEK_ID: u32 = 0x53AB;
pub const SEEK_POSITION: u32 = 0x53AC;

// =============================================================================
//   Info (Segment Information)
// =============================================================================

pub const INFO: u32 = 0x1549_A966;
pub const SEGMENT_UID: u32 = 0x73A4;
pub const SEGMENT_FILENAME: u32 = 0x7384;
pub const PREV_UID: u32 = 0x3CB9_23;
pub const PREV_FILENAME: u32 = 0x3C83_AB;
pub const NEXT_UID: u32 = 0x3EB9_23;
pub const NEXT_FILENAME: u32 = 0x3E83_BB;
pub const SEGMENT_FAMILY: u32 = 0x4444;
pub const CHAPTER_TRANSLATE: u32 = 0x6924;
pub const TIMESTAMP_SCALE: u32 = 0x2AD7_B1;
pub const DURATION: u32 = 0x4489;
pub const DATE_UTC: u32 = 0x4461;
pub const TITLE: u32 = 0x7BA9;
pub const MUXING_APP: u32 = 0x4D80;
pub const WRITING_APP: u32 = 0x5741;

// =============================================================================
//   Cluster (we never enter clusters during header-only parse)
// =============================================================================

pub const CLUSTER: u32 = 0x1F43_B675;
pub const CLUSTER_TIMESTAMP: u32 = 0xE7;
pub const CLUSTER_POSITION: u32 = 0xA7;
pub const CLUSTER_PREV_SIZE: u32 = 0xAB;
pub const CLUSTER_SIMPLE_BLOCK: u32 = 0xA3;
pub const CLUSTER_BLOCK_GROUP: u32 = 0xA0;

// =============================================================================
//   Tracks
// =============================================================================

pub const TRACKS: u32 = 0x1654_AE6B;
pub const TRACK_ENTRY: u32 = 0xAE;

// Track common
pub const TRACK_NUMBER: u32 = 0xD7;
pub const TRACK_UID: u32 = 0x73C5;
pub const TRACK_TYPE: u32 = 0x83;
pub const FLAG_ENABLED: u32 = 0xB9;
pub const FLAG_DEFAULT: u32 = 0x88;
pub const FLAG_FORCED: u32 = 0x55AA;
pub const FLAG_HEARING_IMPAIRED: u32 = 0x55AB;
pub const FLAG_VISUAL_IMPAIRED: u32 = 0x55AC;
pub const FLAG_TEXT_DESCRIPTIONS: u32 = 0x55AD;
pub const FLAG_ORIGINAL: u32 = 0x55AE;
pub const FLAG_COMMENTARY: u32 = 0x55AF;
pub const FLAG_LACING: u32 = 0x9C;
pub const MIN_CACHE: u32 = 0x6DE7;
pub const MAX_CACHE: u32 = 0x6DF8;
pub const DEFAULT_DURATION: u32 = 0x23E3_83;
pub const DEFAULT_DECODED_FIELD_DURATION: u32 = 0x234E_7A;
pub const TRACK_TIMESTAMP_SCALE: u32 = 0x2331_59;
pub const MAX_BLOCK_ADDITION_ID: u32 = 0x55EE;
pub const BLOCK_ADDITION_MAPPING: u32 = 0x41E4;
pub const BLOCK_ADD_ID_VALUE: u32 = 0x41F0;
pub const BLOCK_ADD_ID_NAME: u32 = 0x41A4;
pub const BLOCK_ADD_ID_TYPE: u32 = 0x41E7;
pub const BLOCK_ADD_ID_EXTRA_DATA: u32 = 0x41ED;
pub const TRACK_NAME: u32 = 0x536E;
pub const TRACK_LANGUAGE: u32 = 0x22B5_9C;
pub const LANGUAGE_IETF: u32 = 0x22B5_9D;
pub const CODEC_ID: u32 = 0x86;
pub const CODEC_PRIVATE: u32 = 0x63A2;
pub const CODEC_NAME: u32 = 0x2586_88;
pub const ATTACHMENT_LINK: u32 = 0x7446;
pub const CODEC_SETTINGS: u32 = 0x3A96_97;
pub const CODEC_INFO_URL: u32 = 0x3B40_40;
pub const CODEC_DOWNLOAD_URL: u32 = 0x2640_67;
pub const CODEC_DECODE_ALL: u32 = 0xAA;
pub const TRACK_OVERLAY: u32 = 0x6FAB;
pub const CODEC_DELAY: u32 = 0x56AA;
pub const SEEK_PRE_ROLL: u32 = 0x56BB;
pub const TRACK_TRANSLATE: u32 = 0x6624;

// Audio sub-tree (TrackEntry > Audio)
pub const TRACK_AUDIO: u32 = 0xE1;
pub const AUDIO_SAMPLING_FREQ: u32 = 0xB5;
pub const AUDIO_OUTPUT_SAMPLING_FREQ: u32 = 0x78B5;
pub const AUDIO_CHANNELS: u32 = 0x9F;
pub const AUDIO_BIT_DEPTH: u32 = 0x6264;
pub const AUDIO_EMPHASIS: u32 = 0x52F1;
pub const AUDIO_CHANNEL_POSITIONS: u32 = 0x7D7B;

// Video sub-tree (TrackEntry > Video)
pub const TRACK_VIDEO: u32 = 0xE0;
pub const VIDEO_FLAG_INTERLACED: u32 = 0x9A;
pub const VIDEO_FIELD_ORDER: u32 = 0x9D;
pub const VIDEO_STEREO_MODE: u32 = 0x53B8;
pub const VIDEO_ALPHA_MODE: u32 = 0x53C0;
pub const VIDEO_PIXEL_WIDTH: u32 = 0xB0;
pub const VIDEO_PIXEL_HEIGHT: u32 = 0xBA;
pub const VIDEO_PIXEL_CROP_BOTTOM: u32 = 0x54AA;
pub const VIDEO_PIXEL_CROP_TOP: u32 = 0x54BB;
pub const VIDEO_PIXEL_CROP_LEFT: u32 = 0x54CC;
pub const VIDEO_PIXEL_CROP_RIGHT: u32 = 0x54DD;
pub const VIDEO_DISPLAY_WIDTH: u32 = 0x54B0;
pub const VIDEO_DISPLAY_HEIGHT: u32 = 0x54BA;
pub const VIDEO_DISPLAY_UNIT: u32 = 0x54B2;
pub const VIDEO_ASPECT_RATIO_TYPE: u32 = 0x54B3;
pub const VIDEO_COLOR_SPACE: u32 = 0x2EB5_24;
pub const VIDEO_FRAME_RATE: u32 = 0x2383_E3;

// Video > Colour
pub const VIDEO_COLOUR: u32 = 0x55B0;
pub const VIDEO_COLOUR_MATRIX: u32 = 0x55B1;
pub const VIDEO_BITS_PER_CHANNEL: u32 = 0x55B2;
pub const VIDEO_CHROMA_SUBSAMP_HORZ: u32 = 0x55B3;
pub const VIDEO_CHROMA_SUBSAMP_VERT: u32 = 0x55B4;
pub const VIDEO_CB_SUBSAMP_HORZ: u32 = 0x55B5;
pub const VIDEO_CB_SUBSAMP_VERT: u32 = 0x55B6;
pub const VIDEO_CHROMA_SIT_HORZ: u32 = 0x55B7;
pub const VIDEO_CHROMA_SIT_VERT: u32 = 0x55B8;
pub const VIDEO_COLOUR_RANGE: u32 = 0x55B9;
pub const VIDEO_COLOUR_TRANSFER_CHARACTER: u32 = 0x55BA;
pub const VIDEO_COLOUR_PRIMARIES: u32 = 0x55BB;
pub const VIDEO_COLOUR_MAX_CLL: u32 = 0x55BC;
pub const VIDEO_COLOUR_MAX_FALL: u32 = 0x55BD;

// Video > Colour > MasteringMetadata
pub const VIDEO_COLOUR_MASTER_META: u32 = 0x55D0;
pub const VIDEO_R_CHROMA_X: u32 = 0x55D1;
pub const VIDEO_R_CHROMA_Y: u32 = 0x55D2;
pub const VIDEO_G_CHROMA_X: u32 = 0x55D3;
pub const VIDEO_G_CHROMA_Y: u32 = 0x55D4;
pub const VIDEO_B_CHROMA_X: u32 = 0x55D5;
pub const VIDEO_B_CHROMA_Y: u32 = 0x55D6;
pub const VIDEO_WHITE_POINT_CHROMA_X: u32 = 0x55D7;
pub const VIDEO_WHITE_POINT_CHROMA_Y: u32 = 0x55D8;
pub const VIDEO_LUMINANCE_MAX: u32 = 0x55D9;
pub const VIDEO_LUMINANCE_MIN: u32 = 0x55DA;

// Video > Projection
pub const VIDEO_PROJECTION: u32 = 0x7670;
pub const VIDEO_PROJECTION_TYPE: u32 = 0x7671;
pub const VIDEO_PROJECTION_PRIVATE: u32 = 0x7672;
pub const VIDEO_PROJECTION_POSE_YAW: u32 = 0x7673;
pub const VIDEO_PROJECTION_POSE_PITCH: u32 = 0x7674;
pub const VIDEO_PROJECTION_POSE_ROLL: u32 = 0x7675;

// Content encoding (compression / encryption applied to track payload)
pub const CONTENT_ENCODINGS: u32 = 0x6D80;
pub const CONTENT_ENCODING: u32 = 0x6240;
pub const CONTENT_ENCODING_ORDER: u32 = 0x5031;
pub const CONTENT_ENCODING_SCOPE: u32 = 0x5032;
pub const CONTENT_ENCODING_TYPE: u32 = 0x5033;
pub const CONTENT_COMPRESSION: u32 = 0x5034;
pub const CONTENT_COMP_ALGO: u32 = 0x4254;
pub const CONTENT_COMP_SETTINGS: u32 = 0x4255;
pub const CONTENT_ENCRYPTION: u32 = 0x5035;
pub const CONTENT_ENC_ALGO: u32 = 0x47E1;
pub const CONTENT_ENC_KEY_ID: u32 = 0x47E2;

// =============================================================================
//   Attachments
// =============================================================================

pub const ATTACHMENTS: u32 = 0x1941_A469;
pub const ATTACHED_FILE: u32 = 0x61A7;
pub const FILE_DESCRIPTION: u32 = 0x467E;
pub const FILE_NAME: u32 = 0x466E;
pub const FILE_MIME_TYPE: u32 = 0x4660;
pub const FILE_DATA: u32 = 0x465C;
pub const FILE_UID: u32 = 0x46AE;
pub const FILE_REFERRAL: u32 = 0x4675;
pub const FILE_USED_START_TIME: u32 = 0x4661;
pub const FILE_USED_END_TIME: u32 = 0x4662;

// =============================================================================
//   Chapters
// =============================================================================

pub const CHAPTERS: u32 = 0x1043_A770;
pub const EDITION_ENTRY: u32 = 0x45B9;
pub const EDITION_UID: u32 = 0x45BC;
pub const EDITION_FLAG_HIDDEN: u32 = 0x45BD;
pub const EDITION_FLAG_DEFAULT: u32 = 0x45DB;
pub const EDITION_FLAG_ORDERED: u32 = 0x45DD;
pub const CHAPTER_ATOM: u32 = 0xB6;
pub const CHAPTER_UID: u32 = 0x73C4;
pub const CHAPTER_STRING_UID: u32 = 0x5654;
pub const CHAPTER_TIMESTAMP_START: u32 = 0x91;
pub const CHAPTER_TIMESTAMP_END: u32 = 0x92;
pub const CHAPTER_FLAG_HIDDEN: u32 = 0x98;
pub const CHAPTER_FLAG_ENABLED: u32 = 0x4598;
pub const CHAPTER_DISPLAY: u32 = 0x80;
pub const CHAP_STRING: u32 = 0x85;
pub const CHAP_LANGUAGE: u32 = 0x437C;
pub const CHAP_LANGUAGE_IETF: u32 = 0x437D;
pub const CHAP_COUNTRY: u32 = 0x437E;
pub const CHAPTER_TRACK: u32 = 0x8F;
pub const CHAPTER_TRACK_NUMBER: u32 = 0x89;
pub const CHAPTER_PROCESS: u32 = 0x6944;
pub const CHAPTER_PROCESS_CODEC_ID: u32 = 0x6955;
pub const CHAPTER_PROCESS_PRIVATE: u32 = 0x450D;

// =============================================================================
//   Tags
// =============================================================================

pub const TAGS: u32 = 0x1254_C367;
pub const TAG: u32 = 0x7373;
pub const TAG_TARGETS: u32 = 0x63C0;
pub const TAG_TARGET_TYPE_VALUE: u32 = 0x68CA;
pub const TAG_TARGET_TYPE: u32 = 0x63CA;
pub const TAG_TRACK_UID: u32 = 0x63C5;
pub const TAG_EDITION_UID: u32 = 0x63C9;
pub const TAG_CHAPTER_UID: u32 = 0x63C4;
pub const TAG_ATTACHMENT_UID: u32 = 0x63C6;
pub const TAG_SIMPLE: u32 = 0x67C8;
pub const TAG_NAME: u32 = 0x45A3;
pub const TAG_LANGUAGE: u32 = 0x447A;
pub const TAG_LANGUAGE_IETF: u32 = 0x447B;
pub const TAG_DEFAULT: u32 = 0x4484;
pub const TAG_STRING: u32 = 0x4487;
pub const TAG_BINARY: u32 = 0x4485;

// =============================================================================
//   Cues (we only count cue points per track for `num_index_entries`)
// =============================================================================

pub const CUES: u32 = 0x1C53_BB6B;
pub const CUE_POINT: u32 = 0xBB;
pub const CUE_TIME: u32 = 0xB3;
pub const CUE_TRACK_POSITIONS: u32 = 0xB7;
pub const CUE_TRACK: u32 = 0xF7;
pub const CUE_CLUSTER_POSITION: u32 = 0xF1;

// Block elements inside a Cluster (read header-only for minimum-timestamp
// discovery — we never decode frame payloads).
pub const CLUSTER_BLOCK: u32 = 0xA1;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn well_known_ebml_constants_match_spec() {
    assert_eq!(EBML, 0x1A45_DFA3);
    assert_eq!(SEGMENT, 0x1853_8067);
    assert_eq!(SEEK_HEAD, 0x114D_9B74);
    assert_eq!(INFO, 0x1549_A966);
    assert_eq!(TRACKS, 0x1654_AE6B);
    assert_eq!(ATTACHMENTS, 0x1941_A469);
    assert_eq!(CHAPTERS, 0x1043_A770);
    assert_eq!(TAGS, 0x1254_C367);
    assert_eq!(CUES, 0x1C53_BB6B);
    assert_eq!(CLUSTER, 0x1F43_B675);
    assert_eq!(VOID, 0xEC);
    assert_eq!(CRC32, 0xBF);
  }

  #[test]
  fn one_byte_ids_have_marker_bit() {
    for id in [VOID, CRC32, TRACK_ENTRY, CODEC_ID, TRACK_TYPE, TRACK_NUMBER] {
      // 1-byte IDs always have the top bit set (marker for VINT width 1)
      assert!(id <= 0xFF);
      assert!(id & 0x80 != 0, "id {id:#x} should have top bit set");
    }
  }

  #[test]
  fn two_byte_ids_have_correct_marker() {
    // Spot-check a handful of well-known width-2 element IDs.
    for id in [TRACK_UID, FLAG_FORCED, TRACK_NAME, SEEK] {
      assert!(id > 0xFF && id <= 0xFFFF);
      // Top byte high nibble bit 6 (`0x40`) is the width-2 marker.
      assert!(((id >> 8) & 0x40) != 0, "id {id:#x} should have width-2 marker bit");
    }
  }

  #[test]
  fn four_byte_ids_match_libmatroska() {
    assert_eq!(VIDEO_COLOR_SPACE, 0x2EB5_24);
    assert_eq!(TRACK_LANGUAGE, 0x22B5_9C);
    assert_eq!(LANGUAGE_IETF, 0x22B5_9D);
    assert_eq!(DEFAULT_DURATION, 0x23E3_83);
    assert_eq!(VIDEO_FRAME_RATE, 0x2383_E3);
  }

  #[test]
  fn projection_block_ids_form_contiguous_range() {
    assert_eq!(VIDEO_PROJECTION_TYPE, VIDEO_PROJECTION + 1);
    assert_eq!(VIDEO_PROJECTION_PRIVATE, VIDEO_PROJECTION + 2);
    assert_eq!(VIDEO_PROJECTION_POSE_YAW, VIDEO_PROJECTION + 3);
    assert_eq!(VIDEO_PROJECTION_POSE_PITCH, VIDEO_PROJECTION + 4);
    assert_eq!(VIDEO_PROJECTION_POSE_ROLL, VIDEO_PROJECTION + 5);
  }
}
