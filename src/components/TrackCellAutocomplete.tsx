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

import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent,
} from "react";
import { Autocomplete, Box, TextField } from "@mui/material";
import { MKV_LANGUAGES } from "../mkvLanguages";
import type { ConfigProfile } from "../protocol";

const OPTION_ROW_HEIGHT = 34;
const OPTION_VISIBLE_ROWS = 10;
const DROPDOWN_MAX_HEIGHT = OPTION_ROW_HEIGHT * OPTION_VISIBLE_ROWS;

/** Short code for a language: the ISO 639-1 alpha-2 from the label's `(xx; yyy)`
 *  suffix when present, else the (3-letter) code. Mirrors BetterMediaInfo. */
function shortLanguageCodeFor(code: string, label: string): string {
  const match = label.match(/\(([a-z]{2});\s*[a-z]{3}\)$/i);
  return match?.[1] ?? code;
}

// Built once from the static language table. Canonical value = the short code
// (alpha-2 when available, else 3-letter) so the cell, options and merge
// command all use `en` rather than `eng`. The lookups normalise any typed form
// (3-letter code, alpha-2, English name, full label) to the short code, and
// back to the 3-letter MKV code used as the settings key.
const LABEL_BY_CODE = new Map<string, string>();
const SHORT_CODE_BY_ANY = new Map<string, string>();
const MKV_CODE_BY_ANY = new Map<string, string>();
const ALL_SHORT_CODES: string[] = [];
const seenShortCodes = new Set<string>();
for (const { code, label } of MKV_LANGUAGES) {
  const shortCode = shortLanguageCodeFor(code, label);
  const displayName = label.replace(/\s*\([^)]*\)\s*$/, "").trim();
  const twoLetter = label.match(/\(([a-z]{2});\s*[a-z]{3}\)/i)?.[1] ?? "";
  const aliases = [code, shortCode, twoLetter, displayName, label].filter(
    Boolean,
  );
  for (const alias of aliases) {
    const key = alias.toLowerCase();
    if (!SHORT_CODE_BY_ANY.has(key)) {
      SHORT_CODE_BY_ANY.set(key, shortCode);
    }
    if (!MKV_CODE_BY_ANY.has(key)) {
      MKV_CODE_BY_ANY.set(key, code);
    }
  }
  if (!LABEL_BY_CODE.has(shortCode)) {
    LABEL_BY_CODE.set(shortCode, label);
  }
  LABEL_BY_CODE.set(code, label);
  if (!seenShortCodes.has(shortCode)) {
    seenShortCodes.add(shortCode);
    ALL_SHORT_CODES.push(shortCode);
  }
}

/** Normalise any language form to the canonical short code. */
function toShortCode(value: string): string {
  return SHORT_CODE_BY_ANY.get(value.trim().toLowerCase()) ?? value.trim();
}

/** Map any language form to the 3-letter MKV code used as the settings key. */
function toMkvCode(value: string): string {
  return MKV_CODE_BY_ANY.get(value.trim().toLowerCase()) ?? value.trim();
}

export function languageLabel(code: string): string {
  return LABEL_BY_CODE.get(code) ?? code;
}

function normalizeLanguageValue(value: string): string {
  return toShortCode(value);
}

function firstMatchingLanguageOptionIndex(
  options: string[],
  inputValue: string,
): number {
  const query = inputValue.trim().toLowerCase();
  if (query.length === 0) {
    return -1;
  }
  const exact = SHORT_CODE_BY_ANY.get(query);
  if (exact) {
    const exactIndex = options.indexOf(exact);
    if (exactIndex >= 0) {
      return exactIndex;
    }
  }
  return options.findIndex((option) => {
    const code = option.toLowerCase();
    const label = (LABEL_BY_CODE.get(option) ?? "").toLowerCase();
    return code.startsWith(query) || label.includes(query);
  });
}

