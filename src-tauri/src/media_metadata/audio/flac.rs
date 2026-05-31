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

//! Native FLAC reader (`.flac` files starting with `fLaC`).
//!
//! Layout (FLAC spec §3):
//!
//! ```text
//! 4   "fLaC"
//! repeat metadata blocks:
//!   u8  is_last(1) | block_type(7)
//!   u24 length (BE)
//!   [length bytes of block body]
//! ```
//!
//! Block type 0 = STREAMINFO (mandatory, first).  Block type 4 =
//! VORBIS_COMMENT — same layout as the in-Ogg variant decoded by [`crate::media_metadata::ogg::comments`].
//!
//! The FLAC codec-private header is rebuilt to match mkvtoolnix's
//! `flac_reader_c::read_headers` (`r_flac.cpp:57-89`): the `"fLaC"` magic plus
//! **every metadata block except PICTURE and PADDING**, with the "last metadata
//! block" flag re-normalised so only the final kept block carries it
//! (PARSER-225).  PICTURE blocks become attachments; their declared payload
//! length is trusted from the block header, so cover art larger than the
//! bounded header read is still surfaced (PARSER-226).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::language::Language;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::attachment::Attachment;
use crate::media_metadata::model::container::ContainerFormat;
use crate::media_metadata::model::duration::DurationValue;
use crate::media_metadata::model::tag::TagEntry;
use crate::media_metadata::model::track::{CodecInfo, CodecPrivate, Track, TrackProperties, TrackType};
use crate::media_metadata::model::track_properties_audio::{AudioCodecConfig, AudioTrackProperties};
use crate::media_metadata::model::track_properties_common::CommonTrackProperties;
use crate::media_metadata::ogg::comments;
use crate::media_metadata::reader::Reader;

use super::id3v2;

const BLOCK_TYPE_STREAMINFO: u8 = 0;
const BLOCK_TYPE_PADDING: u8 = 1;
const BLOCK_TYPE_VORBIS_COMMENT: u8 = 4;
const BLOCK_TYPE_PICTURE: u8 = 6;

/// Append a metadata block (`header` + `body`) to the FLAC codec-private blob,
/// clearing the "last metadata block" flag.  Returns the offset of the block's
/// header byte so the caller can set the flag on the final kept block, exactly
/// as `r_flac.cpp:71-85` does when re-emitting the kept blocks (PARSER-225).
fn append_kept_block(codec_private: &mut Vec<u8>, header: &[u8; 4], body: &[u8]) -> usize {
  let offset = codec_private.len();
  let mut normalised = *header;
  normalised[0] &= 0x7f; // clear last-block flag; the final block is set later
  codec_private.extend_from_slice(&normalised);
  codec_private.extend_from_slice(body);
  offset
}

/// Byte offset where the FLAC stream starts, skipping a leading ID3v2 tag.
/// Mirrors `r_flac.cpp`'s `mtx::id3::skip_v2_tag` (PARSER-023).
fn payload_start(src: &mut FileSource) -> Result<u64, ParseError> {
  let mut head = [0u8; 10];
  let n = src.read_at_most(&mut head)?;
  src.seek_to(0)?;
  if n == 10 {
    Ok(id3v2::skip_id3v2(&head).unwrap_or(0) as u64)
  } else {
    Ok(0)
  }
}

/// Walk the FLAC metadata-block chain over a [`FileSource`], skipping past
/// large PICTURE/PADDING/APPLICATION blocks via seeks so VORBIS_COMMENT and
/// STREAMINFO are found regardless of how far into the file they sit
/// (PARSER-024). Skips a leading ID3v2 tag (PARSER-023).
pub fn parse_source(src: &mut FileSource) -> Result<Option<FlacMetadata>, ParseError> {
  parse_source_with_deadline(src, None)
}

