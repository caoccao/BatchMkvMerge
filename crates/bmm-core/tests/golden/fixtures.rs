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

//! Synthetic media files for the merge-argument golden test
//! (`tests/merge_args_golden.rs`): a Matroska movie with every row type, an
//! audio-only Matroska file and an SRT subtitle, named like the members of one
//! "Group by file name" tree. Kept in a sub-directory so Cargo does not build
//! it as a test crate of its own.

use bmm_core::media_metadata::matroska::ids;

/// One synthetic input: the fixture name used by the golden files, the file
/// extension and the bytes.
pub struct Fixture {
  pub name: &'static str,
  pub extension: &'static str,
  pub bytes: Vec<u8>,
}

/// Every fixture the golden files may reference.
pub fn fixtures() -> Vec<Fixture> {
  vec![
    Fixture {
      name: "movie",
      extension: "mkv",
      bytes: movie_mkv(),
    },
    Fixture {
      name: "movie.ja",
      extension: "mka",
      bytes: commentary_mka(),
    },
    Fixture {
      name: "movie.en",
      extension: "srt",
      bytes: b"1\n00:00:01,000 --> 00:00:02,500\nHello.\n\n2\n00:00:03,000 --> 00:00:04,000\nBye.\n".to_vec(),
    },
  ]
}

fn id_bytes(id: u32) -> Vec<u8> {
  let bytes = id.to_be_bytes();
  let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(3);
  bytes[first..].to_vec()
}

fn size_bytes(size: u64) -> Vec<u8> {
  for width in 1u32..=8 {
    if size < (1u64 << (7 * width)) - 1 {
      let value = (1u64 << (7 * width)) | size;
      return value.to_be_bytes()[(8 - width as usize)..].to_vec();
    }
  }
  panic!("element of {size} bytes is too large");
}

fn element(id: u32, payload: &[u8]) -> Vec<u8> {
  let mut out = id_bytes(id);
  out.extend(size_bytes(payload.len() as u64));
  out.extend_from_slice(payload);
  out
}

fn uint(id: u32, value: u64) -> Vec<u8> {
  let bytes = value.to_be_bytes();
  let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
  element(id, &bytes[first..])
}

fn string(id: u32, value: &str) -> Vec<u8> {
  element(id, value.as_bytes())
}

fn float(id: u32, value: f64) -> Vec<u8> {
  element(id, &value.to_bits().to_be_bytes())
}

fn concat(parts: &[Vec<u8>]) -> Vec<u8> {
  parts.concat()
}

fn ebml_head() -> Vec<u8> {
  element(
    ids::EBML,
    &concat(&[
      uint(ids::EBML_VERSION, 1),
      uint(ids::EBML_READ_VERSION, 1),
      uint(ids::EBML_MAX_ID_LENGTH, 4),
      uint(ids::EBML_MAX_SIZE_LENGTH, 8),
      string(ids::DOC_TYPE, "matroska"),
      uint(ids::DOC_TYPE_VERSION, 4),
      uint(ids::DOC_TYPE_READ_VERSION, 2),
    ]),
  )
}

fn info(title: &str) -> Vec<u8> {
  element(
    ids::INFO,
    &concat(&[
      uint(ids::TIMESTAMP_SCALE, 1_000_000),
      float(ids::DURATION, 5_400_000.0),
      string(ids::TITLE, title),
      string(ids::MUXING_APP, "libebml v1.4.5 + libmatroska v1.7.1"),
      string(ids::WRITING_APP, "mkvmerge v89.0"),
    ]),
  )
}

/// A track entry with the common fields; `extra` carries the codec and the
/// type-specific sub-element.
fn track_entry(
  number: u64,
  track_type: u64,
  language: &str,
  flags: (Option<u64>, Option<u64>),
  extra: &[Vec<u8>],
) -> Vec<u8> {
  let mut parts = vec![
    uint(ids::TRACK_NUMBER, number),
    uint(ids::TRACK_UID, 0xB0_0000 + number),
    uint(ids::TRACK_TYPE, track_type),
    string(ids::TRACK_LANGUAGE, language),
  ];
  if let Some(default) = flags.0 {
    parts.push(uint(ids::FLAG_DEFAULT, default));
  }
  if let Some(forced) = flags.1 {
    parts.push(uint(ids::FLAG_FORCED, forced));
  }
  parts.extend_from_slice(extra);
  element(ids::TRACK_ENTRY, &concat(&parts))
}

fn audio(sampling_frequency: f64, channels: u64) -> Vec<u8> {
  element(
    ids::TRACK_AUDIO,
    &concat(&[
      float(ids::AUDIO_SAMPLING_FREQ, sampling_frequency),
      uint(ids::AUDIO_CHANNELS, channels),
    ]),
  )
}

