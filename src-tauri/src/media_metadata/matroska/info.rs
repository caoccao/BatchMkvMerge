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

//! Segment / Info parsing.  Mirrors `r_matroska.cpp::read_headers_info`
//! (lines 1123-1204) — but produces our own `ContainerProperties` instead of
//! writing into a libmatroska tree.
//!
//! Fields covered:
//! - TimestampScale (default 1_000_000 ns/tick).
//! - Duration (raw + scaled — we expose nanoseconds + formatted string).
//! - Title.
//! - MuxingApp / WritingApp (raw + parsed app name + numeric version).
//! - SegmentUID / PrevUID / NextUID (hex-encoded for JS safe-integer reasons).
//! - DateUTC (Matroska epoch 2001-01-01T00:00:00Z + signed ns offset).

use crate::media_metadata::deadline::Deadline;
use crate::media_metadata::error::ParseError;
use crate::media_metadata::io::file_source::FileSource;
use crate::media_metadata::model::MediaMetadata;
use crate::media_metadata::model::duration::DurationValue;

use super::ebml::{self, ChildAction, ElementHeader};
use super::ids;
use super::writing_app;

/// Cap for SegmentUID / NextUID / PrevUID payloads — spec says they're 16
/// bytes but we accept up to 32 to be lenient.
const UID_CAP: u64 = 32;

/// Matroska epoch: 2001-01-01T00:00:00Z, seconds-since-Unix-epoch.
const MATROSKA_EPOCH_UNIX: i64 = 978_307_200;

pub fn parse(
  src: &mut FileSource,
  parent: &ElementHeader,
  deadline: &Deadline,
  out: &mut MediaMetadata,
) -> Result<(), ParseError> {
  let mut timestamp_scale: u64 = 1_000_000; // mkvmerge default
  let mut raw_duration: Option<f64> = None;

  ebml::walk_children(src, parent, "matroska::info", deadline, |src, child| {
    match child.id {
      ids::TIMESTAMP_SCALE => {
        timestamp_scale = ebml::read_uint(src, child)?;
        Ok(ChildAction::Consumed)
      }
      ids::DURATION => {
        raw_duration = Some(ebml::read_float(src, child)?);
        Ok(ChildAction::Consumed)
      }
      ids::TITLE => {
        out.container.properties.title = Some(ebml::read_string(src, child, deadline.max_element_size())?);
        Ok(ChildAction::Consumed)
      }
      ids::MUXING_APP => {
        let s = ebml::read_string(src, child, deadline.max_element_size())?;
        out.container.properties.muxing_app = Some(s);
        Ok(ChildAction::Consumed)
      }
      ids::WRITING_APP => {
        let s = ebml::read_string(src, child, deadline.max_element_size())?;
        // Mirror mkvtoolnix's lower-case + version-extraction logic.
        let parsed = writing_app::parse(&s);
        out.container.properties.writing_app = Some(parsed.into_display(&s));
        Ok(ChildAction::Consumed)
      }
      ids::SEGMENT_UID => {
        let bytes = ebml::read_binary(src, child, UID_CAP)?;
        out.container.properties.segment_uid_hex = Some(hex_encode(&bytes));
        Ok(ChildAction::Consumed)
      }
      ids::PREV_UID => {
        let bytes = ebml::read_binary(src, child, UID_CAP)?;
        out.container.properties.previous_segment_uid_hex = Some(hex_encode(&bytes));
        Ok(ChildAction::Consumed)
      }
      ids::NEXT_UID => {
        let bytes = ebml::read_binary(src, child, UID_CAP)?;
        out.container.properties.next_segment_uid_hex = Some(hex_encode(&bytes));
        Ok(ChildAction::Consumed)
      }
      ids::DATE_UTC => {
        let ns_since_matroska_epoch = ebml::read_int(src, child)?;
        out.container.properties.date_utc = Some(format_matroska_date(ns_since_matroska_epoch));
        Ok(ChildAction::Consumed)
      }
      _ => Ok(ChildAction::Skip),
    }
  })?;

  out.container.properties.timestamp_scale = Some(timestamp_scale);
  if let Some(raw) = raw_duration {
    let ns = (raw * timestamp_scale as f64).round() as u64;
    out.container.properties.duration = Some(DurationValue::from_ns(ns));
  }
  Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{:02x}", b));
  }
  s
}

