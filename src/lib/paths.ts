import { homeDir } from "@tauri-apps/api/path";

let cachedHome: string | null = null;

export async function getHomeDir(): Promise<string> {
  if (!cachedHome) {
    cachedHome = await homeDir();
  }
  return cachedHome;
}

export function shortenPathWithHome(path: string, home: string): string {
  if (path.startsWith(home)) {
    const rest = path.slice(home.length);
    return "~/" + rest.replace(/^\/+/, "");
  }
  return path;
}
