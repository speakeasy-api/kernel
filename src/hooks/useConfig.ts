import { useEffect, useState } from "react";
import type { KernelConfig } from "../lib/types";
import { loadProjectConfig } from "../lib/commands";

export function useConfig(projectPath: string | null) {
  const [config, setConfig] = useState<KernelConfig | null>(null);

  useEffect(() => {
    if (!projectPath) return;
    loadProjectConfig(projectPath).then(setConfig).catch(() => {
      // Fall back to null on error (e.g. no kernel.toml)
      setConfig(null);
    });
  }, [projectPath]);

  return config;
}
