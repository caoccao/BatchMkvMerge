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

import { useCallback, useRef, useState } from "react";
import {
  Autocomplete,
  Box,
  Button,
  Checkbox,
  Divider,
  FormControl,
  FormControlLabel,
  IconButton,
  InputAdornment,
  MenuItem,
  Paper,
  Radio,
  RadioGroup,
  Select,
  Stack,
  Tab,
  Tabs,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
} from "@mui/material";
import BrightnessAutoIcon from "@mui/icons-material/BrightnessAuto";
import ClearIcon from "@mui/icons-material/Clear";
import DarkModeIcon from "@mui/icons-material/DarkMode";
import ExtensionIcon from "@mui/icons-material/Extension";
import InfoIcon from "@mui/icons-material/Info";
import PermMediaIcon from "@mui/icons-material/PermMedia";
import LightModeIcon from "@mui/icons-material/LightMode";
import MovieIcon from "@mui/icons-material/Movie";
import PaletteIcon from "@mui/icons-material/Palette";
import PersonIcon from "@mui/icons-material/Person";
import TuneIcon from "@mui/icons-material/Tune";
import UpdateIcon from "@mui/icons-material/Update";
import { open } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import * as Protocol from "../protocol";
import { MKV_LANGUAGES } from "../mkvLanguages";
import { detectBetterMediaInfo, isMkvtoolnixFound } from "../service";
import { useMkvStore } from "../store";
import {
  buildCombinedLanguageOptions,
  languageLabel as fullLanguageLabel,
} from "./TrackCellAutocomplete";
import { TrackTypeIcon } from "./TrackTypeIcon";
import { ExternalToolPathRow } from "./settings/ExternalToolPathRow";
import { LanguagePicker } from "./settings/LanguagePicker";
import {
  DetectToolPath,
  useToolPathDetection,
} from "./settings/useToolPathDetection";

enum SettingsTab {
  Appearance = "Appearance",
  Formatting = "Formatting",
  Group = "Group",
  Profiles = "Profiles",
  Integration = "Integration",
  Update = "Update",
}

type FormatStreamKind = "video" | "audio" | "subtitle";

enum ProfileTab {
  TrackSelection = "TrackSelection",
  Languages = "Languages",
  TrackNames = "TrackNames",
  Automation = "Automation",
}

type LanguageTrackType = "video" | "audio" | "subtitles";

/**
 * Profile field backing tab 2's preferred-language picker. These are separate
 * from the tab-1 track-selection language filters — the two tabs don't share
 * data.
 */
const PREFERRED_LANGUAGES_KEY: Record<
  LanguageTrackType,
  | "preferredVideoLanguages"
  | "preferredAudioLanguages"
  | "preferredSubtitleLanguages"
> = {
  video: "preferredVideoLanguages",
  audio: "preferredAudioLanguages",
  subtitles: "preferredSubtitleLanguages",
};

/** Profile field backing tab 3's per-language track-name presets. */
const TRACK_NAMES_KEY: Record<
  LanguageTrackType,
  "trackNamesVideo" | "trackNamesAudio" | "trackNamesSubtitle"
> = {
  video: "trackNamesVideo",
  audio: "trackNamesAudio",
  subtitles: "trackNamesSubtitle",
};

const LANGUAGE_LABEL_BY_CODE = new Map(
  MKV_LANGUAGES.map((lang) => [lang.code, lang.label]),
);
const ALL_LANGUAGE_CODES = MKV_LANGUAGES.map((lang) => lang.code);

function languageLabel(code: string): string {
  return LANGUAGE_LABEL_BY_CODE.get(code) ?? code;
}

