// markdown-it + lazily loaded shiki highlighting (languages load on first use).
import MarkdownIt from "markdown-it";
import { createSignal } from "solid-js";
import type { HighlighterCore, ThemeRegistrationRaw } from "shiki";

const THEME: ThemeRegistrationRaw = {
  name: "xode",
  type: "dark",
  colors: { "editor.background": "#00000000", "editor.foreground": "#d4d9e3" },
  settings: [
    { settings: { foreground: "#d4d9e3" } },
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#6b7486", fontStyle: "italic" } },
    { scope: ["keyword", "storage", "storage.type", "storage.modifier", "keyword.control"], settings: { foreground: "#7aa7ff" } },
    { scope: ["string", "string.quoted", "markup.inline.raw"], settings: { foreground: "#8fc8a0" } },
    { scope: ["constant.numeric", "constant.language", "constant.character"], settings: { foreground: "#e0b870" } },
    { scope: ["entity.name.function", "support.function", "meta.function-call"], settings: { foreground: "#8ab8ff" } },
    { scope: ["entity.name.type", "support.type", "entity.name.class", "entity.other.inherited-class"], settings: { foreground: "#9fd0e8" } },
    { scope: ["variable.parameter", "variable.other.property", "meta.object-literal.key"], settings: { foreground: "#c9cfdb" } },
    { scope: ["entity.name.tag", "meta.tag"], settings: { foreground: "#7aa7ff" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#b8a4f0" } },
    { scope: ["punctuation", "meta.brace"], settings: { foreground: "#9aa3b4" } },
    { scope: ["entity.name.namespace", "entity.name.module"], settings: { foreground: "#b8c2d4" } },
    { scope: ["variable.language", "support.constant"], settings: { foreground: "#e59a8a" } },
    { scope: ["markup.inserted"], settings: { foreground: "#3fb68b" } },
    { scope: ["markup.deleted"], settings: { foreground: "#e5604f" } },
    { scope: ["markup.heading"], settings: { foreground: "#7aa7ff", fontStyle: "bold" } },
  ],
};

const LIGHT_THEME: ThemeRegistrationRaw = {
  name: "xode-light",
  type: "light",
  colors: { "editor.background": "#00000000", "editor.foreground": "#1b2230" },
  settings: [
    { settings: { foreground: "#1b2230" } },
    { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: "#7a8496", fontStyle: "italic" } },
    { scope: ["keyword", "storage", "storage.type", "storage.modifier", "keyword.control"], settings: { foreground: "#1d5ad0" } },
    { scope: ["string", "string.quoted", "markup.inline.raw"], settings: { foreground: "#1f7a4d" } },
    { scope: ["constant.numeric", "constant.language", "constant.character"], settings: { foreground: "#a0620f" } },
    { scope: ["entity.name.function", "support.function", "meta.function-call"], settings: { foreground: "#2f5fb8" } },
    { scope: ["entity.name.type", "support.type", "entity.name.class", "entity.other.inherited-class"], settings: { foreground: "#0f6f8f" } },
    { scope: ["variable.parameter", "variable.other.property", "meta.object-literal.key"], settings: { foreground: "#34405a" } },
    { scope: ["entity.name.tag", "meta.tag"], settings: { foreground: "#1d5ad0" } },
    { scope: ["entity.other.attribute-name"], settings: { foreground: "#6a4fc0" } },
    { scope: ["punctuation", "meta.brace"], settings: { foreground: "#5a6478" } },
    { scope: ["variable.language", "support.constant"], settings: { foreground: "#b8452f" } },
    { scope: ["markup.inserted"], settings: { foreground: "#1f9a6c" } },
    { scope: ["markup.deleted"], settings: { foreground: "#cf4633" } },
  ],
};

const ALIASES: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  js: "javascript",
  py: "python",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  ps: "powershell",
  ps1: "powershell",
  pwsh: "powershell",
  yml: "yaml",
  md: "markdown",
  cs: "csharp",
  "c++": "cpp",
  h: "c",
  hpp: "cpp",
  kt: "kotlin",
  rb: "ruby",
  golang: "go",
};

let highlighter: HighlighterCore | null = null;
let loading: Promise<HighlighterCore> | null = null;
const loaded = new Set<string>();
const failed = new Set<string>();
const pendingLangs = new Set<string>();
/** Bumped when a language finishes loading so rendered markdown re-highlights. */
const [version, setVersion] = createSignal(0);
export const highlightVersion = version;

