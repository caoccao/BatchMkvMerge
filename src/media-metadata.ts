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
  AudioTrackProperties,
  MediaMetadata,
  SubtitleTrackProperties,
  Track,
  TrackFlag,
  TrackType,
  VideoTrackProperties,
} from "./protocol";

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
  /** Short human-readable summary of the track's domain properties: resolution
   *  + frame rate (video), channel layout (audio), text/image + variant +
   *  encoding (subtitles). Empty for synthetic rows. */
  description: string;
  /** Payload size in bytes when the source exposes it (Matroska
   *  `NUMBER_OF_BYTES` statistics tag, or an attachment's stored size). Null
   *  when the parser has no value. */
  size: number | null;
  /** Bit rate in bits per second when available (Matroska `BPS` statistics
   *  tag). Null when the parser has no value. */
  bitRate: number | null;
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
 * Canonical sort rank for the UI `type` string, used wherever tracks are shown
 * or ordered: video, audio, subtitle, "menu"/buttons, then the synthetic
 * chapters and attachment rows, then anything unknown. Shared by the combined
 * merge-tree table (`file-tree.ts`) and the multi-input merge command
 * (`merge.ts`) so both stay in lock-step.
 */
export const TRACK_TYPE_ORDER: Record<string, number> = {
  video: 0,
  audio: 1,
  subtitles: 2,
  buttons: 3,
  chapters: 4,
  attachment: 5,
  unknown: 6,
};

export function trackTypeRank(type: string): number {
  return TRACK_TYPE_ORDER[type] ?? TRACK_TYPE_ORDER.unknown;
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
 * The language code shown in the track table's Language column and emitted in
 * the merge command. Prefers the ISO 639-1 alpha-2 form (`en`, `fr`, `zh`) to
 * match the editable dropdown / mkvmerge convention, falling back to the
 * bibliographic then terminologic ISO 639-2 code when no alpha-2 exists, else
 * "und". Filter *matching* uses `trackLanguageCodes` below, which carries every
 * equivalent form, so auto-select against the 3-letter settings lists still
 * works.
 */
function pickTrackLanguage(track: Track): string {
  const lang = track.properties.common.language ?? null;
  if (!lang) {
    return "und";
  }
  return lang.iso639_1 || lang.iso639_2Bib || lang.iso639_2 || "und";
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
 * Read a numeric Matroska statistics tag (e.g. `BPS`, `NUMBER_OF_BYTES`) off a
 * track. mkvmerge writes these per-track; many files lack them, so callers get
 * `null` when the tag is absent or non-numeric. Matched case-insensitively.
 */
function statTagNumber(track: Track, name: string): number | null {
  const upper = name.toUpperCase();
  const tag = track.properties.tags.find((t) => t.name.toUpperCase() === upper);
  if (!tag) {
    return null;
  }
  const value = Number(tag.value);
  return Number.isFinite(value) ? value : null;
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

/** Human label for a canonical channel-layout kind (`layout51` → "5.1"). */
const CHANNEL_LAYOUT_LABELS: Record<string, string> = {
  mono: "mono",
  stereo: "stereo",
  layout21: "2.1",
  layout30: "3.0",
  layout31: "3.1",
  layout40: "4.0",
  layout41: "4.1",
  layout50: "5.0",
  layout51: "5.1",
  layout61: "6.1",
  layout71: "7.1",
  layout714: "7.1.4",
};

/** Frame rate from a per-frame duration in ns, e.g. 41708333 → "23.976fps". */
function formatFrameRate(defaultDurationNs: number | null): string | null {
  if (!defaultDurationNs || defaultDurationNs <= 0) {
    return null;
  }
  const fps = 1e9 / defaultDurationNs;
  return `${Math.round(fps * 1000) / 1000}fps`;
}

function videoDescription(video: VideoTrackProperties): string {
  const parts: string[] = [];
  const dim = video.pixelDimensions ?? video.displayDimensions;
  if (dim) {
    parts.push(`${dim.width}x${dim.height}`);
  }
  const fps = formatFrameRate(video.defaultDurationNs);
  if (fps) {
    parts.push(fps);
  }
  return parts.join(" ");
}

/** Sample rate in Hz formatted as kHz, e.g. 48000 → "48kHz", 44100 → "44.1kHz". */
function formatSampleRate(hz: number | null): string | null {
  if (!hz || hz <= 0) {
    return null;
  }
  return `${Math.round((hz / 1000) * 1000) / 1000}kHz`;
}

function channelLabel(audio: AudioTrackProperties): string | null {
  const kind = audio.channelLayout?.kind ?? null;
  if (kind && CHANNEL_LAYOUT_LABELS[kind]) {
    return CHANNEL_LAYOUT_LABELS[kind];
  }
  const channels = audio.channelLayout?.channels ?? audio.channels ?? null;
  if (channels === 1) {
    return "mono";
  }
  if (channels === 2) {
    return "stereo";
  }
  return channels != null ? `${channels}ch` : null;
}

function audioDescription(audio: AudioTrackProperties): string {
  const parts: string[] = [];
  const channels = channelLabel(audio);
  if (channels) {
    parts.push(channels);
  }
  const sampleRate = formatSampleRate(audio.samplingFrequency);
  if (sampleRate) {
    parts.push(sampleRate);
  }
  return parts.join(" ");
}

function subtitleDescription(subtitle: SubtitleTrackProperties): string {
  const parts: string[] = [subtitle.textSubtitles ? "Text" : "Image"];
  if (subtitle.variant) {
    parts.push(subtitle.variant);
  }
  if (subtitle.encoding) {
    parts.push(subtitle.encoding);
  }
  if (subtitle.teletextPage != null) {
    parts.push(`p${subtitle.teletextPage}`);
  }
  return parts.join(" ");
}

/** Short per-track summary for the table's Description column, derived from the
 *  populated domain sub-tree (video / audio / subtitle). */
function trackDescription(track: Track): string {
  const props = track.properties;
  if (props.video) {
    return videoDescription(props.video);
  }
  if (props.audio) {
    return audioDescription(props.audio);
  }
  if (props.subtitle) {
    return subtitleDescription(props.subtitle);
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
      description: trackDescription(track),
      size: statTagNumber(track, "NUMBER_OF_BYTES"),
      bitRate: statTagNumber(track, "BPS"),
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
      description: "",
      size: null,
      bitRate: null,
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
      description: "",
      size: Number.isFinite(attachment.size) ? attachment.size : null,
      bitRate: null,
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