/// Statistics tags (as mkvmerge writes them) for one track.
fn statistics(track_uid: u64, bit_rate: &str, bytes: &str) -> Vec<u8> {
  let simple = |name: &str, value: &str| {
    element(
      ids::TAG_SIMPLE,
      &concat(&[string(ids::TAG_NAME, name), string(ids::TAG_STRING, value)]),
    )
  };
  element(
    ids::TAG,
    &concat(&[
      element(
        ids::TAG_TARGETS,
        &concat(&[
          uint(ids::TAG_TARGET_TYPE_VALUE, 50),
          uint(ids::TAG_TRACK_UID, track_uid),
        ]),
      ),
      simple("BPS", bit_rate),
      simple("NUMBER_OF_BYTES", bytes),
    ]),
  )
}

fn attached_file(name: &str, mime_type: &str, uid: u64, size: usize) -> Vec<u8> {
  element(
    ids::ATTACHED_FILE,
    &concat(&[
      string(ids::FILE_NAME, name),
      string(ids::FILE_MIME_TYPE, mime_type),
      uint(ids::FILE_UID, uid),
      element(ids::FILE_DATA, &vec![0x5A; size]),
    ]),
  )
}

fn chapter(uid: u64, start_ns: u64, title: &str) -> Vec<u8> {
  element(
    ids::CHAPTER_ATOM,
    &concat(&[
      uint(ids::CHAPTER_UID, uid),
      uint(ids::CHAPTER_TIMESTAMP_START, start_ns),
      element(ids::CHAPTER_DISPLAY, &string(ids::CHAP_STRING, title)),
    ]),
  )
}

/// Video, two audio and two subtitle tracks with names, languages, flags and
/// statistics tags, three chapters and two attachments.
fn movie_mkv() -> Vec<u8> {
  let tracks = element(
    ids::TRACKS,
    &concat(&[
      track_entry(
        1,
        1,
        "eng",
        (Some(1), None),
        &[
          string(ids::CODEC_ID, "V_MPEG4/ISO/AVC"),
          string(ids::TRACK_NAME, "Main Feature"),
          uint(ids::DEFAULT_DURATION, 41_708_333),
          element(
            ids::TRACK_VIDEO,
            &concat(&[uint(ids::VIDEO_PIXEL_WIDTH, 1920), uint(ids::VIDEO_PIXEL_HEIGHT, 1080)]),
          ),
        ],
      ),
      track_entry(
        2,
        2,
        "jpn",
        (Some(1), None),
        &[
          string(ids::CODEC_ID, "A_AC3"),
          string(ids::TRACK_NAME, "Japanese 5.1"),
          audio(48_000.0, 6),
        ],
      ),
      track_entry(
        3,
        2,
        "eng",
        (Some(0), None),
        &[string(ids::CODEC_ID, "A_AAC"), audio(44_100.0, 2)],
      ),
      track_entry(
        4,
        17,
        "fre",
        (Some(0), Some(1)),
        &[string(ids::CODEC_ID, "S_TEXT/UTF8")],
      ),
      track_entry(
        5,
        17,
        "chi",
        (None, None),
        &[
          string(ids::CODEC_ID, "S_TEXT/ASS"),
          string(ids::TRACK_NAME, "Simplified Chinese"),
        ],
      ),
    ]),
  );
  let chapters = element(
    ids::CHAPTERS,
    &element(
      ids::EDITION_ENTRY,
      &concat(&[
        chapter(1, 0, "Opening"),
        chapter(2, 600_000_000_000, "Middle"),
        chapter(3, 4_800_000_000_000, "End"),
      ]),
    ),
  );
  let attachments = element(
    ids::ATTACHMENTS,
    &concat(&[
      attached_file("cover.jpg", "image/jpeg", 0xA1, 2048),
      attached_file("Subtitle Font.TTF", "application/x-truetype-font", 0xA2, 512),
    ]),
  );
  let tags = element(
    ids::TAGS,
    &concat(&[
      statistics(0xB0_0001, "8000000", "5400000000"),
      statistics(0xB0_0002, "640000", "432000000"),
    ]),
  );
  let mut file = ebml_head();
  file.extend(element(
    ids::SEGMENT,
    &concat(&[info("Synthetic Movie"), tracks, chapters, attachments, tags]),
  ));
  file
}

/// One stereo Opus commentary track.
fn commentary_mka() -> Vec<u8> {
  let tracks = element(
    ids::TRACKS,
    &track_entry(
      1,
      2,
      "jpn",
      (Some(0), None),
      &[
        string(ids::CODEC_ID, "A_OPUS"),
        string(ids::TRACK_NAME, "Commentary"),
        audio(48_000.0, 2),
      ],
    ),
  );
  let mut file = ebml_head();
  file.extend(element(ids::SEGMENT, &concat(&[info("Commentary"), tracks])));
  file
}
