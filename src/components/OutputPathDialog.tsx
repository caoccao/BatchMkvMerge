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

import { useEffect, useRef, useState } from "react";
import {
  Alert,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  IconButton,
  InputAdornment,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import ClearIcon from "@mui/icons-material/Clear";
import { open, save } from "@tauri-apps/plugin-dialog";
import { dirname } from "@tauri-apps/api/path";
import { useTranslation } from "react-i18next";
import { checkOutputPathWritable, outputPathExists } from "../service";

interface Props {
  open: boolean;
  initialValue: string;
  /** Dialog title; defaults to the per-file "Set Output Path" label. */
  title?: string;
  /** "directory" (default) edits an output folder; "file" edits a full output
   *  file path — Browse opens a save dialog and the existence warning targets
   *  the parent directory rather than the (always-absent) output file. */
  mode?: "directory" | "file";
  /** Default file name for the save dialog in "file" mode (the input file). */
  defaultFileName?: string;
  onConfirm: (value: string) => void;
  onClose: () => void;
}

export function OutputPathDialog({
  open: dialogOpen,
  initialValue,
  title,
  mode = "directory",
  defaultFileName,
  onConfirm,
  onClose,
}: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState(initialValue);
  const [error, setError] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (dialogOpen) {
      setValue(initialValue);
      setError(null);
      setMissing(false);
    }
  }, [dialogOpen, initialValue]);

  // Debounced check: warn (non-blocking) when the directory doesn't yet exist.
  // In "file" mode the output file itself is always absent, so we check the
  // parent directory instead.
  useEffect(() => {
    if (!dialogOpen) {
      return;
    }
    const trimmed = value.trim();
    if (trimmed.length === 0) {
      setMissing(false);
      return;
    }
    let cancelled = false;
    const handle = setTimeout(() => {
      const dirPromise =
        mode === "file" ? dirname(trimmed) : Promise.resolve(trimmed);
      dirPromise
        .then((dir) => outputPathExists(dir))
        .then((exists) => {
          if (!cancelled) {
            setMissing(!exists);
          }
        })
        .catch(() => {
          if (!cancelled) {
            setMissing(false);
          }
        });
    }, 300);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [value, dialogOpen, mode]);

  const handleBrowse = async () => {
    try {
      if (mode === "file") {
        // The OS save dialog handles its own overwrite prompt when the user
        // picks an existing file, so we just take whatever it returns.
        const selected = await save({
          defaultPath: value.trim() || defaultFileName || undefined,
        });
        if (typeof selected === "string" && selected.length > 0) {
          setValue(selected);
          setError(null);
        }
        return;
      }
      const directory = await open({
        directory: true,
        defaultPath: value.trim() || undefined,
      });
      if (typeof directory === "string" && directory.length > 0) {
        setValue(directory);
        setError(null);
      }
    } catch (err) {
      setError(String(err));
    }
  };

  const handleConfirm = async () => {
    const trimmed = value.trim();
    if (trimmed.length === 0) {
      onConfirm("");
      onClose();
      return;
    }
    try {
      const ok = await checkOutputPathWritable(trimmed);
      if (!ok) {
        setError(t("merge.outputPathNotWritable"));
        return;
      }
    } catch (err) {
      setError(String(err));
      return;
    }
    onConfirm(trimmed);
    onClose();
  };

  return (
    <Dialog
      open={dialogOpen}
      onClose={onClose}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        }
      }}
      slotProps={{
        transition: {
          onEntered: () => inputRef.current?.focus(),
        },
      }}
      sx={{ "& .MuiDialog-paper": { width: "60vw", maxWidth: "60vw" } }}
    >
      <DialogTitle>{title ?? t("merge.setOutputPath")}</DialogTitle>
      <DialogContent>
        <Stack direction="row" spacing={1} sx={{ mt: 1 }}>
          <TextField
            fullWidth
            inputRef={inputRef}
            size="small"
            label={t("merge.outputPath")}
            value={value}
            onChange={(e) => {
              setValue(e.target.value);
              setError(null);
            }}
            onFocus={(e) => e.target.select()}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                handleConfirm();
              }
            }}
            slotProps={{
              input: {
                endAdornment: value ? (
                  <InputAdornment position="end">
                    <Tooltip title={t("settings.clear")}>
                      <IconButton
                        size="small"
                        aria-label={t("settings.clear")}
                        edge="end"
                        onClick={() => {
                          setValue("");
                          setError(null);
                          inputRef.current?.focus();
                        }}
                      >
                        <ClearIcon fontSize="small" />
                      </IconButton>
                    </Tooltip>
                  </InputAdornment>
                ) : null,
              },
            }}
          />
          <Button
            variant="outlined"
            size="small"
            onClick={handleBrowse}
            sx={{ whiteSpace: "nowrap", textTransform: "none" }}
          >
            {t("merge.browse")}
          </Button>
        </Stack>
        {missing ? (
          <Typography variant="body2" color="error" sx={{ mt: 1 }}>
            {t("merge.outputPathDoesNotExist")}
          </Typography>
        ) : null}
        {error ? (
          <Alert severity="error" sx={{ mt: 2 }}>
            {error}
          </Alert>
        ) : null}
      </DialogContent>
      <DialogActions sx={{ justifyContent: "center", mb: 1 }}>
        <Button
          onClick={handleConfirm}
          variant="contained"
          sx={{ textTransform: "none" }}
        >
          {t("merge.ok")}
        </Button>
        <Button onClick={onClose} sx={{ textTransform: "none" }}>
          {t("merge.cancel")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