fn parse_source_with_deadline(
  src: &mut FileSource,
  deadline: Option<&Deadline>,
) -> Result<Option<FlacMetadata>, ParseError> {
  let start = payload_start(src)?;
  src.seek_to(start)?;
  let mut magic = [0u8; 4];
  if src.read_at_most(&mut magic)? < 4 || &magic != b"fLaC" {
    return Ok(None);
  }
  let file_size = src.length().unwrap_or(u64::MAX);

  let mut metadata = FlacMetadata::default();
  metadata.codec_private.extend_from_slice(b"fLaC");
  // Offset of the most-recently-appended kept block's header byte; the final
  // kept block gets the "last metadata block" flag set after the walk.
  let mut last_kept_offset: Option<usize> = None;
  let mut pos = start + 4;
  let block_cap = deadline.map(Deadline::max_element_size).unwrap_or(u64::MAX);

  loop {
    if let Some(deadline) = deadline {
      deadline.check("flac::metadata")?;
    }
    src.seek_to(pos)?;
    let mut header = [0u8; 4];
    if src.read_at_most(&mut header)? < 4 {
      break;
    }
    let last_block = header[0] & 0x80 != 0;
    let block_type = header[0] & 0x7F;
    let length = ((header[1] as u64) << 16) | ((header[2] as u64) << 8) | (header[3] as u64);
    let body_pos = pos + 4;

    match block_type {
      BLOCK_TYPE_STREAMINFO => {
        src.seek_to(body_pos)?;
        let body = src.read_vec_capped(length, block_cap)?;
        if body.len() >= 34 {
          metadata.streaminfo = Some(decode_streaminfo(&body));
        }
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, &body));
      }
      BLOCK_TYPE_VORBIS_COMMENT => {
        src.seek_to(body_pos)?;
        let body = src.read_vec_capped(length, block_cap)?;
        if let Some(c) = comments::parse(&body) {
          metadata.vendor = Some(c.vendor);
          metadata.tags = c.entries;
        }
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, &body));
      }
      // PARSER-225: PICTURE and PADDING are the only blocks mkvmerge strips
      // from the FLAC header (`r_flac.cpp:62-68`); everything else is kept.
      BLOCK_TYPE_PADDING => {}
      BLOCK_TYPE_PICTURE => {
        // PARSER-226 / PARSER-334: read the variable PICTURE header fields
        // completely, but skip the image payload itself.  This keeps large
        // descriptions/MIME strings parseable without materialising cover art.
        if let Some(p) = decode_picture_from_source(src, body_pos, length, file_size, block_cap)? {
          metadata.pictures.push(p);
        }
      }
      // APPLICATION (2), SEEKTABLE (3), CUESHEET (5), and unknown blocks are
      // kept verbatim in the codec-private header (PARSER-225).
      _ => {
        src.seek_to(body_pos)?;
        let body = src.read_vec_capped(length, block_cap)?;
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, &body));
      }
    }

    pos = body_pos + length;
    if last_block || pos >= file_size {
      break;
    }
  }

  // Set the "last metadata block" flag on the final kept block (r_flac.cpp:82).
  if let Some(offset) = last_kept_offset {
    metadata.codec_private[offset] |= 0x80;
  }

  Ok(Some(metadata))
}

#[derive(Debug, Clone)]
pub struct FlacStreaminfo {
  pub min_block_size: u32,
  pub max_block_size: u32,
  pub min_frame_size: u32,
  pub max_frame_size: u32,
  pub sample_rate: u32,
  pub channels: u32,
  pub bits_per_sample: u32,
  pub total_samples: u64,
  pub md5_hex: String,
}

#[derive(Debug, Default, Clone)]
pub struct FlacMetadata {
  pub streaminfo: Option<FlacStreaminfo>,
  pub vendor: Option<String>,
  pub tags: Vec<TagEntry>,
  pub pictures: Vec<FlacPicture>,
  pub codec_private: Vec<u8>,
}

/// Decoded FLAC PICTURE metadata block — mirrors mkvtoolnix's
/// `FLAC__StreamMetadata_Picture` (PARSER-097).  Payload data length is the
/// declared length, even when the file is truncated — matches the on-disk
/// value advertised by the producer.
#[derive(Debug, Clone)]
pub struct FlacPicture {
  pub picture_type: u32,
  pub mime_type: String,
  pub description: String,
  pub data_length: u32,
}

pub fn parse(bytes: &[u8]) -> Option<FlacMetadata> {
  if bytes.len() < 4 || &bytes[..4] != b"fLaC" {
    return None;
  }
  let mut metadata = FlacMetadata::default();
  metadata.codec_private.extend_from_slice(b"fLaC");
  let mut last_kept_offset: Option<usize> = None;
  let mut pos = 4usize;
  loop {
    if pos + 4 > bytes.len() {
      break;
    }
    let mut header = [0u8; 4];
    header.copy_from_slice(&bytes[pos..pos + 4]);
    let last_block = header[0] & 0x80 != 0;
    let block_type = header[0] & 0x7F;
    let length = ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | header[3] as usize;
    pos += 4;
    let body_end = pos + length;
    if body_end > bytes.len() {
      break;
    }
    let body = &bytes[pos..body_end];
    match block_type {
      BLOCK_TYPE_STREAMINFO => {
        if body.len() >= 34 {
          metadata.streaminfo = Some(decode_streaminfo(body));
        }
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, body));
      }
      BLOCK_TYPE_VORBIS_COMMENT => {
        if let Some(c) = comments::parse(body) {
          metadata.vendor = Some(c.vendor);
          metadata.tags = c.entries;
        }
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, body));
      }
      // PARSER-225: only PICTURE and PADDING are excluded from the header.
      BLOCK_TYPE_PADDING => {}
      BLOCK_TYPE_PICTURE => {
        if let Some(p) = decode_picture(body, length) {
          metadata.pictures.push(p);
        }
      }
      _ => {
        last_kept_offset = Some(append_kept_block(&mut metadata.codec_private, &header, body));
      }
    }
    pos = body_end;
    if last_block {
      break;
    }
  }
  if let Some(offset) = last_kept_offset {
    metadata.codec_private[offset] |= 0x80;
  }
  Some(metadata)
}