function firstMatchingTitleOptionIndex(
  options: string[],
  inputValue: string,
): number {
  const query = inputValue.trim().toLowerCase();
  if (query.length === 0) {
    return -1;
  }
  const startsWith = options.findIndex((option) =>
    option.toLowerCase().startsWith(query),
  );
  if (startsWith >= 0) {
    return startsWith;
  }
  return options.findIndex((option) => option.toLowerCase().includes(query));
}

const LANGUAGE_PREF_KEY: Record<
  string,
  "preferredVideoLanguages" | "preferredAudioLanguages" | "preferredSubtitleLanguages"
> = {
  video: "preferredVideoLanguages",
  audio: "preferredAudioLanguages",
  subtitles: "preferredSubtitleLanguages",
};

const TRACK_NAMES_KEY: Record<
  string,
  "trackNamesVideo" | "trackNamesAudio" | "trackNamesSubtitle"
> = {
  video: "trackNamesVideo",
  audio: "trackNamesAudio",
  subtitles: "trackNamesSubtitle",
};

/** Language options for a track type: the profile's preferred languages first
 *  (their count returned separately so the dropdown can draw a divider), then
 *  every remaining language. */
export function buildLanguageOptions(
  profile: ConfigProfile | null,
  trackType: string,
): { options: string[]; preferredCount: number } {
  const key = LANGUAGE_PREF_KEY[trackType];
  const preferredRaw =
    profile && key
      ? profile[key]
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
      : [];
  const seen = new Set<string>();
  const preferred: string[] = [];
  for (const raw of preferredRaw) {
    const short = toShortCode(raw);
    if (!seen.has(short)) {
      seen.add(short);
      preferred.push(short);
    }
  }
  const rest = ALL_SHORT_CODES.filter((code) => !seen.has(code));
  return { options: [...preferred, ...rest], preferredCount: preferred.length };
}

/** Language options for a profile-wide picker (not tied to a track type): the
 *  union of all three preferred lists first, then every remaining language. */
export function buildCombinedLanguageOptions(
  profile: ConfigProfile | null,
): { options: string[]; preferredCount: number } {
  const raw = profile
    ? [
        profile.preferredVideoLanguages,
        profile.preferredAudioLanguages,
        profile.preferredSubtitleLanguages,
      ].join(",")
    : "";
  const seen = new Set<string>();
  const preferred: string[] = [];
  for (const part of raw.split(",").map((s) => s.trim()).filter(Boolean)) {
    const short = toShortCode(part);
    if (!seen.has(short)) {
      seen.add(short);
      preferred.push(short);
    }
  }
  const rest = ALL_SHORT_CODES.filter((code) => !seen.has(code));
  return { options: [...preferred, ...rest], preferredCount: preferred.length };
}

/** Preferred track-name presets for a track type + language (one per line in
 *  the settings store). */
export function buildTrackNameOptions(
  profile: ConfigProfile | null,
  trackType: string,
  language: string,
): string[] {
  const key = TRACK_NAMES_KEY[trackType];
  if (!profile || !key) {
    return [];
  }
  // Track-name presets are keyed by the 3-letter MKV code in settings, while
  // the track's language is the short code — map it back to look them up.
  const raw = profile[key][toMkvCode(language)] ?? "";
  const seen = new Set<string>();
  return raw
    .split("\n")
    .map((s) => s.trim())
    .filter((s) => {
      if (s.length === 0 || seen.has(s.toLowerCase())) {
        return false;
      }
      seen.add(s.toLowerCase());
      return true;
    });
}

const popperSlotSx = {
  width: "max-content !important",
  minWidth: 280,
  maxWidth: "calc(100vw - 32px)",
  "& .MuiAutocomplete-paper": {
    width: "max-content",
    minWidth: 280,
    maxWidth: "calc(100vw - 32px)",
  },
  "& .MuiAutocomplete-listbox": {
    p: 0,
    boxSizing: "border-box",
    maxHeight: `min(${DROPDOWN_MAX_HEIGHT}px, calc(100vh - 16px))`,
  },
  "& .MuiAutocomplete-option": {
    boxSizing: "border-box",
    height: OPTION_ROW_HEIGHT,
    minHeight: OPTION_ROW_HEIGHT,
    py: 0.5,
    whiteSpace: "nowrap",
  },
} as const;

