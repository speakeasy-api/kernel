import type { ThemeRegistration } from "shiki";

/**
 * Custom dark theme matching the Kernel app palette.
 * Based on VS Code token color scopes.
 */
export const kernelDarkTheme: ThemeRegistration = {
  name: "kernel-dark",
  type: "dark",
  colors: {
    "editor.background": "#0a0a0b",
    "editor.foreground": "#8b8b95",
    "editorLineNumber.foreground": "#3a3a42",
    "editorLineNumber.activeForeground": "#55555e",
    "diffEditor.insertedTextBackground": "rgba(80, 200, 120, 0.08)",
    "diffEditor.removedTextBackground": "rgba(248, 81, 73, 0.08)",
  },
  tokenColors: [
    {
      scope: ["comment", "punctuation.definition.comment"],
      settings: { foreground: "#55555e" },
    },
    {
      scope: ["string", "string.quoted"],
      settings: { foreground: "#7ec699" },
    },
    {
      scope: ["constant.numeric"],
      settings: { foreground: "#d4976c" },
    },
    {
      scope: ["constant.language"],
      settings: { foreground: "#79b8ff" },
    },
    {
      scope: ["keyword", "storage.type", "storage.modifier"],
      settings: { foreground: "#b392f0" },
    },
    {
      scope: ["entity.name.function", "support.function"],
      settings: { foreground: "#79b8ff" },
    },
    {
      scope: ["entity.name.type", "support.type", "entity.name.class"],
      settings: { foreground: "#b392f0" },
    },
    {
      scope: ["variable", "variable.parameter"],
      settings: { foreground: "#ededef" },
    },
    {
      scope: ["variable.other.property", "support.variable.property"],
      settings: { foreground: "#8b8b95" },
    },
    {
      scope: ["entity.name.tag"],
      settings: { foreground: "#7ec699" },
    },
    {
      scope: ["entity.other.attribute-name"],
      settings: { foreground: "#d4976c" },
    },
    {
      scope: ["punctuation"],
      settings: { foreground: "#55555e" },
    },
    {
      scope: [
        "punctuation.definition.tag",
        "punctuation.separator",
        "punctuation.terminator",
      ],
      settings: { foreground: "#55555e" },
    },
    {
      scope: ["meta.decorator", "meta.annotation"],
      settings: { foreground: "#d4976c" },
    },
    {
      scope: ["keyword.operator"],
      settings: { foreground: "#8b8b95" },
    },
    {
      scope: ["string.regexp"],
      settings: { foreground: "#d4976c" },
    },
    {
      scope: ["markup.heading"],
      settings: { foreground: "#79b8ff", fontStyle: "bold" },
    },
    {
      scope: ["markup.bold"],
      settings: { fontStyle: "bold" },
    },
    {
      scope: ["markup.italic"],
      settings: { fontStyle: "italic" },
    },
    {
      scope: ["markup.inline.raw"],
      settings: { foreground: "#7ec699" },
    },
  ],
};