/// Decode the fixed prefix of a FLAC PICTURE metadata block.  Matches FLAC spec
/// §8.4: picture-type / MIME / description / dims / declared data length, all
/// big-endian u32 except the strings.  We deliberately do **not** materialise
/// the picture body itself — the declared length lets us emit it as an
/// [`Attachment`] with the correct size.
///
/// `block_length` is the declared metadata-block length (which, on disk, equals
/// the header fields plus the payload).  Validation is against that declared
/// length rather than the in-memory `body` slice, so a picture whose payload
/// exceeds the bounded read cap is still surfaced (PARSER-226) while a block
/// whose declared payload does not fit the block (truncated / inconsistent) is
/// dropped, matching libFLAC's all-or-nothing block read (PARSER-154).
fn decode_picture(body: &[u8], block_length: usize) -> Option<FlacPicture> {
  let mut pos = 0usize;
  let picture_type = read_be_u32(body, &mut pos)?;
  let mime_len = read_be_u32(body, &mut pos)? as usize;
  let mime_type = read_string(body, &mut pos, mime_len)?;
  let desc_len = read_be_u32(body, &mut pos)? as usize;
  let description = read_string(body, &mut pos, desc_len)?;
  // width, height, colour-depth, colours-used — skip four u32 fields.
  for _ in 0..4 {
    let _ = read_be_u32(body, &mut pos)?;
  }
  let data_length = read_be_u32(body, &mut pos)?;
  // mkvtoolnix drops pictures whose data is missing or zero-length
  // (`!picture.data || !picture.data_length`, r_flac.cpp:251). Require the
  // declared payload to be non-empty and to fit within the declared block.
  if data_length == 0 || pos.saturating_add(data_length as usize) > block_length {
    return None;
  }
  Some(FlacPicture {
    picture_type,
    mime_type,
    description,
    data_length,
  })
}

fn decode_picture_from_source(
  src: &mut FileSource,
  body_pos: u64,
  block_length: u64,
  file_size: u64,
  cap: u64,
) -> Result<Option<FlacPicture>, ParseError> {
  if body_pos.saturating_add(block_length) > file_size {
    return Ok(None);
  }
  let mut consumed = 0u64;

  src.seek_to(body_pos)?;
  let picture_type = src.read_u32_be()?;
  consumed += 4;

  let mime_len = src.read_u32_be()? as u64;
  consumed += 4;
  if consumed.saturating_add(mime_len) > block_length {
    return Ok(None);
  }
  let mime_type = String::from_utf8_lossy(&src.read_vec_capped(mime_len, cap)?).into_owned();
  consumed += mime_len;

  let desc_len = src.read_u32_be()? as u64;
  consumed += 4;
  if consumed.saturating_add(desc_len) > block_length {
    return Ok(None);
  }
  let description = String::from_utf8_lossy(&src.read_vec_capped(desc_len, cap)?).into_owned();
  consumed += desc_len;

  if consumed.saturating_add(20) > block_length {
    return Ok(None);
  }
  src.skip(16)?;
  consumed += 16;
  let data_length = src.read_u32_be()?;
  consumed += 4;
  if data_length == 0 || consumed.saturating_add(data_length as u64) > block_length {
    return Ok(None);
  }
  Ok(Some(FlacPicture {
    picture_type,
    mime_type,
    description,
    data_length,
  }))
}

fn read_be_u32(body: &[u8], pos: &mut usize) -> Option<u32> {
  if *pos + 4 > body.len() {
    return None;
  }
  let v = u32::from_be_bytes([body[*pos], body[*pos + 1], body[*pos + 2], body[*pos + 3]]);
  *pos += 4;
  Some(v)
}

fn read_string(body: &[u8], pos: &mut usize, len: usize) -> Option<String> {
  if *pos + len > body.len() {
    return None;
  }
  let s = String::from_utf8_lossy(&body[*pos..*pos + len]).into_owned();
  *pos += len;
  Some(s)
}

