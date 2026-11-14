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

//! Size, bit-rate and duration labels. Port of the React app's `format.ts`
//! (itself from BetterMediaInfo) and `formatHMS` from its `merge.ts`.
//!
//! The `K…` unit ladders divide by 1024, the `K…i` ladders by 1000 (the
//! labels are BetterMediaInfo's, so `Ki` means 1000 here).

use std::time::Duration;

use bmm_core::config::{ConfigFormatField, ConfigFormatting, FormatPrecision, FormatUnit};
use bmm_core::media_track::{MediaTrackCounts, MediaTrackType};

const KIBI: f64 = 1024.0;
const MEBI: f64 = 1_048_576.0;
const GIBI: f64 = 1_073_741_824.0;
const TEBI: f64 = 1_099_511_627_776.0;

/// The divisor and label of each step of a unit ladder, smallest first.
fn tiers(unit: &FormatUnit) -> &'static [(f64, &'static str)] {
  match unit {
    FormatUnit::K => &[(KIBI, "K")],
    FormatUnit::KM => &[(KIBI, "K"), (MEBI, "M")],
    FormatUnit::KMG => &[(KIBI, "K"), (MEBI, "M"), (GIBI, "G")],
    FormatUnit::KMGT => &[(KIBI, "K"), (MEBI, "M"), (GIBI, "G"), (TEBI, "T")],
    FormatUnit::KMi => &[(1e3, "Ki"), (1e6, "Mi")],
    FormatUnit::KMiGi => &[(1e3, "Ki"), (1e6, "Mi"), (1e9, "Gi")],
    FormatUnit::KMiGiTi => &[(1e3, "Ki"), (1e6, "Mi"), (1e9, "Gi"), (1e12, "Ti")],
  }
}

fn decimal_places(precision: &FormatPrecision) -> usize {
  match precision {
    FormatPrecision::Zero => 0,
    FormatPrecision::One => 1,
    FormatPrecision::Two => 2,
  }
}

/// Drop trailing fraction zeros and a trailing dot: `1.50` → `1.5`, `2.00` →
/// `2`. Integers are left alone.
fn trim_fraction_zeros(value: &str) -> &str {
  let mut value = value;
  if value.rfind('.').is_some_and(|dot| dot > 0) {
    value = value.trim_end_matches('0');
  }
  value.strip_suffix('.').unwrap_or(value)
}

/// The largest ladder step the value exceeds, with `field`'s precision; the
/// plain number below the first step; empty for negative or non-finite values.
fn format_value(value: f64, field: &ConfigFormatField, suffix: &str) -> String {
  if !value.is_finite() || value < 0.0 {
    return String::new();
  }
  let decimals = decimal_places(&field.precision);
  for (divisor, label) in tiers(&field.unit).iter().rev() {
    if value > *divisor {
      return format!(
        "{}{label}{suffix}",
        trim_fraction_zeros(&format!("{:.*}", decimals, value / divisor))
      );
    }
  }
  format!("{value}{suffix}")
}

/// The configured field of a track type; chapters, attachments and anything
/// else use the default (two decimals, K/M/G/T).
fn field_for(formatting: Option<&ConfigFormatting>, track_type: MediaTrackType, bit_rate: bool) -> ConfigFormatField {
  let stream = formatting.and_then(|formatting| match track_type {
    MediaTrackType::Video => Some(&formatting.video),
    MediaTrackType::Audio => Some(&formatting.audio),
    MediaTrackType::Subtitles => Some(&formatting.subtitle),
    _ => None,
  });
  match stream {
    Some(stream) if bit_rate => stream.bit_rate.clone(),
    Some(stream) => stream.size.clone(),
    None => ConfigFormatField::default(),
  }
}

/// A track's byte count with the configured size precision and unit, e.g.
/// `1.5GB`.
pub fn format_track_size(bytes: f64, track_type: MediaTrackType, formatting: Option<&ConfigFormatting>) -> String {
  format_value(bytes, &field_for(formatting, track_type, false), "B")
}

/// A whole file's byte count: two decimals on the K/M/G/T ladder, e.g.
/// `1.2GB`.
pub fn format_file_size(bytes: f64) -> String {
  format_value(bytes, &ConfigFormatField::default(), "B")
}

/// A track's bit rate with the configured precision and unit, e.g. `640Kbps`.
pub fn format_track_bit_rate(bps: f64, track_type: MediaTrackType, formatting: Option<&ConfigFormatting>) -> String {
  format_value(bps, &field_for(formatting, track_type, true), "bps")
}

/// A unit's track counts as a card summary shows them (`CardSummary.tsx`):
/// count and emoji per type that has any, e.g. `1 🎞️ 2 🔊 3 💬`; empty
/// without tracks.
pub fn format_track_counts(counts: &MediaTrackCounts) -> String {
  [
    (counts.video, "🎞️"),
    (counts.audio, "🔊"),
    (counts.subtitles, "💬"),
    (counts.chapters, "📑"),
    (counts.attachments, "📎"),
  ]
  .into_iter()
  .filter(|(count, _)| *count > 0)
  .map(|(count, emoji)| format!("{count} {emoji}"))
  .collect::<Vec<_>>()
  .join(" ")
}

