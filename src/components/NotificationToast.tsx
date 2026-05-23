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

import { Alert, AlertTitle, Snackbar, Typography } from "@mui/material";
import { useMkvStore } from "../store";

const AUTO_HIDE_MS = 5000;

export function NotificationToast() {
  const notification = useMkvStore((s) => s.notification);
  const dismissNotification = useMkvStore((s) => s.dismissNotification);

  const handleClose = (
    _event: React.SyntheticEvent | Event,
    reason?: string,
  ) => {
    if (reason === "clickaway") {
      return;
    }
    dismissNotification();
  };

  return (
    <Snackbar
      key={notification?.id ?? 0}
      open={notification !== null}
      autoHideDuration={AUTO_HIDE_MS}
      onClose={handleClose}
      anchorOrigin={{ vertical: "top", horizontal: "center" }}
    >
      <Alert
        onClose={() => dismissNotification()}
        severity={notification?.kind ?? "success"}
        variant="filled"
        sx={{ maxWidth: 480 }}
      >
        <AlertTitle sx={{ wordBreak: "break-all" }}>
          {notification?.file}
        </AlertTitle>
        <Typography variant="body2" sx={{ wordBreak: "break-word" }}>
          {notification?.detail}
        </Typography>
      </Alert>
    </Snackbar>
  );
}