/// Convert a Matroska-epoch nanosecond offset into an ISO-8601 string.  We
/// emit the timestamp in UTC without sub-second precision (matches what
/// mkvmerge -J shows in identification JSON).
fn format_matroska_date(ns_since_epoch: i64) -> String {
  let total_seconds = MATROSKA_EPOCH_UNIX + ns_since_epoch.div_euclid(1_000_000_000);
  format_unix_seconds_utc(total_seconds)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` without any external date
/// dependency.  Handles the 1970..=9999 range; values outside come out as
/// the closest in-range timestamp.
fn format_unix_seconds_utc(secs: i64) -> String {
  // Days from Unix epoch and remaining seconds-of-day.
  let mut days = secs.div_euclid(86_400);
  let seconds_of_day = secs.rem_euclid(86_400) as u64;
  let hour = (seconds_of_day / 3_600) as u32;
  let minute = ((seconds_of_day % 3_600) / 60) as u32;
  let second = (seconds_of_day % 60) as u32;

  // Civil-from-days algorithm (Howard Hinnant, public-domain).
  days += 719_468;
  let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
  let doe = (days - era * 146_097) as u32;
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let y = yoe as i64 + era * 400;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = doy - (153 * mp + 2) / 5 + 1;
  let m = if mp < 10 { mp + 3 } else { mp - 9 };
  let year = if m <= 2 { y + 1 } else { y };

  format!("{year:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::media_metadata::matroska::ebml::{
    encode_element, encode_element_float, encode_element_string, encode_element_uint,
  };
  use std::io::Cursor;

  fn no_deadline() -> Deadline {
    Deadline::new(60_000)
  }

  fn build_info(payload: Vec<u8>) -> (Vec<u8>, ElementHeader, FileSource) {
    let info_bytes = encode_element(ids::INFO, 4, &payload);
    let mut s = FileSource::from_reader_for_test(Cursor::new(info_bytes.clone()));
    let header = ebml::read_element_header(&mut s).unwrap();
    (info_bytes, header, s)
  }

  #[test]
  fn parse_extracts_basic_metadata() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TIMESTAMP_SCALE, 3, 1_000_000));
    payload.extend(encode_element_float(ids::DURATION, 2, 60_000.0));
    payload.extend(encode_element_string(ids::TITLE, 2, "Test Movie"));
    payload.extend(encode_element_string(ids::MUXING_APP, 2, "libmkv"));
    payload.extend(encode_element_string(ids::WRITING_APP, 2, "mkvmerge v89"));
    payload.extend(encode_element(ids::SEGMENT_UID, 2, &[0xDE, 0xAD, 0xBE, 0xEF]));

    let (_bytes, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 100);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();

    let p = &out.container.properties;
    assert_eq!(p.timestamp_scale, Some(1_000_000));
    assert_eq!(
      p.duration.as_ref().map(|d| d.ns),
      Some(60_000 * 1_000_000) // 60 ms in ns × scale=1ms — i.e. 60 s
    );
    assert_eq!(p.title.as_deref(), Some("Test Movie"));
    assert_eq!(p.muxing_app.as_deref(), Some("libmkv"));
    assert!(p.writing_app.is_some());
    assert_eq!(p.segment_uid_hex.as_deref(), Some("deadbeef"));
  }

  #[test]
  fn info_strings_use_shared_element_budget() {
    let title = "T".repeat(5 * 1024);
    let payload = encode_element_string(ids::TITLE, 2, &title);
    let (_bytes, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 100);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.title.as_deref(), Some(title.as_str()));
  }

  #[test]
  fn default_timestamp_scale_is_1_million() {
    let (_b, header, mut s) = build_info(Vec::new());
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(1_000_000));
    assert!(out.container.properties.duration.is_none());
  }

  #[test]
  fn non_default_timestamp_scale_used_in_duration() {
    let mut payload = Vec::new();
    payload.extend(encode_element_uint(ids::TIMESTAMP_SCALE, 3, 100_000));
    payload.extend(encode_element_float(ids::DURATION, 2, 100.0));
    let (_b, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    // 100 ticks × 100_000 ns = 10_000_000 ns = 10 ms
    assert_eq!(
      out.container.properties.duration.as_ref().map(|d| d.ns),
      Some(10_000_000)
    );
  }

  #[test]
  fn segment_uid_hex_round_trip() {
    let payload = encode_element(ids::SEGMENT_UID, 2, &[0x01, 0x23, 0x45, 0x67]);
    let (_b, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.segment_uid_hex.as_deref(), Some("01234567"));
  }

  #[test]
  fn date_utc_formats_as_iso_8601() {
    // ns offset from Matroska epoch (2001-01-01) — pick 1 year (no leap)
    let one_year_ns: i64 = 365 * 86_400 * 1_000_000_000;
    let payload = encode_element(ids::DATE_UTC, 2, &one_year_ns.to_be_bytes());
    let (_b, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    // Should be 2002-01-01T00:00:00Z
    assert_eq!(
      out.container.properties.date_utc.as_deref(),
      Some("2002-01-01T00:00:00Z")
    );
  }

  #[test]
  fn negative_date_utc_formats_correctly() {
    // ns offset of -1 second from matroska epoch (= 2000-12-31T23:59:59Z)
    let neg_ns: i64 = -1_000_000_000;
    let payload = encode_element(ids::DATE_UTC, 2, &neg_ns.to_be_bytes());
    let (_b, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(
      out.container.properties.date_utc.as_deref(),
      Some("2000-12-31T23:59:59Z")
    );
  }

  #[test]
  fn matroska_epoch_unix_timestamp_known() {
    // 2001-01-01T00:00:00Z = 978307200
    assert_eq!(MATROSKA_EPOCH_UNIX, 978_307_200);
    // Format the epoch second itself
    assert_eq!(format_unix_seconds_utc(MATROSKA_EPOCH_UNIX), "2001-01-01T00:00:00Z");
  }

  #[test]
  fn hex_encoding_is_lower_case() {
    assert_eq!(hex_encode(&[0xAB, 0xCD]), "abcd");
    assert_eq!(hex_encode(&[]), "");
  }

  #[test]
  fn unknown_children_are_skipped_quietly() {
    // Random unknown element followed by TimestampScale — both must be
    // processed without erroring.
    let mut payload = Vec::new();
    payload.extend(encode_element(0x80, 1, &[1, 2, 3]));
    payload.extend(encode_element_uint(ids::TIMESTAMP_SCALE, 3, 2_000_000));
    let (_b, header, mut s) = build_info(payload);
    let mut out = MediaMetadata::new("clip.mkv", 0);
    parse(&mut s, &header, &no_deadline(), &mut out).unwrap();
    assert_eq!(out.container.properties.timestamp_scale, Some(2_000_000));
  }
}
