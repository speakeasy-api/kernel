import { createHighlighter, type Highlighter } from "shiki";
import { kernelDarkTheme } from "./syntaxTheme";

let highlighterPromise: Promise<Highlighter> | null = null;

const PRELOAD_LANGS = [
  "typescript",
  "javascript",
  "tsx",
  "jsx",
  "rust",
  "python",
  "json",
  "html",
  "css",
  "markdown",
  "bash",
  "toml",
  "yaml",
  "sql",
] as const;

export function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: [kernelDarkTheme],
      langs: [...PRELOAD_LANGS],
    });
  }
  return highlighterPromise;
}

const EXT_MAP: Record<string, string> = {
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  mjs: "javascript",
  cjs: "javascript",
  rs: "rust",
  py: "python",
  json: "json",
  html: "html",
  htm: "html",
  css: "css",
  scss: "css",
  md: "markdown",
  mdx: "markdown",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  toml: "toml",
  yaml: "yaml",
  yml: "yaml",
  sql: "sql",
  go: "go",
  rb: "ruby",
  java: "java",
  kt: "kotlin",
  swift: "swift",
  c: "c",
  cpp: "cpp",
  h: "c",
  hpp: "cpp",
  svelte: "svelte",
  vue: "vue",
  xml: "xml",
  svg: "xml",
  graphql: "graphql",
  gql: "graphql",
  dockerfile: "dockerfile",
  makefile: "makefile",
};

/** Resolve a Shiki language ID from a file path. */
export function langFromPath(filePath: string): string {
  const ext = filePath.split(".").pop()?.toLowerCase() ?? "";
  return EXT_MAP[ext] ?? "text";
}
