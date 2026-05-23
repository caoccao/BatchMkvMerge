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

import { Box, CircularProgress } from "@mui/material";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import ErrorIcon from "@mui/icons-material/Error";
import ScheduleIcon from "@mui/icons-material/Schedule";
import { QueueItemStatus } from "../protocol";

interface Props {
  status: QueueItemStatus | undefined;
  size?: number;
}

export function FileStatusIcon({ status, size = 18 }: Props) {
  if (!status) {
    return null;
  }
  const iconSx = { fontSize: size };
  switch (status) {
    case QueueItemStatus.Extracting:
      return (
        <Box
          sx={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            width: size,
            height: size,
            flexShrink: 0,
          }}
        >
          <CircularProgress size={size - 4} thickness={5} />
        </Box>
      );
    case QueueItemStatus.Waiting:
      return (
        <ScheduleIcon
          sx={{ ...iconSx, color: "text.secondary", flexShrink: 0 }}
        />
      );
    case QueueItemStatus.Completed:
      return (
        <CheckCircleIcon
          sx={{ ...iconSx, color: "success.main", flexShrink: 0 }}
        />
      );
    case QueueItemStatus.Failed:
      return (
        <ErrorIcon sx={{ ...iconSx, color: "error.main", flexShrink: 0 }} />
      );
    case QueueItemStatus.Cancelled:
      return null;
    default:
      return null;
  }
}
