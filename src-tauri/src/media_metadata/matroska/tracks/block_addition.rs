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
//! For Phase 3 we only validate / collect the tuples — the values feed
//! `CommonTrackProperties.max_block_addition_id` for cross-track reference.
//! Codec-specific decoders (Dolby Vision RPU, HDR10+) consume the extra
//! data in Phase 8.

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;

use crate::media_metadata::matroska::ebml::{self, ChildAction, ElementHeader};
use crate::media_metadata::matroska::ids;

const BINARY_CAP: u64 = 16 * 1024;

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
          mapping.id_name = Some(ebml::read_string(src, child, 1024)?);
        }
        ids::BLOCK_ADD_ID_VALUE => {
          mapping.id_value = Some(ebml::read_uint(src, child)?);
        }
        ids::BLOCK_ADD_ID_TYPE => {
          mapping.id_type = Some(ebml::read_uint(src, child)?);
        }
        ids::BLOCK_ADD_ID_EXTRA_DATA => {
          mapping.id_extra_data = Some(ebml::read_binary(src, child, BINARY_CAP)?);
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
  fn mapping_with_extra_data_kept_and_capped() {
    let mut payload = Vec::new();
    payload.extend(encode_element(ids::BLOCK_ADD_ID_EXTRA_DATA, 2, &[1u8; 8]));
    payload.extend(encode_element_uint(ids::BLOCK_ADD_ID_VALUE, 2, 1));
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_extra_data.as_ref().map(|v| v.len()), Some(8));
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
  fn unknown_children_are_ignored() {
    let mut payload = Vec::new();
    payload.extend(encode_element(0x80, 1, &[1, 2, 3])); // bogus
    payload.extend(encode_element_uint(ids::BLOCK_ADD_ID_VALUE, 2, 7));
    let m = parse_mapping(payload).unwrap();
    assert_eq!(m.id_value, Some(7));
  }
}
