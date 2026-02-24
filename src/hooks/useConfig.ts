import { useEffect, useState } from "react";
import type { KernelConfig } from "../lib/types";
import { loadProjectConfig } from "../lib/commands";

export function useConfig(projectPath: string | null) {
  const [config, setConfig] = useState<KernelConfig | null>(null);

  useEffect(() => {
    if (!projectPath) return;
    loadProjectConfig(projectPath).then(setConfig).catch((err) => {
      console.warn("Failed to load kernel.toml:", err);
      setConfig(null);
    });
  }, [projectPath]);

  return config;
}
