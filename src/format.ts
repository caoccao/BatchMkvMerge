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

import type {
  ConfigFormatField,
  ConfigFormatting,
} from "./protocol";
import { FormatPrecision, FormatUnit } from "./protocol";

// Ported from ../BetterMediaInfo/src/lib/format.ts. `K*` ladders use 1024-step
// divisors; `K*i` ladders use 1000-step divisors (matching that app's labels).

interface FormatTier {
  divisor: number;
  label: string;
}

const DEFAULT_FIELD: ConfigFormatField = {
  precision: FormatPrecision.Two,
  unit: FormatUnit.KMGT,
};

function decimalPlaces(precision: FormatPrecision): number {
  switch (precision) {
    case FormatPrecision.Zero:
      return 0;
    case FormatPrecision.One:
      return 1;
    default:
      return 2;
  }
}

function tiersFor(unit: FormatUnit): FormatTier[] {
  switch (unit) {
    case FormatUnit.K:
      return [{ divisor: 1024, label: "K" }];
    case FormatUnit.KM:
      return [
        { divisor: 1024, label: "K" },
        { divisor: 1048576, label: "M" },
      ];
    case FormatUnit.KMG:
      return [
        { divisor: 1024, label: "K" },
        { divisor: 1048576, label: "M" },
        { divisor: 1073741824, label: "G" },
      ];
    case FormatUnit.KMi:
      return [
        { divisor: 1e3, label: "Ki" },
        { divisor: 1e6, label: "Mi" },
      ];
    case FormatUnit.KMiGi:
      return [
        { divisor: 1e3, label: "Ki" },
        { divisor: 1e6, label: "Mi" },
        { divisor: 1e9, label: "Gi" },
      ];
    case FormatUnit.KMiGiTi:
      return [
        { divisor: 1e3, label: "Ki" },
        { divisor: 1e6, label: "Mi" },
        { divisor: 1e9, label: "Gi" },
        { divisor: 1e12, label: "Ti" },
      ];
    default:
      return [
        { divisor: 1024, label: "K" },
        { divisor: 1048576, label: "M" },
        { divisor: 1073741824, label: "G" },
        { divisor: 1099511627776, label: "T" },
      ];
  }
}

function trimFractionZeros(value: string): string {
  if (value.lastIndexOf(".") > 0) {
    while (value.endsWith("0")) {
      value = value.slice(0, -1);
    }
  }
  if (value.endsWith(".")) {
    value = value.slice(0, -1);
  }
  return value;
}

function formatValue(
  value: number,
  field: ConfigFormatField,
  suffix: string,
): string {
  if (!Number.isFinite(value) || value < 0) {
    return "";
  }
  const decimals = decimalPlaces(field.precision);
  const tiers = tiersFor(field.unit);
  for (let i = tiers.length - 1; i >= 0; i -= 1) {
    if (value > tiers[i].divisor) {
      return `${trimFractionZeros(
        (value / tiers[i].divisor).toFixed(decimals),
      )}${tiers[i].label}${suffix}`;
    }
  }
  return `${value}${suffix}`;
}

/** Pick the stream format for a track type, falling back to defaults for
 *  chapters / attachments / unknown kinds. */
function fieldFor(
  formatting: ConfigFormatting | null | undefined,
  trackType: string,
  which: "size" | "bitRate",
): ConfigFormatField {
  if (!formatting) {
    return DEFAULT_FIELD;
  }
  const stream =
    trackType === "video"
      ? formatting.video
      : trackType === "audio"
        ? formatting.audio
        : trackType === "subtitles"
          ? formatting.subtitle
          : null;
  return stream ? stream[which] : DEFAULT_FIELD;
}

/** Format a byte count using the configured size precision/unit for the type. */
export function formatTrackSize(
  bytes: number,
  trackType: string,
  formatting: ConfigFormatting | null | undefined,
): string {
  return formatValue(bytes, fieldFor(formatting, trackType, "size"), "B");
}

/** Human-readable whole-file byte count (KMGT ladder, 2 decimals, e.g. "1.2GB"). */
export function formatFileSize(bytes: number): string {
  return formatValue(bytes, DEFAULT_FIELD, "B");
}

/** Format a bit rate (bps) using the configured precision/unit for the type. */
export function formatTrackBitRate(
  bps: number,
  trackType: string,
  formatting: ConfigFormatting | null | undefined,
): string {
  return formatValue(bps, fieldFor(formatting, trackType, "bitRate"), "bps");
}