/// Picture-type → file-base-name mapping — port of
/// `mtx::flac::file_base_name_for_picture_type` in `common/flac.cpp:355-377`.
fn picture_type_name(t: u32) -> &'static str {
  match t {
    0 => "other",
    1 => "icon",
    2 => "other icon",
    3 => "cover",
    4 => "cover (back)",
    5 => "leaflet page",
    6 => "media",
    7 => "lead artist - lead performer - soloist",
    8 => "artist - performer",
    9 => "conductor",
    10 => "band - orchestra",
    11 => "composer",
    12 => "lyricist - text writer",
    13 => "recording location",
    14 => "during recording",
    15 => "during performance",
    16 => "movie - video screen capture",
    17 => "a bright colored fish",
    18 => "illustration",
    19 => "band - artist logotype",
    20 => "publisher - Studio logotype",
    _ => "unknown",
  }
}

/// Approximate `mtx::mime::primary_file_extension_for_type` — covers the
/// image MIME types FLAC PICTURE blocks declare in practice.  Returns the
/// extension without a leading dot, or empty if unknown.
fn primary_extension_for_mime(mime: &str) -> &'static str {
  match mime.to_ascii_lowercase().as_str() {
    "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/jfif" => "jpg",
    "image/png" => "png",
    "image/gif" => "gif",
    "image/bmp" | "image/x-bmp" => "bmp",
    "image/webp" => "webp",
    "image/tiff" => "tiff",
    "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
    "-->" => "",
    _ => "",
  }
}

fn decode_streaminfo(body: &[u8]) -> FlacStreaminfo {
  let min_block_size = u16::from_be_bytes([body[0], body[1]]) as u32;
  let max_block_size = u16::from_be_bytes([body[2], body[3]]) as u32;
  let min_frame_size = ((body[4] as u32) << 16) | ((body[5] as u32) << 8) | body[6] as u32;
  let max_frame_size = ((body[7] as u32) << 16) | ((body[8] as u32) << 8) | body[9] as u32;
  let packed = u64::from_be_bytes([
    body[10], body[11], body[12], body[13], body[14], body[15], body[16], body[17],
  ]);
  let sample_rate = ((packed >> 44) & 0xF_FFFF) as u32;
  let channels = (((packed >> 41) & 0x07) + 1) as u32;
  let bps = (((packed >> 36) & 0x1F) + 1) as u32;
  let total_samples = packed & 0x0F_FFFF_FFFF;
  let md5: [u8; 16] = body[18..34].try_into().unwrap();
  let md5_hex = md5.iter().map(|b| format!("{:02x}", b)).collect();
  FlacStreaminfo {
    min_block_size,
    max_block_size,
    min_frame_size,
    max_frame_size,
    sample_rate,
    channels,
    bits_per_sample: bps,
    total_samples,
    md5_hex,
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FlacReader;

impl Reader for FlacReader {
  fn name(&self) -> &'static str {
    "flac"
  }

  fn probe(&self, src: &mut FileSource) -> Result<bool, ParseError> {
    let start = payload_start(src)?;
    src.seek_to(start)?;
    let mut head = [0u8; 4];
    let read = src.read_at_most(&mut head)?;
    src.seek_to(0)?;
    Ok(read == 4 && &head == b"fLaC")
  }

  fn read_headers(&self, src: &mut FileSource, deadline: &Deadline, out: &mut MediaMetadata) -> Result<(), ParseError> {
    let metadata = parse_source_with_deadline(src, Some(deadline))?.ok_or(ParseError::Unrecognised)?;
    let streaminfo = metadata.streaminfo.ok_or(ParseError::Malformed {
      format: "flac",
      offset: 0,
      reason: "missing STREAMINFO block".to_string(),
    })?;

    out.container.format = ContainerFormat::Flac;
    out.container.recognized = true;
    out.container.supported = true;
    if streaminfo.sample_rate > 0 {
      let ns = (streaminfo.total_samples as u128).saturating_mul(1_000_000_000) / streaminfo.sample_rate as u128;
      out.container.properties.duration = Some(DurationValue::from_ns(ns as u64));
    }
    if let Some(vendor) = metadata.vendor.clone() {
      out.container.properties.muxing_app = Some(vendor);
    }

    let mut common = CommonTrackProperties::default();
    common.number = Some(1);
    if let Some(title) = tag_value(&metadata.tags, "TITLE") {
      common.track_name = Some(title.to_string());
    }
    if let Some(language) = tag_value(&metadata.tags, "LANGUAGE") {
      common.language = Some(Language::resolve(Some(language), Some(language), false));
    }
    let audio = AudioTrackProperties {
      channels: Some(streaminfo.channels),
      sampling_frequency: Some(streaminfo.sample_rate as f64),
      bit_depth: Some(streaminfo.bits_per_sample),
      codec_config: Some(AudioCodecConfig {
        flac_min_block_size: Some(streaminfo.min_block_size),
        flac_max_block_size: Some(streaminfo.max_block_size),
        flac_min_frame_size: Some(streaminfo.min_frame_size),
        flac_max_frame_size: Some(streaminfo.max_frame_size),
        flac_total_samples: if streaminfo.total_samples == 0 {
          None
        } else {
          Some(streaminfo.total_samples)
        },
        flac_md5_hex: Some(streaminfo.md5_hex.clone()),
        ..AudioCodecConfig::default()
      }),
      ..AudioTrackProperties::default()
    };
    out.tracks.push(Track {
      id: 0,
      track_type: TrackType::Audio,
      codec: CodecInfo {
        id: "A_FLAC".to_string(),
        name: Some("FLAC".to_string()),
        codec_private: Some(CodecPrivate::from_bytes(&metadata.codec_private)),
      },
      properties: TrackProperties {
        common,
        audio: Some(audio),
        tags: metadata.tags,
        ..TrackProperties::default()
      },
    });

    // PARSER-097: FLAC PICTURE blocks become attachments.  mkvtoolnix's
    // `r_flac.cpp::handle_picture_metadata` drops pictures with empty MIME
    // or generated name — mirror that gate exactly.
    let mut next_id: u32 = (out.attachments.len() as u32) + 1;
    for picture in metadata.pictures {
      if picture.mime_type.is_empty() {
        continue;
      }
      let base = picture_type_name(picture.picture_type);
      let ext = primary_extension_for_mime(&picture.mime_type);
      let file_name = if ext.is_empty() {
        base.to_string()
      } else {
        format!("{base}.{ext}")
      };
      if file_name.is_empty() {
        continue;
      }
      out.attachments.push(Attachment {
        id: next_id,
        file_name,
        mime_type: Some(picture.mime_type),
        description: if picture.description.is_empty() {
          None
        } else {
          Some(picture.description)
        },
        size: picture.data_length as u64,
        uid_hex: None,
      });
      next_id += 1;
    }
    Ok(())
  }
}

fn tag_value<'a>(tags: &'a [TagEntry], name: &str) -> Option<&'a str> {
  tags
    .iter()
    .find(|tag| tag.name.eq_ignore_ascii_case(name))
    .map(|tag| tag.value.as_str())
}

