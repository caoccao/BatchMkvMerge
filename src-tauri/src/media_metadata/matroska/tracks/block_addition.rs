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

//! KaxBlockAdditionMapping walker.  Port of the loop in
//! `r_matroska.cpp:1465-1479` — extracts `(id_name, id_type, id_value,
//! id_extra_data)` tuples per BlockAdditionMapping child of a TrackEntry.
//!
//! PARSER-186: the validated tuples are converted to the wire-format
//! `BlockAdditionMapping { id_type, data_hex, id_name, id_value }` via
//! [`BlockAdditionMapping::to_model`] and carried onto the video track,
//! mirroring how `r_matroska.cpp:1465-1479` stores them on
//! `track->block_addition_mappings`.  PARSER-228: the `BlockAddIDName` and
//! `BlockAddIDValue` are now preserved on the model alongside the type and
//! extra data.  Dolby Vision / HDR10+ configuration records ride through here
//! verbatim.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::matroska::ebml::{self, ChildAction, ElementHeader};
use crate::media_metadata::matroska::ids;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockAdditionMapping {
  pub id_name: Option<String>,
  pub id_value: Option<u64>,
  pub id_type: Option<u64>,
  pub id_extra_data: Option<Vec<u8>>,
}

impl BlockAdditionMapping {
  /// Mirrors `block_addition_mapping_t::is_valid` (matroska_common.cpp).
  /// At minimum one of value / type / extra_data must be present.
  pub fn is_valid(&self) -> bool {
    self.id_value.is_some() || self.id_type.is_some() || self.id_extra_data.is_some()
  }