/** Shared placement + auto-scroll behaviour for both cell autocompletes. */
function useDropdownBehaviour(matchingOptionIndex: number, optionCount: number) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const listboxRef = useRef<HTMLUListElement | null>(null);
  const [placement, setPlacement] = useState<"bottom-start" | "top-start">(
    "bottom-start",
  );
  const [openVersion, setOpenVersion] = useState(0);

  const updatePlacement = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) {
      return;
    }
    const visibleRows = Math.min(optionCount, OPTION_VISIBLE_ROWS);
    const dropdownHeight = visibleRows * OPTION_ROW_HEIGHT;
    const below = window.innerHeight - rect.bottom - 8;
    const above = rect.top - 8;
    setPlacement(below >= dropdownHeight || below >= above ? "bottom-start" : "top-start");
  }, [optionCount]);

  const handleOpen = useCallback(() => {
    updatePlacement();
    setOpenVersion((v) => v + 1);
  }, [updatePlacement]);

  const scrollToMatch = useCallback(() => {
    if (matchingOptionIndex < 0 || !listboxRef.current) {
      return;
    }
    const visibleRows = Math.min(optionCount, OPTION_VISIBLE_ROWS);
    const firstVisible = Math.max(
      0,
      matchingOptionIndex - Math.floor(visibleRows / 2),
    );
    listboxRef.current.scrollTop = firstVisible * OPTION_ROW_HEIGHT;
  }, [matchingOptionIndex, optionCount]);

  useLayoutEffect(() => {
    if (matchingOptionIndex < 0) {
      return;
    }
    scrollToMatch();
    let follow = 0;
    const frame = requestAnimationFrame(() => {
      scrollToMatch();
      follow = requestAnimationFrame(scrollToMatch);
    });
    return () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(follow);
    };
  }, [openVersion, matchingOptionIndex, scrollToMatch]);

  return { rootRef, listboxRef, placement, handleOpen, updatePlacement };
}