#[cfg(test)]
pub(crate) fn build_flac_native(sample_rate: u32, channels: u32, bps: u32, total_samples: u64) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"fLaC");
  // STREAMINFO: type 0, last flag, length 34
  bytes.push(0x80); // last_block + type 0
  bytes.extend_from_slice(&[0u8, 0u8, 34]);
  let mut info = vec![0u8; 34];
  info[..2].copy_from_slice(&4096u16.to_be_bytes());
  info[2..4].copy_from_slice(&4096u16.to_be_bytes());
  let packed = ((sample_rate as u64) << 44)
    | (((channels - 1) as u64 & 0x7) << 41)
    | (((bps - 1) as u64 & 0x1F) << 36)
    | (total_samples & 0x0F_FFFF_FFFF);
  info[10..18].copy_from_slice(&packed.to_be_bytes());
  bytes.extend(info);
  bytes
}

#[cfg(test)]
fn block_header(last: bool, block_type: u8, length: usize) -> Vec<u8> {
  let mut h = vec![if last { 0x80 | block_type } else { block_type }];
  h.extend_from_slice(&[(length >> 16) as u8, (length >> 8) as u8, length as u8]);
  h
}

/// Build a valid FLAC PICTURE block payload (header + data bytes).  Mirrors
/// the FLAC spec §8.4 layout used by the decoder above.
#[cfg(test)]
fn build_picture_block(picture_type: u32, mime: &str, description: &str, data_length: u32) -> Vec<u8> {
  let mut b = Vec::new();
  b.extend_from_slice(&picture_type.to_be_bytes());
  b.extend_from_slice(&(mime.len() as u32).to_be_bytes());
  b.extend_from_slice(mime.as_bytes());
  b.extend_from_slice(&(description.len() as u32).to_be_bytes());
  b.extend_from_slice(description.as_bytes());
  // width, height, depth, colours used — values irrelevant for the parser.
  b.extend_from_slice(&0u32.to_be_bytes());
  b.extend_from_slice(&0u32.to_be_bytes());
  b.extend_from_slice(&0u32.to_be_bytes());
  b.extend_from_slice(&0u32.to_be_bytes());
  b.extend_from_slice(&data_length.to_be_bytes());
  b.extend(vec![0xCDu8; data_length as usize]);
  b
}