async function getHighlighter() {
  if (highlighter) return highlighter;
  if (!loading) {
    loading = (async () => {
      const [{ createHighlighterCore }, { createJavaScriptRegexEngine }] = await Promise.all([
        import("shiki/core"),
        import("shiki/engine/javascript"),
      ]);
      highlighter = await createHighlighterCore({ themes: [THEME, LIGHT_THEME], langs: [], engine: createJavaScriptRegexEngine() });
      return highlighter;
    })();
  }
  return loading;
}

async function loadLang(lang: string) {
  if (loaded.has(lang) || failed.has(lang) || pendingLangs.has(lang)) return;
  pendingLangs.add(lang);
  try {
    const [{ bundledLanguages }, h] = await Promise.all([import("shiki/langs"), getHighlighter()]);
    const loader = (bundledLanguages as Record<string, () => Promise<{ default: unknown }>>)[lang];
    if (!loader) {
      failed.add(lang);
      return;
    }
    const mod = await loader();
    await h.loadLanguage(mod.default as Parameters<HighlighterCore["loadLanguage"]>[0]);
    loaded.add(lang);
    cache.clear();
    setVersion((v) => v + 1);
  } catch (e) {
    console.warn("shiki: failed to load", lang, e);
    failed.add(lang);
  } finally {
    pendingLangs.delete(lang);
  }
}

const cache = new Map<string, string>();