export function LanguageAutocomplete({
  value,
  options,
  preferredOptionCount,
  disabled,
  onChange,
}: {
  value: string;
  options: string[];
  preferredOptionCount: number;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  const matchingOptionIndex = useMemo(
    () => firstMatchingLanguageOptionIndex(options, value),
    [options, value],
  );
  const { rootRef, listboxRef, placement, handleOpen, updatePlacement } =
    useDropdownBehaviour(matchingOptionIndex, options.length);

  const commitValue = useCallback(
    (next: string) => onChange(normalizeLanguageValue(next)),
    [onChange],
  );

  return (
    <Autocomplete<string, false, false, true>
      ref={rootRef}
      freeSolo
      fullWidth
      size="small"
      disabled={disabled}
      options={options}
      filterOptions={(allOptions) => allOptions}
      value={value}
      inputValue={value}
      getOptionLabel={(option) => languageLabel(option)}
      renderOption={(props, option, state) => {
        const { key, ...optionProps } = props;
        const isFirstRest =
          state.index === preferredOptionCount &&
          preferredOptionCount > 0 &&
          preferredOptionCount < options.length;
        const isMatched = state.index === matchingOptionIndex;
        return (
          <Box
            key={key}
            component="li"
            {...optionProps}
            sx={{
              borderTop: isFirstRest ? 1 : 0,
              borderColor: "divider",
              bgcolor: isMatched ? "action.selected" : undefined,
              "&.Mui-focused": {
                bgcolor: isMatched ? "action.selected" : "action.hover",
              },
            }}
          >
            {languageLabel(option)}
          </Box>
        );
      }}
      onChange={(_e, next) => commitValue(next ?? "")}
      onInputChange={(_e, next, reason) => {
        if (reason === "input" || reason === "clear") {
          onChange(next);
        }
      }}
      onOpen={handleOpen}
      slotProps={{
        popper: {
          placement,
          modifiers: [
            { name: "flip", enabled: false },
            {
              name: "preventOverflow",
              enabled: true,
              options: { mainAxis: true, altAxis: false, padding: 8 },
            },
            { name: "offset", options: { offset: [0, 0] } },
          ],
          sx: popperSlotSx,
        },
        listbox: { ref: listboxRef },
      }}
      renderInput={(params) => (
        <TextField
          {...params}
          variant="standard"
          fullWidth
          slotProps={{
            ...params.slotProps,
            htmlInput: {
              ...params.slotProps.htmlInput,
              onFocus: (event: FocusEvent<HTMLInputElement>) => {
                params.slotProps.htmlInput?.onFocus?.(event);
                updatePlacement();
                event.currentTarget.select();
              },
              onClick: (event: MouseEvent<HTMLInputElement>) => {
                params.slotProps.htmlInput?.onClick?.(event);
                updatePlacement();
                event.currentTarget.select();
              },
              onKeyDown: (event: ReactKeyboardEvent<HTMLInputElement>) => {
                if (event.key === "Enter") {
                  const normalized = normalizeLanguageValue(
                    event.currentTarget.value,
                  );
                  if (
                    normalized !== event.currentTarget.value &&
                    options.includes(normalized)
                  ) {
                    event.preventDefault();
                    event.stopPropagation();
                    commitValue(event.currentTarget.value);
                    return;
                  }
                }
                params.slotProps.htmlInput?.onKeyDown?.(event);
              },
            },
          }}
        />
      )}
    />
  );
}

export function TitleAutocomplete({
  value,
  options,
  disabled,
  onChange,
}: {
  value: string;
  options: string[];
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  const matchingOptionIndex = useMemo(
    () => firstMatchingTitleOptionIndex(options, value),
    [options, value],
  );
  const { rootRef, listboxRef, placement, handleOpen, updatePlacement } =
    useDropdownBehaviour(matchingOptionIndex, options.length);

  return (
    <Autocomplete<string, false, false, true>
      ref={rootRef}
      freeSolo
      fullWidth
      size="small"
      disabled={disabled}
      options={options}
      filterOptions={(allOptions) => allOptions}
      value={value}
      inputValue={value}
      renderOption={(props, option, state) => {
        const { key, ...optionProps } = props;
        const isMatched = state.index === matchingOptionIndex;
        return (
          <Box
            key={key}
            component="li"
            {...optionProps}
            sx={{
              bgcolor: isMatched ? "action.selected" : undefined,
              "&.Mui-focused": {
                bgcolor: isMatched ? "action.selected" : "action.hover",
              },
            }}
          >
            {option}
          </Box>
        );
      }}
      onChange={(_e, next) => onChange(next ?? "")}
      onInputChange={(_e, next, reason) => {
        if (reason === "input" || reason === "clear") {
          onChange(next);
        }
      }}
      onOpen={handleOpen}
      slotProps={{
        popper: {
          placement,
          modifiers: [
            { name: "flip", enabled: false },
            {
              name: "preventOverflow",
              enabled: true,
              options: { mainAxis: true, altAxis: false, padding: 8 },
            },
            { name: "offset", options: { offset: [0, 0] } },
          ],
          sx: popperSlotSx,
        },
        listbox: { ref: listboxRef },
      }}
      renderInput={(params) => (
        <TextField
          {...params}
          variant="standard"
          fullWidth
          slotProps={{
            ...params.slotProps,
            htmlInput: {
              ...params.slotProps.htmlInput,
              onFocus: (event: FocusEvent<HTMLInputElement>) => {
                params.slotProps.htmlInput?.onFocus?.(event);
                updatePlacement();
                event.currentTarget.select();
              },
              onClick: (event: MouseEvent<HTMLInputElement>) => {
                params.slotProps.htmlInput?.onClick?.(event);
                updatePlacement();
                event.currentTarget.select();
              },
            },
          }}
        />
      )}
    />
  );
}