  /// Render a numeric `BlockAddIDType` as the source FOURCC string when its
  /// four big-endian bytes are all printable ASCII (mkvtoolnix stores the
  /// type as `fourcc_c{"dvvC"}.value()` — a packed u32, see
  /// `common/dovi_meta.cpp:329`), otherwise as the decimal value.  Keeps the
  /// `id_type` field consistent with the MP4 / IVF block-addition output,
  /// where Dolby Vision mappings are keyed by the `dvcC` / `dvvC` FOURCC.
  fn render_id_type(value: u64) -> String {
    if value <= u64::from(u32::MAX) {
      let bytes = (value as u32).to_be_bytes();
      if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return bytes.iter().map(|b| *b as char).collect();
      }
    }
    value.to_string()
  }

  /// Convert the parsed Matroska mapping into the wire-format
  /// [`crate::media_metadata::model::track_properties_video::BlockAdditionMapping`]
  /// shape (`id_type` string + hex-encoded `data_hex`), matching the MP4 / IVF
  /// representation.  Mirrors `r_matroska.cpp:1465-1479` carrying the parsed
  /// mapping onto the track.
  pub fn to_model(&self) -> crate::media_metadata::model::track_properties_video::BlockAdditionMapping {
    let id_type = self.id_type.map(Self::render_id_type).unwrap_or_default();
    let data_hex = self
      .id_extra_data
      .as_deref()
      .map(|bytes| bytes.iter().map(|b| format!("{:02x}", b)).collect())
      .unwrap_or_default();
    // PARSER-228: carry the BlockAddIDName / BlockAddIDValue through to the
    // wire model so mappings keyed by value (or carrying a useful name) keep
    // that information.
    crate::media_metadata::model::track_properties_video::BlockAdditionMapping {
      id_type,
      data_hex,
      id_name: self.id_name.clone(),
      id_value: self.id_value,
    }
  }
}

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
) -> Result<Option<BlockAdditionMapping>, ParseError> {
  let mut mapping = BlockAdditionMapping {
    id_name: None,
    id_value: None,
    id_type: None,
    id_extra_data: None,
  };
  ebml::walk_children(
    src,
    parent,
    "matroska::block_addition_mapping",
    deadline,
    |src, child| {
      match child.id {
        ids::BLOCK_ADD_ID_NAME => {
          mapping.id_name = Some(ebml::read_string(src, child, deadline.max_element_size())?);
        }
        ids::BLOCK_ADD_ID_VALUE => {
          mapping.id_value = Some(ebml::read_uint(src, child)?);
        }
        ids::BLOCK_ADD_ID_TYPE => {
          mapping.id_type = Some(ebml::read_uint(src, child)?);
        }
        ids::BLOCK_ADD_ID_EXTRA_DATA => {
          mapping.id_extra_data = Some(ebml::read_binary(src, child, deadline.max_element_size())?);
        }
        _ => return Ok(ChildAction::Skip),
      }
      Ok(ChildAction::Consumed)
    },
  )?;
  Ok(if mapping.is_valid() { Some(mapping) } else { None })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{encode_element, encode_element_string, encode_element_uint};
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn parse_mapping(payload: Vec<u8>) -> Option<BlockAdditionMapping> {
    let bytes = encode_element(ids::BLOCK_ADDITION_MAPPING, 2, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(bytes));
    let header = ebml::read_element_header(&mut s).unwrap();
    parse(&mut s, &header, &no_deadline()).unwrap()
  }

  #[test]
  fn mapping_with_only_name_is_rejected_as_invalid() {
    let payload = encode_element_string(ids::BLOCK_ADD_ID_NAME, 2, "Dolby Vision");
    let m = parse_mapping(payload);
    assert!(m.is_none());
  }

  #[test]
  fn mapping_with_value_is_kept() {
    let payload = encode_element_uint(ids::BLOCK_ADD_ID_VALUE, 2, 4);
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_value, Some(4));
  }

  #[test]
  fn mapping_with_type_is_kept() {
    let payload = encode_element_uint(ids::BLOCK_ADD_ID_TYPE, 2, 0xDEADBEEF);
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_type, Some(0xDEADBEEF));
  }

  #[test]
  fn mapping_with_extra_data_uses_shared_element_budget() {
    let mut payload = Vec::new();
    payload.extend(encode_element(ids::BLOCK_ADD_ID_EXTRA_DATA, 2, &vec![1u8; 17 * 1024]));
    payload.extend(encode_element_uint(ids::BLOCK_ADD_ID_VALUE, 2, 1));
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_extra_data.as_ref().map(|v| v.len()), Some(17 * 1024));
  }

  #[test]
  fn is_valid_predicate() {
    let m = BlockAdditionMapping {
      id_name: Some("x".to_owned()),
      id_value: None,
      id_type: None,
      id_extra_data: None,
    };
    assert!(!m.is_valid());

    let m2 = BlockAdditionMapping {
      id_name: None,
      id_value: Some(1),
      id_type: None,
      id_extra_data: None,
    };
    assert!(m2.is_valid());
  }

  #[test]
  fn to_model_renders_fourcc_type_and_hex_extra_data() {
    let m = BlockAdditionMapping {
      id_name: Some("Dolby Vision configuration".to_owned()),
      id_value: None,
      id_type: Some(u64::from(u32::from_be_bytes(*b"dvvC"))),
      id_extra_data: Some(vec![0x01, 0x00, 0x4a, 0xff]),
    };
    let model = m.to_model();
    assert_eq!(model.id_type, "dvvC");
    assert_eq!(model.data_hex, "01004aff");
    // PARSER-228: the name and (absent) value travel through to the model.
    assert_eq!(model.id_name.as_deref(), Some("Dolby Vision configuration"));
    assert_eq!(model.id_value, None);
  }

  #[test]
  fn to_model_carries_name_and_value() {
    let m = BlockAdditionMapping {
      id_name: Some("HDR10+".to_owned()),
      id_value: Some(4),
      id_type: Some(7),
      id_extra_data: None,
    };
    let model = m.to_model();
    assert_eq!(model.id_name.as_deref(), Some("HDR10+"));
    assert_eq!(model.id_value, Some(4));
  }

  #[test]
  fn to_model_renders_non_printable_type_as_decimal() {
    let m = BlockAdditionMapping {
      id_name: None,
      id_value: None,
      id_type: Some(7),
      id_extra_data: None,
    };
    let model = m.to_model();
    assert_eq!(model.id_type, "7");
    assert_eq!(model.data_hex, "");
  }

  #[test]
  fn to_model_with_no_type_yields_empty_id_type() {
    let m = BlockAdditionMapping {
      id_name: None,
      id_value: Some(3),
      id_type: None,
      id_extra_data: None,
    };
    let model = m.to_model();
    assert_eq!(model.id_type, "");
    assert_eq!(model.data_hex, "");
  }

  #[test]
  fn unknown_children_are_ignored() {
    let mut payload = Vec::new();
    payload.extend(encode_element(0x80, 1, &[1, 2, 3])); // bogus
    payload.extend(encode_element_uint(ids::BLOCK_ADD_ID_VALUE, 2, 7));
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_value, Some(7));
  }
}
