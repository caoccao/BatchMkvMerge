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

import { Box, Tooltip } from "@mui/material";

export const TRACK_TYPE_EMOJI: Record<string, string> = {
  video: "🎞️",
  audio: "🔊",
  subtitles: "💬",
  chapters: "📑",
  attachment: "📎",
  buttons: "🔘",
  images: "🖼️",
};

interface Props {
  type: string;
}

export function TrackTypeIcon({ type }: Props) {
  const emoji = TRACK_TYPE_EMOJI[type] ?? "❓";
  return (
    <Tooltip title={type}>
      <Box component="span" sx={{ fontSize: 16, lineHeight: 1 }}>
        {emoji}
      </Box>
    </Tooltip>
  );
}