/** Parse a comma-separated language filter into an ordered code list. */
function parseLanguageCodes(filter: string): string[] {
  return filter
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

/** Trailing clear (×) button for a text field — shown only when non-empty. */
function ClearAdornment({
  value,
  onClear,
  label,
}: {
  value: string;
  onClear: () => void;
  label: string;
}) {
  if (!value) {
    return null;
  }
  return (
    <InputAdornment position="end">
      <Tooltip title={label}>
        <IconButton size="small" aria-label={label} onClick={onClear} edge="end">
          <ClearIcon fontSize="small" />
        </IconButton>
      </Tooltip>
    </InputAdornment>
  );
}

/** Video / Audio / Subtitle selector shared by the Languages and Track Names tabs. */
function TrackTypeToggle({
  value,
  onChange,
}: {
  value: LanguageTrackType;
  onChange: (next: LanguageTrackType) => void;
}) {
  const { t } = useTranslation();
  return (
    <ToggleButtonGroup
      value={value}
      exclusive
      size="small"
      onChange={(_e, next: LanguageTrackType | null) => {
        if (next !== null) {
          onChange(next);
        }
      }}
      sx={{ mb: 1.5, "& .MuiToggleButton-root": { textTransform: "none" } }}
    >
      <ToggleButton value="video" sx={{ px: 1.5, gap: 0.5 }}>
        <TrackTypeIcon type="video" />
        <Typography variant="caption">{t("settings.video")}</Typography>
      </ToggleButton>
      <ToggleButton value="audio" sx={{ px: 1.5, gap: 0.5 }}>
        <TrackTypeIcon type="audio" />
        <Typography variant="caption">{t("settings.audio")}</Typography>
      </ToggleButton>
      <ToggleButton value="subtitles" sx={{ px: 1.5, gap: 0.5 }}>
        <TrackTypeIcon type="subtitles" />
        <Typography variant="caption">{t("settings.subtitles")}</Typography>
      </ToggleButton>
    </ToggleButtonGroup>
  );
}

/** Precision + unit selects for one formatted field (bit rate or size). */
function FormatFieldControls({
  label,
  value,
  onChange,
}: {
  label: string;
  value: Protocol.ConfigFormatField;
  onChange: (next: Protocol.ConfigFormatField) => void;
}) {
  const { t } = useTranslation();
  return (
    <Box>
      <Typography variant="body2" sx={{ fontWeight: 500, mb: 1 }}>
        {label}
      </Typography>
      <Box sx={{ display: "flex", gap: 2 }}>
        <Box sx={{ flex: 1 }}>
          <Typography variant="caption" color="text.secondary">
            {t("settings.precision")}
          </Typography>
          <FormControl size="small" fullWidth sx={{ mt: 0.5 }}>
            <Select
              value={value.precision}
              onChange={(e) =>
                onChange({
                  ...value,
                  precision: e.target.value as Protocol.FormatPrecision,
                })
              }
            >
              {Protocol.getFormatPrecisions().map((p) => (
                <MenuItem key={p} value={p}>
                  {Protocol.getFormatPrecisionLabel(p)}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
        </Box>
        <Box sx={{ flex: 1 }}>
          <Typography variant="caption" color="text.secondary">
            {t("settings.unit")}
          </Typography>
          <FormControl size="small" fullWidth sx={{ mt: 0.5 }}>
            <Select
              value={value.unit}
              onChange={(e) =>
                onChange({
                  ...value,
                  unit: e.target.value as Protocol.FormatUnit,
                })
              }
            >
              {Protocol.getFormatUnits().map((u) => (
                <MenuItem key={u} value={u}>
                  {Protocol.getFormatUnitLabel(u)}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
        </Box>
      </Box>
    </Box>
  );
}

function SectionHeader({
  icon,
  title,
}: {
  icon: React.ReactNode;
  title: string;
}) {
  return (
    <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 2 }}>
      <Box sx={{ color: "primary.main", display: "flex" }}>{icon}</Box>
      <Typography variant="subtitle1" sx={{ fontWeight: 600 }}>
        {title}
      </Typography>
    </Box>
  );
}

function SettingRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        py: 1,
        "&:not(:last-child)": { borderBottom: 1, borderColor: "divider" },
      }}
    >
      <Typography variant="body2" color="text.secondary">
        {label}
      </Typography>
      <Box>{children}</Box>
    </Box>
  );
}

export default function Settings() {
  const { t } = useTranslation();
  const config = useMkvStore((s) => s.config);
  const updateConfig = useMkvStore((s) => s.updateConfig);
  const updateActiveProfile = useMkvStore((s) => s.updateActiveProfile);
  const addProfile = useMkvStore((s) => s.addProfile);
  const deleteActiveProfile = useMkvStore((s) => s.deleteActiveProfile);
  const setActiveProfile = useMkvStore((s) => s.setActiveProfile);
  const resetActiveProfileTemplates = useMkvStore(
    (s) => s.resetActiveProfileTemplates,
  );
  const setBetterMediaInfoAvailable = useMkvStore(
    (s) => s.setBetterMediaInfoAvailable,
  );
  const [newProfileName, setNewProfileName] = useState("");
  const [tab, setTab] = useState<SettingsTab>(SettingsTab.Appearance);
  const [profileTab, setProfileTab] = useState<ProfileTab>(
    ProfileTab.TrackSelection,
  );
  const [langType, setLangType] = useState<LanguageTrackType>("audio");
  const [formatStreamKind, setFormatStreamKind] =
    useState<FormatStreamKind>("video");
  const [trackNameFilter, setTrackNameFilter] = useState("");
  const [selectedTrackNameLang, setSelectedTrackNameLang] = useState("");
  const trackNameListRef = useRef<HTMLDivElement | null>(null);

  const updateExternalTools = useCallback(
    (patch: Partial<Protocol.ConfigExternalTools>) => {
      if (!config) {
        return;
      }
      updateConfig({
        externalTools: {
          mkvToolNixPath:
            patch.mkvToolNixPath ?? config.externalTools?.mkvToolNixPath ?? "",
          betterMediaInfoPath:
            patch.betterMediaInfoPath ??
            config.externalTools?.betterMediaInfoPath ??
            "",
        },
      });
    },
    [config, updateConfig],
  );

  const persistMkvToolNixPath = useCallback(
    (value: string) => {
      if (!config) {
        return;
      }
      if (value === (config.externalTools?.mkvToolNixPath ?? "")) {
        return;
      }
      updateExternalTools({ mkvToolNixPath: value });
    },
    [config, updateExternalTools],
  );

  const persistBetterMediaInfoPath = useCallback(
    (value: string) => {
      if (!config) {
        return;
      }
      if (value === (config.externalTools?.betterMediaInfoPath ?? "")) {
        return;
      }
      updateExternalTools({ betterMediaInfoPath: value });
    },
    [config, updateExternalTools],
  );

  const detectMkvToolNixPath = useCallback<DetectToolPath>(
    async (path, checkRunning = false) => {
      const status = await isMkvtoolnixFound(path, checkRunning);
      return {
        found: status.found,
        path: status.mkvToolNixPath,
      };
    },
    [],
  );

  const detectBetterMediaInfoPath = useCallback<DetectToolPath>(
    async (path, checkRunning = false) => {
      const status = await detectBetterMediaInfo(path, checkRunning);
      return {
        found: status.found,
        path: status.path,
      };
    },
    [],
  );

  const {
    path: mkvToolNixPath,
    setPath: setMkvToolNixPath,
    detection: mkvtoolnixFound,
    detectNow: handleDetectMkvToolNix,
    handleBlur: handlePathBlur,
  } = useToolPathDetection({
    ready: config !== null,
    initialPath: config?.externalTools?.mkvToolNixPath ?? "",
    detectPath: detectMkvToolNixPath,
    persistPath: persistMkvToolNixPath,
  });

  const {
    path: betterMediaInfoPath,
    setPath: setBetterMediaInfoPath,
    detection: betterMediaInfoDetection,
    detectNow: handleDetectBetterMediaInfo,
    handleBlur: handleBetterMediaInfoPathBlur,
  } = useToolPathDetection({
    ready: config !== null,
    initialPath: config?.externalTools?.betterMediaInfoPath ?? "",
    detectPath: detectBetterMediaInfoPath,
    persistPath: persistBetterMediaInfoPath,
    onFoundChange: setBetterMediaInfoAvailable,
  });

  const handleBrowseMkvToolNixPath = async () => {
    const directory = await open({
      directory: true,
      defaultPath: mkvToolNixPath.trim() || undefined,
    });
    if (typeof directory === "string" && directory.length > 0) {
      setMkvToolNixPath(directory);
      persistMkvToolNixPath(directory);
    }
  };

  const handleBrowseBetterMediaInfoPath = async () => {
    const directory = await open({
      directory: true,
      defaultPath: betterMediaInfoPath.trim() || undefined,
    });
    if (typeof directory === "string" && directory.length > 0) {
      setBetterMediaInfoPath(directory);
      persistBetterMediaInfoPath(directory);
    }
  };

  if (!config) {
    return null;
  }

  const appearancePanel = (
    <Box>
      <SectionHeader
        icon={<PaletteIcon fontSize="small" />}
        title={t("settings.appearance")}
      />
      <SettingRow label={t("settings.mode")}>
        <ToggleButtonGroup
          value={config.displayMode}
          exclusive
          size="small"
          onChange={(_e, value) => {
            if (value !== null) {
              updateConfig({ displayMode: value as Protocol.DisplayMode });
            }
          }}
          sx={{ "& .MuiToggleButton-root": { textTransform: "none" } }}
        >
          <ToggleButton
            value={Protocol.DisplayMode.Auto}
            sx={{ px: 1.5, gap: 0.5 }}
          >
            <BrightnessAutoIcon sx={{ fontSize: 16 }} />
            <Typography variant="caption">{t("settings.autoMode")}</Typography>
          </ToggleButton>
          <ToggleButton
            value={Protocol.DisplayMode.Light}
            sx={{ px: 1.5, gap: 0.5 }}
          >
            <LightModeIcon sx={{ fontSize: 16 }} />
            <Typography variant="caption">{t("settings.lightMode")}</Typography>
          </ToggleButton>
          <ToggleButton
            value={Protocol.DisplayMode.Dark}
            sx={{ px: 1.5, gap: 0.5 }}
          >
            <DarkModeIcon sx={{ fontSize: 16 }} />
            <Typography variant="caption">{t("settings.darkMode")}</Typography>
          </ToggleButton>
        </ToggleButtonGroup>
      </SettingRow>
      <SettingRow label={t("settings.theme")}>
        <FormControl size="small" sx={{ minWidth: 150 }}>
          <Select
            value={config.theme}
            onChange={(e) =>
              updateConfig({ theme: e.target.value as Protocol.Theme })
            }
          >
            {Protocol.getThemes().map((theme) => (
              <MenuItem key={theme} value={theme}>
                {t(`settings.theme${theme}`)}
              </MenuItem>
            ))}
          </Select>
        </FormControl>
      </SettingRow>
      <SettingRow label={t("settings.language")}>
        <FormControl size="small" sx={{ minWidth: 180 }}>
          <Select
            value={config.language}
            onChange={(e) =>
              updateConfig({
                language: e.target.value as Protocol.Language,
              })
            }
          >
            {Protocol.getLanguages().map((lang) => (
              <MenuItem key={lang} value={lang}>
                {Protocol.getLanguageLabel(lang)}
              </MenuItem>
            ))}
          </Select>
        </FormControl>
      </SettingRow>
    </Box>
  );

  const formattingPanel = (() => {
    const formatting = config.formatting;
    const stream = formatting[formatStreamKind];
    const updateStream = (
      field: "bitRate" | "size",
      next: Protocol.ConfigFormatField,
    ) =>
      updateConfig({
        formatting: {
          ...formatting,
          [formatStreamKind]: { ...stream, [field]: next },
        },
      });
    return (
      <Box>
        <SectionHeader
          icon={<TuneIcon fontSize="small" />}
          title={t("settings.formatting")}
        />
        <Tabs
          value={formatStreamKind}
          onChange={(_e, value: FormatStreamKind) =>
            setFormatStreamKind(value)
          }
          sx={{
            mb: 2,
            minHeight: 40,
            borderBottom: 1,
            borderColor: "divider",
            "& .MuiTab-root": { minHeight: 40, textTransform: "none" },
          }}
        >
          <Tab
            value="video"
            icon={<TrackTypeIcon type="video" />}
            iconPosition="start"
            label={t("settings.video")}
          />
          <Tab
            value="audio"
            icon={<TrackTypeIcon type="audio" />}
            iconPosition="start"
            label={t("settings.audio")}
          />
          <Tab
            value="subtitle"
            icon={<TrackTypeIcon type="subtitles" />}
            iconPosition="start"
            label={t("settings.subtitles")}
          />
        </Tabs>
        <Stack spacing={2}>
          <FormatFieldControls
            label={t("settings.bitRate")}
            value={stream.bitRate}
            onChange={(next) => updateStream("bitRate", next)}
          />
          <FormatFieldControls
            label={t("settings.size")}
            value={stream.size}
            onChange={(next) => updateStream("size", next)}
          />
        </Stack>
      </Box>
    );
  })();

  const groupPanel = (
    <Box>
      <SectionHeader
        icon={<PermMediaIcon fontSize="small" />}
        title={t("groupMode.title")}
      />
      <FormControl>
        <RadioGroup
          value={config.groupMode}
          onChange={(e) =>
            updateConfig({ groupMode: e.target.value as Protocol.GroupMode })
          }
        >
          {Protocol.getGroupModes().map((mode) => (
            <FormControlLabel
              key={mode}
              value={mode}
              control={<Radio size="small" />}
              label={
                <Typography variant="body2">
                  {t(Protocol.groupModeLabelKey(mode))}
                </Typography>
              }
            />
          ))}
        </RadioGroup>
      </FormControl>
    </Box>
  );

  const profilesPanel = (() => {
    const activeProfile =
      config.profiles.find((p) => p.name === config.activeProfile) ??
      config.profiles[0];
    if (!activeProfile) {
      return null;
    }
    const trimmed = newProfileName.trim();
    const canAdd =
      trimmed.length > 0 && !config.profiles.some((p) => p.name === trimmed);
    const canDelete = config.activeProfile !== Protocol.DEFAULT_PROFILE_NAME;
    return (
      <Box
        sx={{
          display: "flex",
          flexDirection: "column",
          flex: 1,
          minHeight: 0,
        }}
      >
        <SectionHeader
          icon={<PersonIcon fontSize="small" />}
          title={t("settings.profiles")}
        />
        <SettingRow label={t("settings.activeProfile")}>
          <FormControl size="small" sx={{ minWidth: 180 }}>
            <Select
              value={config.activeProfile}
              onChange={(e) => setActiveProfile(e.target.value)}
            >
              {config.profiles.map((p) => (
                <MenuItem key={p.name} value={p.name}>
                  {p.name}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
        </SettingRow>
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            gap: 1,
            py: 1,
            borderBottom: 1,
            borderColor: "divider",
          }}
        >
          <TextField
            size="small"
            placeholder={t("settings.newProfileName")}
            value={newProfileName}
            onChange={(e) => setNewProfileName(e.target.value)}
            sx={{ flex: 1 }}
            slotProps={{
              input: {
                endAdornment: (
                  <ClearAdornment
                    value={newProfileName}
                    onClear={() => setNewProfileName("")}
                    label={t("settings.clear")}
                  />
                ),
              },
            }}
          />
          <Button
            variant="outlined"
            size="small"
            disabled={!canAdd}
            onClick={async () => {
              await addProfile(trimmed);
              setNewProfileName("");
            }}
            sx={{
              height: 36,
              textTransform: "none",
              whiteSpace: "nowrap",
            }}
          >
            {t("settings.addProfile")}
          </Button>
          <Button
            variant="outlined"
            size="small"
            color="error"
            disabled={!canDelete}
            onClick={() => deleteActiveProfile()}
            sx={{
              height: 36,
              textTransform: "none",
              whiteSpace: "nowrap",
            }}
          >
            {t("settings.deleteProfile")}
          </Button>
          <Button
            variant="outlined"
            size="small"
            onClick={() => resetActiveProfileTemplates()}
            sx={{
              height: 36,
              textTransform: "none",
              whiteSpace: "nowrap",
            }}
          >
            {t("settings.reset")}
          </Button>
        </Box>
        <Tabs
          value={profileTab}
          onChange={(_e, value: ProfileTab) => setProfileTab(value)}
          sx={{
            mt: 1,
            minHeight: 40,
            borderBottom: 1,
            borderColor: "divider",
            "& .MuiTab-root": { minHeight: 40, textTransform: "none" },
          }}
        >
          <Tab
            value={ProfileTab.TrackSelection}
            label={t("settings.trackSelection")}
          />
          <Tab value={ProfileTab.Languages} label={t("settings.languages")} />
          <Tab value={ProfileTab.TrackNames} label={t("settings.trackNames")} />
          <Tab
            value={ProfileTab.Automation}
            label={t("settings.automation")}
          />
        </Tabs>

        {profileTab === ProfileTab.TrackSelection && (
          <Stack spacing={1.5} sx={{ py: 2 }}>
            {(
              [
                {
                  typeKey: "video" as const,
                  selectKey: "selectVideo" as const,
                  languagesKey: "videoLanguagesForTrackSelection" as const,
                  label: t("settings.video"),
                },
                {
                  typeKey: "audio" as const,
                  selectKey: "selectAudio" as const,
                  languagesKey: "audioLanguagesForTrackSelection" as const,
                  label: t("settings.audio"),
                },
                {
                  typeKey: "subtitles" as const,
                  selectKey: "selectSubtitle" as const,
                  languagesKey: "subtitleLanguagesForTrackSelection" as const,
                  label: t("settings.subtitles"),
                },
                {
                  typeKey: "chapters" as const,
                  selectKey: "selectChapters" as const,
                  languagesKey: null,
                  label: t("settings.chapters"),
                },
                {
                  typeKey: "attachments" as const,
                  selectKey: "selectAttachments" as const,
                  languagesKey: null,
                  label: t("settings.attachments"),
                },
              ] as const
            ).map((row) => (
              <Box key={row.typeKey}>
                <Typography
                  variant="caption"
                  color="text.secondary"
                  sx={{ fontWeight: 600 }}
                >
                  {row.label}
                </Typography>
                <Box
                  sx={{
                    display: "flex",
                    alignItems: "center",
                    gap: 1,
                    mt: 0.5,
                  }}
                >
                  <FormControlLabel
                    sx={{ mr: 0 }}
                    control={
                      <Checkbox
                        size="small"
                        checked={activeProfile[row.selectKey]}
                        onChange={(e) =>
                          updateActiveProfile({
                            [row.selectKey]: e.target.checked,
                          })
                        }
                      />
                    }
                    label={
                      <Typography variant="caption">
                        {row.languagesKey
                          ? t("settings.onlyAutoSelectOnDropForLanguages")
                          : t("settings.autoSelectOnDrop")}
                      </Typography>
                    }
                  />
                  {row.languagesKey && (
                    <TextField
                      size="small"
                      value={activeProfile[row.languagesKey] ?? ""}
                      onChange={(e) =>
                        updateActiveProfile({
                          [row.languagesKey]: e.target.value,
                        })
                      }
                      sx={{ flex: 1 }}
                      slotProps={{
                        input: {
                          endAdornment: (
                            <ClearAdornment
                              value={activeProfile[row.languagesKey] ?? ""}
                              onClear={() =>
                                updateActiveProfile({ [row.languagesKey]: "" })
                              }
                              label={t("settings.clear")}
                            />
                          ),
                        },
                      }}
                    />
                  )}
                </Box>
              </Box>
            ))}
          </Stack>
        )}

        {profileTab === ProfileTab.Languages && (
          <Box
            sx={{
              py: 2,
              flex: 1,
              minHeight: 0,
              display: "flex",
              flexDirection: "column",
            }}
          >
            <TrackTypeToggle value={langType} onChange={setLangType} />
            <LanguagePicker
              key={langType}
              availableLanguages={MKV_LANGUAGES}
              preferredCodes={parseLanguageCodes(
                activeProfile[PREFERRED_LANGUAGES_KEY[langType]] ?? "",
              )}
              onPreferredCodesChange={(codes) =>
                updateActiveProfile({
                  [PREFERRED_LANGUAGES_KEY[langType]]: codes.join(", "),
                })
              }
            />
          </Box>
        )}

        {profileTab === ProfileTab.TrackNames &&
          (() => {
            const preferredCodes = parseLanguageCodes(
              activeProfile[PREFERRED_LANGUAGES_KEY[langType]] ?? "",
            );
            const preferredSet = new Set(preferredCodes);
            const otherCodes = ALL_LANGUAGE_CODES.filter(
              (code) => !preferredSet.has(code),
            );
            const f = trackNameFilter.trim().toLowerCase();
            const matches = (code: string) =>
              !f ||
              languageLabel(code).toLowerCase().includes(f) ||
              code.toLowerCase().includes(f);
            const filteredPreferred = preferredCodes.filter(matches);
            const filteredOther = otherCodes.filter(matches);
            const trackNamesMap = activeProfile[TRACK_NAMES_KEY[langType]] ?? {};
            const currentNames = selectedTrackNameLang
              ? (trackNamesMap[selectedTrackNameLang] ?? "")
              : "";

            const visibleCodes = [...filteredPreferred, ...filteredOther];

            const renderRow = (code: string) => (
              <Box
                key={code}
                data-code={code}
                onClick={() => {
                  setSelectedTrackNameLang(code);
                  trackNameListRef.current?.focus();
                }}
                sx={{
                  px: 1,
                  py: 0.5,
                  cursor: "pointer",
                  fontSize: "0.8125rem",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  color: (trackNamesMap[code] ?? "").trim()
                    ? "success.main"
                    : undefined,
                  bgcolor:
                    selectedTrackNameLang === code
                      ? "action.selected"
                      : undefined,
                  "&:hover": { bgcolor: "action.hover" },
                }}
              >
                {languageLabel(code)}
              </Box>
            );

            const handleListKeyDown = (
              e: React.KeyboardEvent<HTMLDivElement>,
            ) => {
              if (e.key !== "ArrowDown" && e.key !== "ArrowUp") {
                return;
              }
              e.preventDefault();
              if (visibleCodes.length === 0) {
                return;
              }
              const idx = visibleCodes.indexOf(selectedTrackNameLang);
              let nextIdx: number;
              if (e.key === "ArrowDown") {
                nextIdx = idx < 0 ? 0 : Math.min(idx + 1, visibleCodes.length - 1);
              } else {
                nextIdx = idx < 0 ? visibleCodes.length - 1 : Math.max(idx - 1, 0);
              }
              const nextCode = visibleCodes[nextIdx];
              setSelectedTrackNameLang(nextCode);
              trackNameListRef.current
                ?.querySelector(`[data-code="${nextCode}"]`)
                ?.scrollIntoView({ block: "nearest" });
            };

            return (
              <Box
                sx={{
                  py: 2,
                  flex: 1,
                  minHeight: 0,
                  display: "flex",
                  flexDirection: "column",
                }}
              >
                <TrackTypeToggle value={langType} onChange={setLangType} />
                <Box sx={{ display: "flex", gap: 1, flex: 1, minHeight: 0 }}>
                  <Box
                    sx={{
                      flex: 1,
                      minWidth: 0,
                      display: "flex",
                      flexDirection: "column",
                    }}
                  >
                    <TextField
                      size="small"
                      placeholder={t("settings.filter")}
                      value={trackNameFilter}
                      onChange={(e) => setTrackNameFilter(e.target.value)}
                      sx={{ mb: 1 }}
                      slotProps={{
                        input: {
                          endAdornment: (
                            <ClearAdornment
                              value={trackNameFilter}
                              onClear={() => setTrackNameFilter("")}
                              label={t("settings.clear")}
                            />
                          ),
                        },
                      }}
                    />
                    <Box
                      ref={trackNameListRef}
                      tabIndex={0}
                      onKeyDown={handleListKeyDown}
                      sx={{
                        flex: 1,
                        overflow: "auto",
                        border: 1,
                        borderColor: "divider",
                        borderRadius: 1,
                        outline: "none",
                      }}
                    >
                      {filteredPreferred.map(renderRow)}
                      {filteredPreferred.length > 0 &&
                        filteredOther.length > 0 && <Divider />}
                      {filteredOther.map(renderRow)}
                    </Box>
                  </Box>
                  <Box
                    sx={{
                      flex: 1,
                      minWidth: 0,
                      display: "flex",
                      flexDirection: "column",
                    }}
                  >
                    {/* Invisible mirror of the left filter so the text box
                        lines up exactly with the language list. */}
                    <TextField
                      size="small"
                      disabled
                      aria-hidden
                      sx={{ mb: 1, visibility: "hidden" }}
                    />
                    <TextField
                      multiline
                      disabled={!selectedTrackNameLang}
                      placeholder={t("settings.trackNamesHint")}
                      value={currentNames}
                      onChange={(e) => {
                        const next = { ...trackNamesMap };
                        if (e.target.value) {
                          next[selectedTrackNameLang] = e.target.value;
                        } else {
                          delete next[selectedTrackNameLang];
                        }
                        updateActiveProfile({
                          [TRACK_NAMES_KEY[langType]]: next,
                        });
                      }}
                      sx={{
                        flex: 1,
                        "& .MuiInputBase-root": {
                          height: "100%",
                          alignItems: "flex-start",
                        },
                        "& .MuiInputBase-inputMultiline": {
                          height: "100% !important",
                          overflow: "auto !important",
                        },
                      }}
                    />
                  </Box>
                </Box>
              </Box>
            );
          })()}

        {profileTab === ProfileTab.Automation &&
          (() => {
            const automation = activeProfile.automation ?? {
              reset_und_language: { enabled: false, language: "en" },
              set_track_name: { enabled: false },
              reset_default_track: { enabled: false },
              reset_forced_display: { enabled: false },
            };
            const langOptions = buildCombinedLanguageOptions(activeProfile);
            const updateAutomation = (
              patch: Partial<Protocol.ConfigAutomation>,
            ) =>
              updateActiveProfile({ automation: { ...automation, ...patch } });
            return (
              <Box sx={{ py: 2 }}>
                <Typography
                  variant="subtitle2"
                  sx={{ fontWeight: 600, mb: 1.5 }}
                >
                  {t("settings.automation")}
                </Typography>
                <Stack spacing={1.5}>
                  <Box
                    sx={{ display: "flex", alignItems: "center", gap: 1 }}
                  >
                    <FormControlLabel
                      sx={{ mr: 0 }}
                      control={
                        <Checkbox
                          size="small"
                          checked={automation.reset_und_language.enabled}
                          onChange={(e) =>
                            updateAutomation({
                              reset_und_language: {
                                ...automation.reset_und_language,
                                enabled: e.target.checked,
                              },
                            })
                          }
                        />
                      }
                      label={
                        <Typography variant="caption">
                          {t("settings.automationResetUndLanguage")}
                        </Typography>
                      }
                    />
                    <Box sx={{ width: 260 }}>
                      <Autocomplete
                        size="small"
                        fullWidth
                        disableClearable
                        options={langOptions.options}
                        filterOptions={(opts) => opts}
                        value={automation.reset_und_language.language}
                        getOptionLabel={(code) => fullLanguageLabel(code)}
                        renderOption={(props, code, state) => {
                          const { key, ...rest } = props;
                          const isFirstRest =
                            state.index === langOptions.preferredCount &&
                            langOptions.preferredCount > 0 &&
                            langOptions.preferredCount <
                              langOptions.options.length;
                          return (
                            <Box
                              component="li"
                              key={key}
                              {...rest}
                              sx={{
                                borderTop: isFirstRest ? 1 : 0,
                                borderColor: "divider",
                              }}
                            >
                              {fullLanguageLabel(code)}
                            </Box>
                          );
                        }}
                        onChange={(_e, code) =>
                          updateAutomation({
                            reset_und_language: {
                              ...automation.reset_und_language,
                              language: code ?? "",
                            },
                          })
                        }
                        renderInput={(params) => <TextField {...params} />}
                      />
                    </Box>
                  </Box>
                  <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                    <FormControlLabel
                      sx={{ mr: 0 }}
                      control={
                        <Checkbox
                          size="small"
                          checked={automation.set_track_name.enabled}
                          onChange={(e) =>
                            updateAutomation({
                              set_track_name: {
                                ...automation.set_track_name,
                                enabled: e.target.checked,
                              },
                            })
                          }
                        />
                      }
                      label={
                        <Typography variant="caption">
                          {t("settings.automationSetTrackName")}
                        </Typography>
                      }
                    />
                  </Box>
                  <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                    <FormControlLabel
                      sx={{ mr: 0 }}
                      control={
                        <Checkbox
                          size="small"
                          checked={automation.reset_default_track.enabled}
                          onChange={(e) =>
                            updateAutomation({
                              reset_default_track: {
                                ...automation.reset_default_track,
                                enabled: e.target.checked,
                              },
                            })
                          }
                        />
                      }
                      label={
                        <Typography variant="caption">
                          {t("settings.automationResetDefaultTrack")}
                        </Typography>
                      }
                    />
                  </Box>
                  <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
                    <FormControlLabel
                      sx={{ mr: 0 }}
                      control={
                        <Checkbox
                          size="small"
                          checked={automation.reset_forced_display.enabled}
                          onChange={(e) =>
                            updateAutomation({
                              reset_forced_display: {
                                ...automation.reset_forced_display,
                                enabled: e.target.checked,
                              },
                            })
                          }
                        />
                      }
                      label={
                        <Typography variant="caption">
                          {t("settings.automationResetForcedDisplay")}
                        </Typography>
                      }
                    />
                  </Box>
                </Stack>
              </Box>
            );
          })()}
      </Box>
    );
  })();

  const integrationPanel = (
    <Box>
      <SectionHeader
        icon={<ExtensionIcon fontSize="small" />}
        title={t("settings.integration")}
      />
      <Stack spacing={2}>
        <Paper variant="outlined" sx={{ p: 2, borderRadius: 2 }}>
          <SectionHeader
            icon={<MovieIcon fontSize="small" />}
            title={t("settings.mkvToolNix")}
          />
          <Box sx={{ py: 1 }}>
            <ExternalToolPathRow
              label={t("settings.mkvToolNixPath")}
              value={mkvToolNixPath}
              status={mkvtoolnixFound ?? false}
              foundLabel={t("settings.mkvToolNixFound")}
              notFoundLabel={t("settings.mkvToolNixNotFound")}
              browseLabel={t("settings.browse")}
              detectLabel={t("settings.detect")}
              onChange={setMkvToolNixPath}
              onBlur={handlePathBlur}
              onBrowse={handleBrowseMkvToolNixPath}
              onDetect={handleDetectMkvToolNix}
            />
          </Box>
        </Paper>
        <Paper variant="outlined" sx={{ p: 2, borderRadius: 2 }}>
          <SectionHeader
            icon={<InfoIcon fontSize="small" />}
            title={t("settings.betterMediaInfo")}
          />
          <Box sx={{ py: 1 }}>
            <ExternalToolPathRow
              label={t("settings.betterMediaInfoPath")}
              value={betterMediaInfoPath}
              status={betterMediaInfoDetection}
              foundLabel={t("settings.betterMediaInfoFound")}
              notFoundLabel={t("settings.betterMediaInfoNotFound")}
              browseLabel={t("settings.browse")}
              detectLabel={t("settings.detect")}
              onChange={setBetterMediaInfoPath}
              onBlur={handleBetterMediaInfoPathBlur}
              onBrowse={handleBrowseBetterMediaInfoPath}
              onDetect={handleDetectBetterMediaInfo}
            />
          </Box>
        </Paper>
      </Stack>
    </Box>
  );

  const updatePanel = (
    <Box>
      <SectionHeader
        icon={<UpdateIcon fontSize="small" />}
        title={t("settings.update")}
      />
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, py: 1 }}>
        <Typography variant="body2" color="text.secondary">
          {t("settings.checkNewVersion")}
        </Typography>
        <FormControl size="small" sx={{ minWidth: 150 }}>
          <Select
            value={
              config.update?.checkInterval ??
              Protocol.UpdateCheckInterval.Weekly
            }
            onChange={(e) => {
              const next = e.target.value as Protocol.UpdateCheckInterval;
              updateConfig({
                update: {
                  checkInterval: next,
                  lastChecked: config.update?.lastChecked ?? 0,
                  lastVersion: config.update?.lastVersion ?? "",
                  ignoreVersion: config.update?.ignoreVersion ?? "",
                },
              });
            }}
          >
            <MenuItem value={Protocol.UpdateCheckInterval.Daily}>
              {t("settings.daily")}
            </MenuItem>
            <MenuItem value={Protocol.UpdateCheckInterval.Weekly}>
              {t("settings.weekly")}
            </MenuItem>
            <MenuItem value={Protocol.UpdateCheckInterval.Monthly}>
              {t("settings.monthly")}
            </MenuItem>
          </Select>
        </FormControl>
      </Box>
    </Box>
  );

  return (
    <Box
      sx={{
        width: "100%",
        maxWidth: 960,
        mx: "auto",
        py: 2,
        px: 1,
        display: "flex",
        gap: 2,
        height: "100%",
        minHeight: 0,
      }}
    >
      <Tabs
        orientation="vertical"
        value={tab}
        onChange={(_e, value: SettingsTab) => setTab(value)}
        sx={{
          borderRight: 1,
          borderColor: "divider",
          minWidth: 180,
          "& .MuiTab-root": {
            minHeight: 40,
            alignItems: "center",
            justifyContent: "flex-start",
            textAlign: "left",
            textTransform: "none",
          },
        }}
      >
        <Tab
          value={SettingsTab.Appearance}
          icon={<PaletteIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("settings.appearance")}
        />
        <Tab
          value={SettingsTab.Formatting}
          icon={<TuneIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("settings.formatting")}
        />
        <Tab
          value={SettingsTab.Group}
          icon={<PermMediaIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("groupMode.title")}
        />
        <Tab
          value={SettingsTab.Integration}
          icon={<ExtensionIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("settings.integration")}
        />
        <Tab
          value={SettingsTab.Profiles}
          icon={<PersonIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("settings.profiles")}
        />
        <Tab
          value={SettingsTab.Update}
          icon={<UpdateIcon sx={{ fontSize: 18 }} />}
          iconPosition="start"
          label={t("settings.update")}
        />
      </Tabs>
      <Box
        sx={{
          flex: 1,
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          overflow: "auto",
        }}
      >
        {tab === SettingsTab.Appearance && appearancePanel}
        {tab === SettingsTab.Formatting && formattingPanel}
        {tab === SettingsTab.Group && groupPanel}
        {tab === SettingsTab.Integration && integrationPanel}
        {tab === SettingsTab.Profiles && profilesPanel}
        {tab === SettingsTab.Update && updatePanel}
      </Box>
    </Box>
  );
}
