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

import { useCallback, useEffect, useRef, useState } from "react";
import { useDebouncedEffect } from "../../hooks";

export interface ToolPathDetectionResult {
  found: boolean;
  path: string;
}

export type DetectToolPath = (
  path: string,
  checkRunning?: boolean,
) => Promise<ToolPathDetectionResult>;

interface UseToolPathDetectionOptions {
  ready: boolean;
  initialPath: string;
  detectPath: DetectToolPath;
  persistPath: (path: string) => void;
  onFoundChange?: (found: boolean) => void;
  debounceMs?: number;
}

export function useToolPathDetection({
  ready,
  initialPath,
  detectPath,
  persistPath,
  onFoundChange,
  debounceMs = 250,
}: UseToolPathDetectionOptions) {
  const [path, setPath] = useState("");
  const [detection, setDetection] = useState<boolean | null>(null);
  const initializedRef = useRef(false);

  useEffect(() => {
    if (!ready || initializedRef.current) {
      return;
    }
    initializedRef.current = true;
    setPath(initialPath);
  }, [ready, initialPath]);

  const applyResult = useCallback(
    (result: ToolPathDetectionResult, sourcePath: string) => {
      setDetection(result.found);
      onFoundChange?.(result.found);
      if (result.found && result.path && result.path !== sourcePath) {
        setPath(result.path);
        persistPath(result.path);
      }
    },
    [onFoundChange, persistPath],
  );

  useDebouncedEffect(
    async (isCancelled) => {
      if (!ready || !initializedRef.current) {
        return;
      }
      const trimmed = path.trim();
      try {
        const result = await detectPath(trimmed);
        if (isCancelled()) {
          return;
        }
        applyResult(result, trimmed);
      } catch {
        if (!isCancelled()) {
          setDetection(false);
          onFoundChange?.(false);
        }
      }
    },
    debounceMs,
    [ready, path, detectPath, applyResult, onFoundChange, debounceMs],
  );

  const detectNow = useCallback(async () => {
    if (!ready || !initializedRef.current) {
      return;
    }
    const trimmed = path.trim();
    try {
      const result = await detectPath(trimmed, true);
      applyResult(result, trimmed);
    } catch {
      setDetection(false);
      onFoundChange?.(false);
    }
  }, [ready, path, detectPath, applyResult, onFoundChange]);

  const handleBlur = useCallback(() => {
    if (!ready || !initializedRef.current) {
      return;
    }
    persistPath(path.trim());
  }, [ready, path, persistPath]);

  return {
    path,
    setPath,
    detection,
    detectNow,
    handleBlur,
  };
}
