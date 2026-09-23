// Tool metadata for rendering: icon, one-line argument summary, activity label.
import type { Component } from "solid-js";
import {
  AppWindow,
  Braces,
  FilePen,
  FilePlus,
  FileText,
  FolderSearch,
  Globe,
  Link,
  Plug,
  Search,
  SquareTerminal,
  Wrench,
} from "lucide-solid";

type Icon = Component<{ size?: number; "stroke-width"?: number; class?: string }>;

const ICONS: Record<string, Icon> = {
  read: FileText,
  write: FilePlus,
  edit: FilePen,
  glob: FolderSearch,
  grep: Search,
  shell: SquareTerminal,
  code: Braces,
  web_search: Globe,
  web_fetch: Link,
  browser: AppWindow,
};

export function toolIcon(name: string): Icon {
  if (name.startsWith("mcp__")) return Plug;
  return ICONS[name] ?? Wrench;
}

export function toolLabel(name: string): string {
  if (name.startsWith("mcp__")) {
    const [, server, tool] = name.split("__");
    return `${server}.${tool ?? ""}`;
  }
  return name;
}

const obj = (a: unknown): Record<string, unknown> => (a && typeof a === "object" ? (a as Record<string, unknown>) : {});
const str = (v: unknown) => (typeof v === "string" ? v : v == null ? "" : JSON.stringify(v));

/** One-line summary of a tool call's arguments (path / command / query ...). */
export function toolSummary(name: string, args: unknown): string {
  const a = obj(args);
  const pick = (...keys: string[]) => {
    for (const k of keys) if (a[k] != null && a[k] !== "") return str(a[k]);
    return "";
  };
  switch (name) {
    case "read": {
      const p = pick("path", "file");
      const off = a.offset ?? a.start;
      const lim = a.limit;
      return off != null && lim != null ? `${p}:${off}-${Number(off) + Number(lim) - 1}` : p;
    }
    case "write":
    case "edit":
      return pick("path", "file");
    case "glob":
      return pick("pattern", "glob");
    case "grep": {
      const p = pick("pattern", "query");
      const where = pick("path", "glob");
      return where ? `${p}  ${where}` : p;
    }
    case "shell":
      return pick("command", "cmd");
    case "web_search":
      return pick("query", "q");
    case "web_fetch":
      return pick("url");
    case "code":
      return pick("symbol", "query", "name", "action");
    case "browser":
      return [pick("action"), pick("url", "selector", "text")].filter(Boolean).join(" ");
    default: {
      const first = Object.values(a).find((v) => typeof v === "string");
      return first ? String(first) : "";
    }
  }
}

const base = (p: string) => p.split(/[\\/]/).pop() || p;

/** Status line text for the working indicator. */
export function toolActivity(name: string, args: unknown): string {
  const a = obj(args);
  const file = a.path ? base(String(a.path)) : "";
  switch (name) {
    case "read":
      return file ? `Reading ${file}…` : "Reading…";
    case "write":
      return file ? `Writing ${file}…` : "Writing file…";
    case "edit":
      return file ? `Editing ${file}…` : "Editing…";
    case "glob":
    case "grep":
      return "Searching…";
    case "shell":
      return "Running shell…";
    case "code":
      return "Querying code index…";
    case "web_search":
      return "Searching the web…";
    case "web_fetch":
      return "Fetching…";
    case "browser":
      return "Using browser…";
    case "":
      return "Thinking…";
    default:
      return `Running ${toolLabel(name)}…`;
  }
}

export function isDiff(text: string): boolean {
  return /^(---|\+\+\+|@@) /m.test(text) && /^[+-]/m.test(text);
}
