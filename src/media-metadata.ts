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

import type { MediaMetadata, Track, TrackFlag, TrackType } from "./protocol";

/**
 * Which kind of row a [`MediaTrack`] represents. The parser's `Track`s are
 * mapped to `"track"`; we also synthesise one `"chapters"` row when the file
 * has chapter editions, and one `"attachment"` row per `Attachment`.
 */
export type MediaTrackType = "track" | "chapters" | "attachment";

/**
 * UI-side flattened row. The wire format keeps `video / audio / subtitle`
 * sub-trees on each parsed [`Track`], plus separate `chapters` summary and
 * `attachments` arrays on [`MediaMetadata`]. The selection table needs one
 * row per parsed track + synthetic rows for chapters and attachments, so we
 * adapt them here. Field names mirror the v1 `MkvTrack` shape so call-sites
 * stay terse.
 */
export interface MediaTrack {
  kind: MediaTrackType;
  /** UI key — track.id for parsed tracks, attachment id, or 0 for chapters. */
  id: number;
  /** TrackNumber for parsed tracks; 0 for synthetic rows. */
  number: number;
  /** Mirrors mkvmerge -J `type` ("video"|"audio"|"subtitles"|"chapters"|"attachment"|"buttons"|"unknown"). */
  type: string;
  /** Human-readable codec name. For attachments this is the mime subtype. */
  codec: string;
  /** Raw container codec id ("V_MPEG4/ISO/AVC", FOURCC, ...). Drives the track extension lookup. */
  codecId: string;
  /** Optional friendly track name (TrackName). Empty for synthetic rows. */
  trackName: string;
  /** Resolved ISO-639-2 language ("eng", "und", ...). Empty for synthetic rows. */
  language: string;
  /** Every equivalent language code (terminologic + bibliographic + alpha-2),
   *  lowercased, used for filter matching so "fre"/"fra"/"fr" all match. Empty
   *  for synthetic rows. */
  languageCodes: string[];
  /** Raw `FlagDefault` tri-state ("true" | "false" | "unspecified"), shown as a
   *  3-state control. "unspecified" for synthetic rows. */
  defaultTrack: TrackFlag;
  /** Raw `FlagForced` tri-state, shown as a 3-state control. "unspecified" for
   *  synthetic rows. */
  forced: TrackFlag;
}

/**
 * Map a parser [`TrackType`] onto the legacy mkvmerge -J string the rest of
 * the UI switches on. The parser distinguishes "subtitles" / "buttons" /
 * "unknown" with camelCase enum values; we map them onto lowercase tokens.
 */
function trackTypeToUiType(t: TrackType): string {
  switch (t) {
    case "video":
      return "video";
    case "audio":
      return "audio";
    case "subtitles":
      return "subtitles";
    case "buttons":
      return "buttons";
    default:
      return "unknown";
  }
}

/**
 * The language code shown in the track table's Language column. Prefers the
 * bibliographic ISO 639-2/B form (fre/ger/chi) so the column reads in the same
 * convention as the language filter list; falls back to the terminologic code
 * (eng/spa/jpn have no B/T split) or "und". Filter *matching* uses
 * `trackLanguageCodes` below, which carries every equivalent form.
 */
function pickTrackLanguage(track: Track): string {
  const lang = track.properties.common.language ?? null;
  if (!lang) {
    return "und";
  }
  return lang.iso639_2Bib || lang.iso639_2 || "und";
}

/**
 * Every equivalent ISO code the backend resolved for a track — terminologic
 * (`fra`), bibliographic (`fre`) and alpha-2 (`fr`) — lowercased and deduped.
 * Language filters match if any of these equal a configured code, so a list
 * written in bibliographic form ("fre", "ger", "chi") still selects tracks
 * the parser canonicalised to terminologic form ("fra", "deu", "zho").
 */
function trackLanguageCodes(track: Track): string[] {
  const lang = track.properties.common.language ?? null;
  if (!lang) {
    return ["und"];
  }
  const codes = [lang.iso639_2, lang.iso639_2Bib, lang.iso639_1]
    .filter((c): c is string => !!c && c.length > 0)
    .map((c) => c.toLowerCase());
  return codes.length > 0 ? Array.from(new Set(codes)) : ["und"];
}

/**
 * Derive the file extension we used to display under "codec" for an
 * attachment. The old `MkvTrack.codec` for attachments was a normalised mime
 * subtype ("jpeg", "x-truetype-font", ...). We reproduce that derivation.
 */
function attachmentSubtype(fileName: string, mimeType: string | null): string {
  const dot = fileName.lastIndexOf(".");
  if (dot >= 0 && dot < fileName.length - 1) {
    return fileName.slice(dot + 1).toLowerCase();
  }
  if (mimeType) {
    const slash = mimeType.indexOf("/");
    if (slash >= 0 && slash < mimeType.length - 1) {
      return mimeType.slice(slash + 1).toLowerCase();
    }
  }
  return "";
}

/**
 * Flatten a parsed [`MediaMetadata`] into the synthetic row list the
 * selection table renders. Synthetic chapter and attachment rows are appended
 * in the same order mkvmerge -J emitted them under the old wire format.
 */
export function metadataToMediaTracks(meta: MediaMetadata): MediaTrack[] {
  const rows: MediaTrack[] = [];
  for (const track of meta.tracks) {
    rows.push({
      kind: "track",
      id: Number(track.id),
      number: Number(track.properties.common.number ?? 0),
      type: trackTypeToUiType(track.trackType),
      codec: track.codec.name ?? track.codec.id,
      codecId: track.codec.id,
      trackName: track.properties.common.trackName ?? "",
      language: pickTrackLanguage(track),
      languageCodes: trackLanguageCodes(track),
      defaultTrack: track.properties.common.default,
      forced: track.properties.common.forced,
    });
  }
  if (meta.chapters && meta.chapters.numEntries > 0) {
    rows.push({
      kind: "chapters",
      id: 0,
      number: 0,
      type: "chapters",
      codec: "xml",
      codecId: "xml",
      trackName: "",
      language: "",
      languageCodes: [],
      defaultTrack: "unspecified",
      forced: "unspecified",
    });
  }
  for (const attachment of meta.attachments) {
    const subtype = attachmentSubtype(attachment.fileName, attachment.mimeType);
    rows.push({
      kind: "attachment",
      id: Number(attachment.id),
      number: 0,
      type: "attachment",
      codec: subtype,
      codecId: subtype,
      trackName: attachment.fileName,
      language: "",
      languageCodes: [],
      defaultTrack: "unspecified",
      forced: "unspecified",
    });
  }
  return rows;
}

/**
 * Coarse track-kind histogram used to decide whether a set of files can be
 * grouped in the file list. Derived from the same flattened tracks we feed
 * to the selection table so the counts stay aligned with what users see.
 */
export interface MediaTrackCounts {
  video: number;
  audio: number;
  subtitles: number;
  chapters: number;
  attachments: number;
}

export function mediaTrackCounts(tracks: MediaTrack[]): MediaTrackCounts {
  let video = 0;
  let audio = 0;
  let subtitles = 0;
  let chapters = 0;
  let attachments = 0;
  for (const row of tracks) {
    if (row.type === "video") {
      video += 1;
    } else if (row.type === "audio") {
      audio += 1;
    } else if (row.type === "subtitles") {
      subtitles += 1;
    } else if (row.type === "chapters") {
      chapters += 1;
    } else if (row.type === "attachment") {
      attachments += 1;
    }
  }
  return { video, audio, subtitles, chapters, attachments };
}
