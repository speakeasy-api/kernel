import { useEffect, useState } from "react";
import { getHomeDir } from "../lib/paths";

export function useHomeDir() {
  const [home, setHome] = useState<string | null>(null);

  useEffect(() => {
    getHomeDir().then(setHome);
  }, []);

  return home;
}
