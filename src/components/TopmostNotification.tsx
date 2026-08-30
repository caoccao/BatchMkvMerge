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

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Box,
  Button,
  CssBaseline,
  Stack,
  ThemeProvider,
  Typography,
  createTheme,
  useMediaQuery,
} from "@mui/material";
import CheckCircleOutlinedIcon from "@mui/icons-material/CheckCircleOutlined";
import { listen } from "@tauri-apps/api/event";
import type { TopmostNotification as TopmostNotificationPayload } from "../protocol";
import {
  closeTopmostNotification,
  getTopmostNotification,
} from "../service";

const TOPMOST_NOTIFICATION_EVENT = "topmost-notification";

export function TopmostNotification() {
  const [notification, setNotification] =
    useState<TopmostNotificationPayload | null>(null);
  const prefersDarkMode = useMediaQuery("(prefers-color-scheme: dark)");
  const theme = useMemo(
    () =>
      createTheme({
        palette: { mode: prefersDarkMode ? "dark" : "light" },
        typography: { fontSize: 12 },
      }),
    [prefersDarkMode],
  );

  const handleClose = useCallback(() => {
    closeTopmostNotification().catch((err) =>
      console.error("Failed to close topmost notification", err),
    );
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    (async () => {
      const stopListening = await listen<TopmostNotificationPayload>(
        TOPMOST_NOTIFICATION_EVENT,
        (event) => setNotification(event.payload),
      );
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      const current = await getTopmostNotification();
      if (!disposed) {
        setNotification(current);
      }
    })().catch((err) =>
      console.error("Failed to load topmost notification", err),
    );
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        handleClose();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [handleClose]);

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <Box
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="completion-notification-title"
        sx={{
          height: "100vh",
          border: 1,
          borderColor: "divider",
          bgcolor: "background.paper",
          color: "text.primary",
          p: 2,
        }}
      >
        {notification && (
          <Stack spacing={1.25} sx={{ height: "100%" }}>
            <Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
              <CheckCircleOutlinedIcon color="success" />
              <Typography
                id="completion-notification-title"
                variant="h6"
                component="h1"
              >
                {notification.title}
              </Typography>
            </Stack>
            <Typography
              variant="body2"
              noWrap
              title={notification.file}
              sx={{ fontWeight: 500 }}
            >
              {notification.file}
            </Typography>
            <Typography variant="body2">{notification.detail}</Typography>
            <Box sx={{ flex: 1 }} />
            <Button
              variant="contained"
              size="small"
              autoFocus
              onClick={handleClose}
              sx={{ alignSelf: "flex-end", minWidth: 88 }}
            >
              {notification.closeLabel}
            </Button>
          </Stack>
        )}
      </Box>
    </ThemeProvider>
  );
}
