// Shared bits of the knowledge-base views: layer metadata, progress helpers.
import type { Component } from "solid-js";
import { BookOpen, BrainCircuit, Library, NotebookPen } from "lucide-solid";
import { state } from "../../lib/store";
import type { KbLayer, KbProgress, KbSource } from "../../lib/types";

type Icon = Component<{ size?: number; "stroke-width"?: number }>;

export const LAYERS: { id: KbLayer; label: string; scope: string; icon: Icon; color: string }[] = [
  { id: "library", label: "Library", scope: "All projects", icon: Library, color: "#4a8dff" },
  { id: "docs", label: "Project docs", scope: "This project", icon: BookOpen, color: "#3fb6a8" },
  { id: "memory", label: "Memory", scope: "All projects", icon: BrainCircuit, color: "#a07cf0" },
  { id: "project_memory", label: "Project memory", scope: "This project", icon: NotebookPen, color: "#e0a84a" },
];

export const layerOf = (id: KbLayer) => LAYERS.find((l) => l.id === id)!;
export const writable = (id: KbLayer) => id === "memory" || id === "project_memory";

export const sources = (): KbSource[] => state.kb.overview?.sources ?? [];
export const sourceOf = (key: string) => sources().find((s) => s.key === key);

/** Active (non-idle) progress for a source, including its store's embedding pass. */
export function progressOf(s: KbSource): KbProgress | null {
  const own = state.kb.progress[s.key];
  if (own && own.stage !== "idle") return own;
  const store = state.kb.progress[s.key[0]];
  if (store && store.stage === "embed" && s.embedded < s.chunks) return store;
  return null;
}

export function fmtBytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${Math.round(n / (1 << 10))} KB`;
  return `${n} B`;
}

export function fmtCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k`;
  return n.toLocaleString();
}

export function stageLabel(p: KbProgress): string {
  if (p.stage === "embed") return "Embedding";
  if (p.stage === "scan") return "Scanning";
  return "Indexing";
}
