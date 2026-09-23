// Theme presets and live application of CSS variables (config.theme).
import type { Theme } from "./types";

export const TOKENS = [
  "bg",
  "surface",
  "panel",
  "panel-solid",
  "elev",
  "hover",
  "active",
  "border",
  "text",
  "text-2",
  "muted",
  "accent",
  "accent-2",
  "ok",
  "warn",
  "err",
  "think",
] as const;

export type Tokens = Record<(typeof TOKENS)[number], string>;

export const PRESETS: Record<string, { label: string; dark: boolean; tokens: Tokens }> = {
  "fluent-blue": {
    label: "Fluent Blue",
    dark: true,
    tokens: {
      bg: "#13161c",
      surface: "rgba(19,22,28,0.97)",
      panel: "rgba(27,31,40,0.86)",
      "panel-solid": "#1a1e27",
      elev: "#222734",
      hover: "rgba(255,255,255,0.05)",
      active: "rgba(90,140,255,0.14)",
      border: "rgba(255,255,255,0.07)",
      text: "#e6e9ef",
      "text-2": "#aab2c0",
      muted: "#7d8595",
      accent: "#4a8dff",
      "accent-2": "#7aa7ff",
      ok: "#3fb68b",
      warn: "#e0a84a",
      err: "#e5604f",
      think: "#9aa6c7",
    },
  },
  graphite: {
    label: "Graphite",
    dark: true,
    tokens: {
      bg: "#151515",
      surface: "rgba(21,21,21,0.97)",
      panel: "rgba(32,32,32,0.86)",
      "panel-solid": "#1d1d1d",
      elev: "#262626",
      hover: "rgba(255,255,255,0.05)",
      active: "rgba(255,255,255,0.09)",
      border: "rgba(255,255,255,0.07)",
      text: "#e8e8e8",
      "text-2": "#b0b0b0",
      muted: "#808080",
      accent: "#8ab4f8",
      "accent-2": "#a8c7fa",
      ok: "#5bb98c",
      warn: "#dcaa55",
      err: "#e06c5a",
      think: "#a3a3a3",
    },
  },
  gruvbox: {
    label: "Gruvbox",
    dark: true,
    tokens: {
      bg: "#1d2021",
      surface: "rgba(29,32,33,0.97)",
      panel: "rgba(40,40,40,0.88)",
      "panel-solid": "#282828",
      elev: "#32302f",
      hover: "rgba(235,219,178,0.06)",
      active: "rgba(254,128,25,0.15)",
      border: "rgba(235,219,178,0.09)",
      text: "#ebdbb2",
      "text-2": "#bdae93",
      muted: "#928374",
      accent: "#fe8019",
      "accent-2": "#fabd2f",
      ok: "#b8bb26",
      warn: "#fabd2f",
      err: "#fb4934",
      think: "#a89984",
    },
  },
  everforest: {
    label: "Everforest",
    dark: true,
    tokens: {
      bg: "#272e33",
      surface: "rgba(39,46,51,0.97)",
      panel: "rgba(45,53,59,0.88)",
      "panel-solid": "#2d353b",
      elev: "#343f44",
      hover: "rgba(211,198,170,0.06)",
      active: "rgba(167,192,128,0.16)",
      border: "rgba(211,198,170,0.09)",
      text: "#d3c6aa",
      "text-2": "#b3ab96",
      muted: "#859289",
      accent: "#a7c080",
      "accent-2": "#83c092",
      ok: "#a7c080",
      warn: "#dbbc7f",
      err: "#e67e80",
      think: "#9da9a0",
    },
  },
  catppuccin: {
    label: "Catppuccin",
    dark: true,
    tokens: {
      bg: "#11111b",
      surface: "rgba(24,24,37,0.97)",
      panel: "rgba(30,30,46,0.88)",
      "panel-solid": "#1e1e2e",
      elev: "#313244",
      hover: "rgba(205,214,244,0.06)",
      active: "rgba(203,166,247,0.16)",
      border: "rgba(205,214,244,0.08)",
      text: "#cdd6f4",
      "text-2": "#bac2de",
      muted: "#7f849c",
      accent: "#cba6f7",
      "accent-2": "#b4befe",
      ok: "#a6e3a1",
      warn: "#f9e2af",
      err: "#f38ba8",
      think: "#9399b2",
    },
  },
  light: {
    label: "Light",
    dark: false,
    tokens: {
      bg: "#f7f8fa",
      surface: "rgba(250,251,252,0.98)",
      panel: "rgba(238,241,246,0.9)",
      "panel-solid": "#eef1f6",
      elev: "#e7ebf2",
      hover: "rgba(20,30,50,0.05)",
      active: "rgba(40,100,230,0.11)",
      border: "rgba(20,30,50,0.09)",
      text: "#1b2230",
      "text-2": "#4a5568",
      muted: "#7a8496",
      accent: "#2f6fe0",
      "accent-2": "#1d5ad0",
      ok: "#1f9a6c",
      warn: "#b8791a",
      err: "#cf4633",
      think: "#5f6b8a",
    },
  },
};

export function resolvedTokens(theme: Theme): Tokens {
  const preset = PRESETS[theme.preset] ?? PRESETS["fluent-blue"];
  return { ...preset.tokens, ...(theme.tokens as Partial<Tokens>) };
}

export function applyTheme(theme: Theme, backdrop: string) {
  const root = document.documentElement;
  const preset = PRESETS[theme.preset] ?? PRESETS["fluent-blue"];
  const tokens = resolvedTokens(theme);
  for (const k of TOKENS) root.style.setProperty(`--${k}`, tokens[k]);
  root.style.setProperty("--font-size", `${theme.font_size || 14}px`);
  if (theme.mono_font) root.style.setProperty("--mono", `"${theme.mono_font}", "Cascadia Code", "JetBrains Mono", Consolas, monospace`);
  else root.style.removeProperty("--mono");
  root.dataset.scheme = preset.dark ? "dark" : "light";
  root.dataset.backdrop = theme.blur && backdrop !== "none" ? "on" : "off";
}