/// `HH:MM:SS` with whole seconds rounded down (hours keep growing past 99);
/// `--:--:--` when there is no duration (not started, or a negative span).
pub fn format_hms(duration: Option<Duration>) -> String {
  let Some(duration) = duration else {
    return "--:--:--".to_string();
  };
  let total = duration.as_secs();
  format!("{:02}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
}

#[cfg(test)]
mod tests {
  use super::*;
  use bmm_core::config::ConfigStreamFormat;

  fn field(precision: FormatPrecision, unit: FormatUnit) -> ConfigFormatField {
    ConfigFormatField { precision, unit }
  }

  #[test]
  fn formats_track_counts_like_the_card_summary() {
    assert_eq!(format_track_counts(&MediaTrackCounts::default()), "");
    let counts = MediaTrackCounts {
      video: 1,
      audio: 2,
      subtitles: 0,
      chapters: 1,
      attachments: 3,
    };
    assert_eq!(format_track_counts(&counts), "1 🎞️ 2 🔊 1 📑 3 📎");
  }

  #[test]
  fn formats_file_sizes_on_the_default_ladder() {
    assert_eq!(format_file_size(0.0), "0B");
    assert_eq!(format_file_size(512.0), "512B");
    // Only values above a divisor use its step.
    assert_eq!(format_file_size(1024.0), "1024B");
    assert_eq!(format_file_size(1025.0), "1KB");
    assert_eq!(format_file_size(1536.0), "1.5KB");
    assert_eq!(format_file_size(1.25 * GIBI), "1.25GB");
    assert_eq!(format_file_size(3.0 * TEBI), "3TB");
    assert_eq!(format_file_size(2048.0 * TEBI), "2048TB");
    // Rust rounds an exact tie to even: 1152 / 1024 = 1.125.
    assert_eq!(format_file_size(1152.0), "1.12KB");
    assert_eq!(format_file_size(1160.0), "1.13KB");
    assert_eq!(format_file_size(-0.0), "-0B");
    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
      assert_eq!(format_file_size(invalid), "");
    }
  }

  #[test]
  fn honours_precision_and_unit() {
    let cases = [
      (FormatPrecision::Zero, FormatUnit::KMGT, 1.6 * MEBI, "2MB"),
      // Exact ties round to even: 1.25 → 1.2, 4.75 → 4.8.
      (FormatPrecision::One, FormatUnit::KMGT, 1.25 * MEBI, "1.2MB"),
      (FormatPrecision::Two, FormatUnit::K, MEBI, "1024KB"),
      (FormatPrecision::Two, FormatUnit::KM, 5.0 * GIBI, "5120MB"),
      (FormatPrecision::Two, FormatUnit::KMG, 2.0 * TEBI, "2048GB"),
      (FormatPrecision::Two, FormatUnit::KMi, 1500.0, "1.5KiB"),
      (FormatPrecision::Two, FormatUnit::KMi, 1e6, "1000KiB"),
      (FormatPrecision::Two, FormatUnit::KMi, 2.5e9, "2500MiB"),
      (FormatPrecision::Two, FormatUnit::KMiGi, 2.5e9, "2.5GiB"),
      (FormatPrecision::One, FormatUnit::KMiGiTi, 4.75e12, "4.8TiB"),
      (FormatPrecision::Two, FormatUnit::KMiGiTi, 999.0, "999B"),
    ];
    for (precision, unit, value, expected) in cases {
      assert_eq!(format_value(value, &field(precision, unit), "B"), expected, "{value}");
    }
  }

  #[test]
  fn picks_the_track_type_field() {
    let formatting = ConfigFormatting {
      video: ConfigStreamFormat::default(),
      audio: ConfigStreamFormat {
        bit_rate: field(FormatPrecision::Zero, FormatUnit::KMi),
        size: field(FormatPrecision::One, FormatUnit::KM),
      },
      subtitle: ConfigStreamFormat {
        bit_rate: field(FormatPrecision::Two, FormatUnit::K),
        size: field(FormatPrecision::Zero, FormatUnit::K),
      },
    };
    let formatting = Some(&formatting);
    assert_eq!(
      format_track_bit_rate(640_000.0, MediaTrackType::Audio, formatting),
      "640Kibps"
    );
    assert_eq!(
      format_track_size(1.5 * GIBI, MediaTrackType::Audio, formatting),
      "1536MB"
    );
    // 2.5 KB is an exact tie and rounds to even.
    assert_eq!(format_track_size(2_560.0, MediaTrackType::Subtitles, formatting), "2KB");
    assert_eq!(
      format_track_bit_rate(640_000.0, MediaTrackType::Video, formatting),
      "625Kbps"
    );
    // Chapters, attachments and no formatting use the default field.
    assert_eq!(
      format_track_size(1.5 * GIBI, MediaTrackType::Attachment, formatting),
      "1.5GB"
    );
    assert_eq!(format_track_size(1.5 * GIBI, MediaTrackType::Audio, None), "1.5GB");
    assert_eq!(format_track_bit_rate(800.0, MediaTrackType::Audio, None), "800bps");
  }

  #[test]
  fn trims_fraction_zeros() {
    assert_eq!(trim_fraction_zeros("1.50"), "1.5");
    assert_eq!(trim_fraction_zeros("2.00"), "2");
    assert_eq!(trim_fraction_zeros("100"), "100");
    assert_eq!(trim_fraction_zeros("100.0"), "100");
    assert_eq!(trim_fraction_zeros("0.05"), "0.05");
  }

  #[test]
  fn formats_durations_as_hours_minutes_seconds() {
    assert_eq!(format_hms(None), "--:--:--");
    assert_eq!(format_hms(Some(Duration::ZERO)), "00:00:00");
    assert_eq!(format_hms(Some(Duration::from_millis(3_661_999))), "01:01:01");
    assert_eq!(format_hms(Some(Duration::from_secs(100 * 3600 + 59))), "100:00:59");
  }
}
