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

import type { MediaMetadataError } from "./protocol";

type TranslateFn = (
  key: string,
  options?: Record<string, string | number>,
) => string;

function isMediaMetadataError(value: unknown): value is MediaMetadataError {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    typeof (value as { kind: unknown }).kind === "string"
  );
}

/**
 * Map a `get_media_metadata` rejection to a human-readable string. The backend
 * categorises every failure into one of the [`MediaMetadataError`] tagged
 * variants; the i18n keys live under `merge.error.parser.*`. Unrecognised
 * values fall back to `String(err)` so debug output is never silently lost.
 */
export function formatMetadataError(err: unknown, t: TranslateFn): string {
  if (isMediaMetadataError(err)) {
    switch (err.kind) {
      case "io":
        return t("merge.error.parser.io", { detail: err.detail });
      case "unexpectedEof":
        return t("merge.error.parser.unexpectedEof", { detail: err.detail });
      case "unrecognised":
        return t("merge.error.parser.unrecognised", { detail: err.detail });
      case "timeout":
        return t("merge.error.parser.timeout", {
          budgetMs: err.budgetMs,
          stage: err.stage,
          detail: err.detail,
        });
      case "malformed":
        return t("merge.error.parser.malformed", { detail: err.detail });
      case "oversizedElement":
        return t("merge.error.parser.oversizedElement", {
          detail: err.detail,
        });
      case "internal":
        return t("merge.error.parser.internal", { detail: err.detail });
    }
  }
  return String(err);
}