/// Build a native FLAC stream with STREAMINFO, a large PICTURE block, then a
/// VORBIS_COMMENT block — exercising the >64 KiB metadata walk.
#[cfg(test)]
fn build_flac_with_picture_and_comment(picture_data_len: usize) -> Vec<u8> {
  let mut bytes = Vec::new();
  bytes.extend_from_slice(b"fLaC");
  // STREAMINFO (type 0, not last).
  bytes.extend(block_header(false, 0, 34));
  let mut info = vec![0u8; 34];
  info[..2].copy_from_slice(&4096u16.to_be_bytes());
  info[2..4].copy_from_slice(&4096u16.to_be_bytes());
  let packed = (48_000u64 << 44) | ((1u64) << 41) | ((23u64) << 36) | 96_000u64;
  info[10..18].copy_from_slice(&packed.to_be_bytes());
  bytes.extend(info);
  // PICTURE (type 6, not last) — front-cover JPEG of the requested size.
  let picture = build_picture_block(3, "image/jpeg", "Front cover", picture_data_len as u32);
  bytes.extend(block_header(false, 6, picture.len()));
  bytes.extend(picture);
  // VORBIS_COMMENT (type 4, last).
  let comment = comments::build_block("ref enc", &[("TITLE", "Far")]);
  bytes.extend(block_header(true, 4, comment.len()));
  bytes.extend(comment);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::io::Cursor;

  #[test]
  fn parse_extracts_streaminfo_fields() {
    let bytes = build_flac_native(48_000, 2, 24, 96_000);
    let m = parse(&bytes).unwrap();
    let si = m.streaminfo.unwrap();
    assert_eq!(si.sample_rate, 48_000);
    assert_eq!(si.channels, 2);
    assert_eq!(si.bits_per_sample, 24);
    assert_eq!(si.total_samples, 96_000);
  }

  #[test]
  fn parse_rejects_non_native_flac() {
    let bytes = b"junk".to_vec();
    assert!(parse(&bytes).is_none());
  }

  #[test]
  fn parse_handles_truncated_block_header_gracefully() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(&[0x80, 0xFF]); // truncated header
    let m = parse(&bytes).unwrap();
    assert!(m.streaminfo.is_none());
  }

  #[test]
  fn probe_accepts_flac_magic() {
    let bytes = build_flac_native(48_000, 2, 24, 1);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    assert!(FlacReader.probe(&mut s).unwrap());
  }

  #[test]
  fn read_headers_populates_track_and_duration() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_flac_native(48_000, 2, 24, 96_000);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.container.format, ContainerFormat::Flac);
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.channels, Some(2));
    assert_eq!(a.bit_depth, Some(24));
    assert!(
      out.tracks[0]
        .codec
        .codec_private
        .as_ref()
        .unwrap()
        .hex
        .starts_with("664c6143")
    );
    // 96_000 samples / 48_000 = 2 seconds
    assert_eq!(out.container.properties.duration.unwrap().ns, 2_000_000_000);
  }

  #[test]
  fn read_headers_promotes_title_language_and_private_metadata() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fLaC");
    let native = build_flac_native(48_000, 2, 24, 96_000);
    let mut streaminfo = native[4..42].to_vec();
    streaminfo[0] &= 0x7f;
    bytes.extend_from_slice(&streaminfo);
    let comment = comments::build_block("ref enc", &[("TITLE", "Song"), ("LANGUAGE", "de")]);
    bytes.extend(block_header(true, 4, comment.len()));
    bytes.extend(comment);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let common = &out.tracks[0].properties.common;
    assert_eq!(common.track_name.as_deref(), Some("Song"));
    assert!(common.language.is_some());
    assert!(out.tracks[0].codec.codec_private.as_ref().unwrap().length > 42);
  }

  #[test]
  fn metadata_walk_continues_past_four_thousand_blocks() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fLaC");
    let native = build_flac_native(48_000, 2, 24, 96_000);
    let mut streaminfo = native[4..42].to_vec();
    streaminfo[0] &= 0x7f;
    bytes.extend_from_slice(&streaminfo);
    for _ in 0..4096 {
      bytes.extend(block_header(false, BLOCK_TYPE_PADDING, 0));
    }
    let comment = comments::build_block("ref enc", &[("TITLE", "Late")]);
    bytes.extend(block_header(true, BLOCK_TYPE_VORBIS_COMMENT, comment.len()));
    bytes.extend(comment);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert_eq!(m.tags.len(), 1);
    assert_eq!(m.tags[0].value, "Late");
  }

  // ---- PARSER-023: ID3v2 prefix ----------------------------------------

  #[test]
  fn probe_and_read_accept_flac_after_id3v2_tag() {
    let mut bytes = crate::media_metadata::audio::id3v2::build_id3v2_tag(false, 256);
    bytes.extend(build_flac_native(44_100, 2, 16, 44_100));
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes.clone()));
    assert!(FlacReader.probe(&mut s).unwrap());

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    let a = out.tracks[0].properties.audio.as_ref().unwrap();
    assert_eq!(a.sampling_frequency, Some(44_100.0));
  }

  // ---- PARSER-024: metadata chain beyond 64 KiB ------------------------

  #[test]
  fn finds_comment_after_large_picture_block() {
    // 128 KiB picture block sits between STREAMINFO and VORBIS_COMMENT.
    let bytes = build_flac_with_picture_and_comment(128 * 1024);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    assert!(m.streaminfo.is_some());
    assert_eq!(m.vendor.as_deref(), Some("ref enc"));
    assert_eq!(m.tags.len(), 1);
    assert_eq!(m.tags[0].name, "TITLE");
    assert_eq!(m.tags[0].value, "Far");
  }

  // ---- PARSER-225: codec-private keeps all non-PICTURE/PADDING blocks --

  #[test]
  fn codec_private_keeps_application_seektable_and_cuesheet() {
    let mut bytes = b"fLaC".to_vec();
    // STREAMINFO (type 0, not last).
    bytes.extend(block_header(false, 0, 34));
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = (48_000u64 << 44) | ((1u64) << 41) | ((23u64) << 36) | 96_000u64;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    // APPLICATION (type 2): 4-byte id + payload.
    let app = b"riff\x01\x02\x03\x04".to_vec();
    bytes.extend(block_header(false, 2, app.len()));
    bytes.extend(&app);
    // SEEKTABLE (type 3): one 18-byte seek point.
    let seektable = vec![0xABu8; 18];
    bytes.extend(block_header(false, 3, seektable.len()));
    bytes.extend(&seektable);
    // PADDING (type 1) — must be dropped.
    bytes.extend(block_header(false, 1, 32));
    bytes.extend(vec![0u8; 32]);
    // PICTURE (type 6) — must be dropped from codec-private.
    let picture = build_picture_block(3, "image/jpeg", "Cover", 16);
    bytes.extend(block_header(false, 6, picture.len()));
    bytes.extend(&picture);
    // VORBIS_COMMENT (type 4, last).
    let comment = comments::build_block("enc", &[("TITLE", "X")]);
    bytes.extend(block_header(true, 4, comment.len()));
    bytes.extend(&comment);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let m = parse_source(&mut s).unwrap().unwrap();
    let cp = &m.codec_private;

    // APPLICATION + SEEKTABLE bodies survive; PICTURE/PADDING bodies do not.
    assert!(
      cp.windows(app.len()).any(|w| w == app.as_slice()),
      "APPLICATION block must be kept"
    );
    assert!(
      cp.windows(18).any(|w| w == seektable.as_slice()),
      "SEEKTABLE block must be kept"
    );
    assert!(
      !cp.windows(picture.len()).any(|w| w == picture.as_slice()),
      "PICTURE block must be stripped"
    );
    // The final kept block (VORBIS_COMMENT) carries the last-block flag, and no
    // earlier kept-block header does.
    assert_eq!(cp[..4], *b"fLaC");
    // STREAMINFO header is the first block header at offset 4 — flag cleared.
    assert_eq!(cp[4] & 0x80, 0, "non-final kept block must clear last flag");
    // Exactly one block header in the kept chain has the last-block flag set.
    let last_flags = count_last_block_flags(cp);
    assert_eq!(last_flags, 1, "exactly one last-block flag expected");
  }

  /// Walk the kept-block chain in a codec-private blob and count how many block
  /// headers have the "last metadata block" flag set.
  fn count_last_block_flags(cp: &[u8]) -> usize {
    let mut pos = 4usize; // skip "fLaC"
    let mut count = 0usize;
    while pos + 4 <= cp.len() {
      if cp[pos] & 0x80 != 0 {
        count += 1;
      }
      let len = ((cp[pos + 1] as usize) << 16) | ((cp[pos + 2] as usize) << 8) | cp[pos + 3] as usize;
      pos += 4 + len;
    }
    count
  }

  // ---- PARSER-097: PICTURE blocks become attachments ------------------

  #[test]
  fn picture_blocks_become_attachments_with_generated_name() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = build_flac_with_picture_and_comment(2048);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.attachments.len(), 1);
    let att = &out.attachments[0];
    assert_eq!(att.id, 1);
    assert_eq!(att.file_name, "cover.jpg");
    assert_eq!(att.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(att.description.as_deref(), Some("Front cover"));
    assert_eq!(att.size, 2048);
  }

  #[test]
  fn large_picture_beyond_read_cap_becomes_attachment() {
    // PARSER-226: a >1 MiB picture is complete on disk; the bounded header read
    // doesn't cover the whole payload, yet the attachment is still surfaced
    // with the declared size.
    let data_len = 1024 * 1024 + 4096;
    let bytes = build_flac_with_picture_and_comment(data_len);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].size as usize, data_len);
    assert_eq!(out.attachments[0].file_name, "cover.jpg");
  }

  #[test]
  fn picture_header_fields_past_one_mib_are_read() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block_header(false, 0, 34));
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = (48_000u64 << 44) | ((1u64) << 41) | ((23u64) << 36) | 0u64;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    let description = "D".repeat(1024 * 1024 + 16);
    let picture = build_picture_block(3, "image/png", &description, 16);
    bytes.extend(block_header(true, 6, picture.len()));
    bytes.extend(picture);

    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(
      out.attachments[0].description.as_ref().unwrap().len(),
      description.len()
    );
  }

  // ---- PARSER-154: zero-length / absent picture data is dropped --------

  #[test]
  fn decode_picture_rejects_zero_data_length() {
    let block = build_picture_block(3, "image/jpeg", "Front", 0);
    assert!(decode_picture(&block, block.len()).is_none());
  }

  #[test]
  fn decode_picture_rejects_truncated_data() {
    // Declares 64 bytes of data but the block only carries 8.
    let mut block = build_picture_block(3, "image/jpeg", "Front", 8);
    let len = block.len();
    // Overwrite the data_length field (last 4 bytes before the data) to claim 64.
    block[len - 8 - 4..len - 8].copy_from_slice(&64u32.to_be_bytes());
    assert!(decode_picture(&block, block.len()).is_none());
  }

  #[test]
  fn decode_picture_accepts_present_non_empty_data() {
    let block = build_picture_block(3, "image/jpeg", "Front", 32);
    let p = decode_picture(&block, block.len()).unwrap();
    assert_eq!(p.data_length, 32);
    assert_eq!(p.mime_type, "image/jpeg");
  }

  #[test]
  fn decode_picture_accepts_payload_beyond_read_cap() {
    // PARSER-226: only the header fields are present in `body`; the declared
    // payload (5 MiB, set in the data_length field) is far larger than the
    // slice but fits the declared block length, so the picture is surfaced.
    let huge: u32 = 5 * 1024 * 1024;
    let mut header_only = Vec::new();
    header_only.extend_from_slice(&3u32.to_be_bytes()); // picture type
    header_only.extend_from_slice(&("image/jpeg".len() as u32).to_be_bytes());
    header_only.extend_from_slice(b"image/jpeg");
    header_only.extend_from_slice(&("Front".len() as u32).to_be_bytes());
    header_only.extend_from_slice(b"Front");
    header_only.extend_from_slice(&[0u8; 16]); // width/height/depth/colours
    header_only.extend_from_slice(&huge.to_be_bytes()); // declared data_length
    let block_length = header_only.len() + huge as usize;
    let p = decode_picture(&header_only, block_length).unwrap();
    assert_eq!(p.data_length, huge);
  }

  #[test]
  fn zero_length_picture_does_not_become_attachment() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block_header(false, 0, 34));
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = (48_000u64 << 44) | ((1u64) << 41) | ((23u64) << 36) | 0u64;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    // A PICTURE block with a valid MIME but zero data length must be dropped.
    let picture = build_picture_block(3, "image/jpeg", "Front cover", 0);
    bytes.extend(block_header(true, 6, picture.len()));
    bytes.extend(picture);
    use crate::media_metadata::deadline::Deadline;
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.attachments.is_empty());
  }

  #[test]
  fn picture_with_empty_mime_is_skipped() {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend(block_header(false, 0, 34));
    let mut info = vec![0u8; 34];
    info[..2].copy_from_slice(&4096u16.to_be_bytes());
    info[2..4].copy_from_slice(&4096u16.to_be_bytes());
    let packed = (48_000u64 << 44) | ((1u64) << 41) | ((23u64) << 36) | 0u64;
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend(info);
    let picture = build_picture_block(3, "", "", 16);
    bytes.extend(block_header(true, 6, picture.len()));
    bytes.extend(picture);
    use crate::media_metadata::deadline::Deadline;
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap();
    assert!(out.attachments.is_empty());
  }

  #[test]
  fn read_headers_returns_malformed_without_streaminfo() {
    use crate::media_metadata::deadline::Deadline;
    let bytes = b"fLaC".to_vec();
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let mut out = MediaMetadata::new("clip.flac", 0);
    let err = FlacReader
      .read_headers(&mut s, &Deadline::new(60_000), &mut out)
      .unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }));
  }
}
