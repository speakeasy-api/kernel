import { useEffect, useState } from "react";
import type { Mode } from "../lib/types";
import { getBuiltinModes } from "../lib/commands";

export function useModes() {
  const [modes, setModes] = useState<Mode[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    getBuiltinModes()
      .then(setModes)
      .finally(() => setLoading(false));
  }, []);

  return { modes, loading };
}