function escapeHtml(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function highlight(code: string, rawLang: string): string {
  const lang = ALIASES[rawLang.toLowerCase()] ?? rawLang.toLowerCase();
  if (!lang || lang === "text" || lang === "plain" || lang === "txt") return `<pre class="shiki"><code>${escapeHtml(code)}</code></pre>`;
  if (!loaded.has(lang) || !highlighter) {
    void loadLang(lang);
    return `<pre class="shiki"><code>${escapeHtml(code)}</code></pre>`;
  }
  const key = `${lang}\u0000${code}`;
  const hit = cache.get(key);
  if (hit) return hit;
  let html: string;
  try {
    html = highlighter.codeToHtml(code, { lang, themes: { dark: "xode", light: "xode-light" }, defaultColor: false });
  } catch (e) {
    console.warn("shiki: highlight failed", lang, e);
    html = `<pre class="shiki"><code>${escapeHtml(code)}</code></pre>`;
  }
  if (cache.size > 400) cache.clear();
  cache.set(key, html);
  return html;
}

// lucide "copy" and "check" icon markup (same glyphs as lucide-solid).
export const COPY_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>';
export const CHECK_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';

const md = new MarkdownIt({ html: false, linkify: true, breaks: false });
md.renderer.rules.fence = (tokens, idx) => {
  const t = tokens[idx];
  const lang = (t.info || "").trim().split(/\s+/)[0] || "";
  const code = t.content.replace(/\n$/, "");
  return `<div class="code-block"><div class="code-head"><span>${escapeHtml(lang)}</span><button class="code-copy" title="Copy">${COPY_SVG}</button></div>${highlight(code, lang)}</div>`;
};
md.renderer.rules.code_block = md.renderer.rules.fence;
// ---------- links to local files (pages, images … the agent made)

let linkBase = "";
const [linkBaseVersion, setLinkBaseVersion] = createSignal(0);

/** Folder relative file links resolve against (the chat's cwd / project root). */
export function setLinkBase(base: string) {
  const b = base.replace(/\\/g, "/").replace(/\/+$/, "");
  if (b === linkBase) return;
  linkBase = b;
  setLinkBaseVersion((v) => v + 1);
}
export { linkBaseVersion };

const FILE_EXT = /\.(html?|png|jpe?g|gif|webp|svg|avif|bmp|ico|pdf|md|txt|csv|tsv|json|xml|log|mp4|mov|webm|mp3|wav|ogg|zip|docx?|xlsx?|pptx?)$/i;
const IMG_EXT = /\.(png|jpe?g|gif|webp|svg|avif|bmp|ico)$/i;
const isAbs = (p: string) => p.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith("\\\\");

/** Absolute local path for a link target, or null for URLs of other schemes. */
export function resolveLocal(href: string): string | null {
  let p = href.trim();
  try {
    p = decodeURI(p);
  } catch {
    /* keep as is */
  }
  if (/^file:\/\//i.test(p)) {
    p = p.replace(/^file:\/\//i, "");
    if (/^\/[a-zA-Z]:\//.test(p)) p = p.slice(1);
  } else if (/^[a-z][a-z0-9+.-]*:/i.test(p) && !/^[a-zA-Z]:[\\/]/.test(p)) return null;
  p = p.replace(/[?#].*$/, "");
  if (!p) return null;
  if (isAbs(p)) return p;
  if (p.startsWith("~/")) return p;
  return linkBase ? `${linkBase}/${p.replace(/^\.\//, "")}` : p;
}

/** URL the webview can load a local image from (Tauri asset protocol). */
function assetUrl(path: string): string {
  const w = window as unknown as { __TAURI_INTERNALS__?: { convertFileSrc?: (p: string, proto?: string) => string } };
  const conv = w.__TAURI_INTERNALS__?.convertFileSrc;
  return conv ? conv(path, "asset") : path;
}

const attr = (s: string) => escapeHtml(s).replace(/"/g, "&quot;");

const defaultLink = md.renderer.rules.link_open ?? ((tokens, idx, opts, _env, self) => self.renderToken(tokens, idx, opts));
md.renderer.rules.link_open = (tokens, idx, opts, env, self) => {
  const t = tokens[idx];
  const href = t.attrGet("href") ?? "";
  const local = /^(https?|mailto):/i.test(href) ? null : resolveLocal(href);
  if (local) {
    t.attrSet("href", "#");
    t.attrSet("data-file", local);
    t.attrJoin("class", "file-link");
    t.attrSet("title", local);
  } else {
    t.attrSet("target", "_blank");
    t.attrSet("rel", "noreferrer");
  }
  return defaultLink(tokens, idx, opts, env, self);
};

md.renderer.rules.image = (tokens, idx) => {
  const t = tokens[idx];
  const src = t.attrGet("src") ?? "";
  const alt = t.content || "";
  const local = /^(https?|data):/i.test(src) ? null : resolveLocal(src);
  if (!local) return `<img class="md-img" src="${attr(src)}" alt="${attr(alt)}" loading="lazy">`;
  return `<img class="md-img file-link" src="${attr(assetUrl(local))}" alt="${attr(alt)}" title="${attr(local)}" data-file="${attr(local)}" loading="lazy">`;
};

// `out/report.html` in backticks: clickable when it looks like a file the user may want to open.
md.renderer.rules.code_inline = (tokens, idx) => {
  const c = tokens[idx].content;
  if (FILE_EXT.test(c) && !/\s/.test(c) && c.length < 300) {
    const local = resolveLocal(c);
    if (local) return `<code class="file-link" data-file="${attr(local)}" title="${attr(local)}">${escapeHtml(c)}</code>`;
  }
  return `<code>${escapeHtml(c)}</code>`;
};

export { IMG_EXT };

export function renderMarkdown(text: string): string {
  return md.render(text);
}

/** Warm up the highlighter in the background. */
export function preloadHighlighter() {
  void getHighlighter();
}

/** Delegated click handler for rendered markdown: file links, web links, code-block copy. */
export function handleCodeCopy(e: MouseEvent) {
  const el = e.target as HTMLElement;
  const file = el.closest("[data-file]") as HTMLElement | null;
  if (file) {
    e.preventDefault();
    const path = file.dataset.file!;
    import("./api").then((a) => (e.altKey || e.metaKey ? a.revealInFileManager(path) : a.openInFileManager(path))).catch((err) =>
      import("./store").then((s) => s.toast(`Cannot open ${path}: ${err}`, "error")),
    );
    return;
  }
  const a = el.closest("a[href]") as HTMLAnchorElement | null;
  if (a && /^(https?|mailto):/i.test(a.getAttribute("href") ?? "")) {
    e.preventDefault();
    import("./api").then((x) => x.openExternal(a.href));
    return;
  }
  const btn = el.closest(".code-copy") as HTMLElement | null;
  if (!btn) return;
  const code = btn.closest(".code-block")?.querySelector("pre")?.textContent ?? "";
  navigator.clipboard.writeText(code).catch(() => {});
  btn.innerHTML = CHECK_SVG;
  setTimeout(() => (btn.innerHTML = COPY_SVG), 1200);
}
