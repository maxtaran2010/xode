// Mock knowledge base for the in-browser backend: a small linked vault per layer.
import type { Backend } from "./api";
import type { KbFolder, KbGraph, KbHit, KbLayer, KbNote, KbOverview, KbSource } from "./types";

type Emit = (ev: import("./types").AgentEvent) => void;

interface MNote {
  id: string;
  source: string;
  rel: string;
  title: string;
  tags: string[];
  text: string;
}

const SOURCES: KbSource[] = [
  { key: "g:1", id: 1, layer: "library", name: "Engineering handbook", path: "/Users/dev/kb/handbook", default_on: true, notes: 0, chunks: 0, embedded: 0, bytes: 0 },
  { key: "g:2", id: 2, layer: "library", name: "Prompt templates", path: "/Users/dev/kb/templates", default_on: false, notes: 0, chunks: 0, embedded: 0, bytes: 0 },
  { key: "g:3", id: 3, layer: "memory", name: "Memory", path: "~/.local/share/xode/memory", default_on: true, notes: 0, chunks: 0, embedded: 0, bytes: 0 },
  { key: "p:1", id: 1, layer: "docs", name: "docs", path: "/Users/dev/xode/docs", default_on: true, notes: 0, chunks: 0, embedded: 0, bytes: 0 },
  { key: "p:2", id: 2, layer: "project_memory", name: "Project memory", path: "/Users/dev/xode/.xode/memory", default_on: true, notes: 0, chunks: 0, embedded: 0, bytes: 0 },
];

const para = (s: string, n = 3) => Array.from({ length: n }, () => s).join(" ");

function seed(): MNote[] {
  const n: MNote[] = [];
  const add = (source: string, rel: string, title: string, tags: string[], body: string) => {
    const id = `${source[0]}${n.filter((x) => x.source[0] === source[0]).length + 1}`;
    n.push({ id, source, rel, title, tags, text: `# ${title}\n\n${body}\n` });
  };
  add("g:1", "index.md", "Handbook", ["index"], "Start here. See [[Code review]], [[Release process]], [[Testing]], [[Rust style]] and [[Windows builds]].");
  add("g:1", "process/code-review.md", "Code review", ["process"], `## Checklist\n\n${para("Small diffs, clear names, tests for behaviour changes.")}\n\n## Tone\n\n${para("Ask, do not order. Link to [[Rust style]] for style nits.")}`);
  add("g:1", "process/release.md", "Release process", ["process", "release"], `## Branching\n\n${para("Release branches are cut on Mondays.")}\n\n## Checklist\n\n${para("Run [[Testing]] suites, bump versions, tag, publish notes.")}\n\n## Hotfixes\n\n${para("Cherry-pick onto the release branch, then forward-port.")}`);
  add("g:1", "eng/testing.md", "Testing", ["testing"], `${para("Unit tests near the code, integration tests under tests/. Snapshot outputs of tools.")}\nSee [[Rust style]].`);
  add("g:1", "eng/rust-style.md", "Rust style", ["rust", "style"], `## Errors\n\n${para("anyhow at the edges, thiserror in libraries.")}\n\n## Async\n\n${para("No blocking calls on the runtime; use spawn_blocking.")}`);
  add("g:1", "eng/windows.md", "Windows builds", ["windows", "build"], `${para("MSVC toolchain, pwsh, long paths enabled. See [[Release process]].")}`);
  add("g:1", "eng/profiling.md", "Profiling", ["perf"], para("Use cargo flamegraph; on Windows use WPA. See [[Rust style]]."));
  add("g:2", "review.md", "Review prompt", ["prompt"], para("You are a strict reviewer. Follow [[Code review]]."));
  add("g:2", "plan.md", "Plan prompt", ["prompt"], para("Write a short plan first, then act."));
  add("g:2", "summary.md", "Summary prompt", ["prompt"], para("Summarize in five bullets."));
  add("g:3", "user-prefers-russian.md", "User prefers Russian replies", ["user"], "Reply in Russian unless code comments. Keep answers short.");
  add("g:3", "llama-server-flags.md", "llama-server flags", ["local-models"], para("Use --jinja and -fa. Embeddings need --embedding. Related: [[Windows builds]]."));
  add("p:1", "architecture.md", "Architecture", ["arch"], `## Crates\n\n${para("core, tools, index, net, engine, cli, desktop. The engine is the only API. See [[Compaction]] and [[Knowledge base]].")}\n\n## Events\n\n${para("One AgentEvent stream feeds both frontends.")}`);
  add("p:1", "compaction.md", "Compaction", ["context"], para("PATH.md + STATE handoff, working set of touched files. See [[Architecture]]."));
  add("p:1", "kb.md", "Knowledge base", ["kb"], para("Four layers in Turso: library, docs, memory, project memory. See [[Architecture]] and [[Rust style]]."));
  add("p:2", "decisions/turso.md", "Turso for the index", ["decision", "db"], "Moved the code index and the KB to Turso: vectors + FTS. The chat store stays on rusqlite. See [[Knowledge base]].");
  add("p:2", "gotchas/windows-lock.md", "Windows file locks", ["gotcha", "windows"], "Turso locks DB files per process; the second window opens read-only. See [[Windows builds]].");
  add("p:2", "todo.md", "Open questions", [], "Graph layout for 10k+ nodes. [[Knowledge base]] PDF ingest.");
  return n;
}

type KbApi = Pick<
  Backend,
  | "kbOverview"
  | "kbAddSource"
  | "kbRemoveSource"
  | "kbUpdateSource"
  | "kbReindex"
  | "kbSearch"
  | "kbNote"
  | "kbList"
  | "kbSaveNote"
  | "kbCreateNote"
  | "kbDeleteNote"
  | "kbGraph"
  | "kbTitles"
  | "kbEmbedRetry"
>;

export function mockKb(emit: Emit): KbApi {
  const notes = seed();
  const sources = SOURCES.map((s) => ({ ...s }));
  const stats = () => {
    for (const s of sources) {
      const ns = notes.filter((n) => n.source === s.key);
      s.notes = ns.length;
      s.chunks = ns.reduce((a, n) => a + Math.max(1, n.text.split("\n## ").length), 0);
      s.embedded = s.chunks;
      s.bytes = ns.reduce((a, n) => a + n.text.length, 0);
    }
  };
  stats();
  const layerOf = (key: string): KbLayer => sources.find((s) => s.key === key)?.layer ?? "library";
  const tok = (s: string) => Math.ceil(s.length / 4);
  const linksOf = (n: MNote) => [...new Set([...n.text.matchAll(/\[\[([^\]|#]+)/g)].map((m) => m[1].trim()))];
  const byTitle = (t: string) => notes.find((n) => n.title.toLowerCase() === t.toLowerCase());
  const allowed = (n: MNote, off: string[] = []) => !off.includes(n.source) && !off.includes(layerOf(n.source));

  const view = (n: MNote): KbNote => {
    const lines = n.text.split("\n");
    return {
      id: n.id,
      title: n.title,
      source: n.source,
      source_name: sources.find((s) => s.key === n.source)?.name ?? "",
      layer: layerOf(n.source),
      rel: n.rel,
      abs: `${sources.find((s) => s.key === n.source)?.path}/${n.rel}`,
      tags: n.tags,
      summary: lines.find((l) => l && !l.startsWith("#"))?.slice(0, 160) ?? "",
      tokens: tok(n.text),
      headings: lines.flatMap((l, i) => {
        const m = /^(#{1,6})\s+(.*)$/.exec(l);
        return m ? [[m[1].length, m[2], i + 1] as [number, string, number]] : [];
      }),
      links: linksOf(n).map((t) => ({ id: byTitle(t)?.id ?? null, title: byTitle(t)?.title ?? t })),
      backlinks: notes.filter((o) => linksOf(o).some((t) => t.toLowerCase() === n.title.toLowerCase())).map((o) => ({ id: o.id, title: o.title })),
      text: n.text,
      writable: ["memory", "project_memory"].includes(layerOf(n.source)),
    };
  };

  return {
    kbOverview: async (): Promise<KbOverview> => {
      stats();
      return { sources: structuredClone(sources), embed: { state: "ready", model: "builtin:multilingual-e5-small", error: "" }, owner: true };
    },
    kbAddSource: async (_p, layer, path, name) => {
      const store = layer === "library" || layer === "memory" ? "g" : "p";
      const id = Math.max(...sources.filter((s) => s.key[0] === store).map((s) => s.id)) + 1;
      const s: KbSource = { key: `${store}:${id}`, id, layer: layer as KbLayer, name: name || path.split("/").pop() || path, path, default_on: true, notes: 0, chunks: 0, embedded: 0, bytes: 0 };
      sources.push(s);
      // Simulated indexing progress.
      let done = 0;
      const total = 240;
      const tick = () => {
        done = Math.min(total, done + 37);
        emit({ type: "kb_progress", session: "", source: s.key, stage: done < total ? "index" : "idle", done, total });
        if (done < total) setTimeout(tick, 160);
      };
      setTimeout(tick, 100);
      return structuredClone(s);
    },
    kbRemoveSource: async (_p, key) => {
      const i = sources.findIndex((s) => s.key === key);
      if (i >= 0) sources.splice(i, 1);
      for (let j = notes.length - 1; j >= 0; j--) if (notes[j].source === key) notes.splice(j, 1);
    },
    kbUpdateSource: async (_p, key, name, defaultOn) => {
      const s = sources.find((x) => x.key === key);
      if (!s) return;
      if (name) s.name = name;
      if (defaultOn != null) s.default_on = defaultOn;
    },
    kbReindex: async (_p, key) => {
      emit({ type: "kb_progress", session: "", source: key, stage: "index", done: 1, total: 3 });
      setTimeout(() => emit({ type: "kb_progress", session: "", source: key, stage: "idle", done: 3, total: 3 }), 600);
    },
    kbSearch: async (_p, req): Promise<KbHit[]> => {
      const words = req.q.toLowerCase().split(/\W+/).filter((w) => w.length > 1);
      return notes
        .filter((n) => allowed(n, req.off) && (!req.layer || layerOf(n.source) === req.layer))
        .map((n) => {
          const t = n.text.toLowerCase();
          const score = words.reduce((a, w) => a + (t.split(w).length - 1) + (n.title.toLowerCase().includes(w) ? 3 : 0), 0);
          const line = n.text.split("\n").findIndex((l) => words.some((w) => l.toLowerCase().includes(w)));
          const snippet = (n.text.split("\n")[Math.max(0, line)] || n.text).replace(/^#+\s*/, "").slice(0, 200);
          return { n, score, line, snippet };
        })
        .filter((x) => x.score > 0)
        .sort((a, b) => b.score - a.score)
        .slice(0, req.k ?? 20)
        .map(({ n, score, line, snippet }) => ({
          id: n.id,
          title: n.title,
          heading: "",
          line_start: line + 1,
          line_end: line + 6,
          tokens: 90,
          note_tokens: tok(n.text),
          snippet,
          score,
          layer: layerOf(n.source),
          source: n.source,
          rel: n.rel,
        }));
    },
    kbNote: async (_p, id) => {
      const n = notes.find((x) => x.id === id);
      if (!n) throw new Error(`note ${id} not found`);
      return view(n);
    },
    kbList: async (_p, key, dir, tag): Promise<KbFolder> => {
      const pre = dir ? `${dir}/` : "";
      const folders = new Map<string, number>();
      const rows = [];
      for (const n of notes.filter((x) => x.source === key && x.rel.startsWith(pre) && (!tag || x.tags.includes(tag)))) {
        const rest = n.rel.slice(pre.length);
        if (!tag && rest.includes("/")) {
          const f = pre + rest.split("/")[0];
          folders.set(f, (folders.get(f) ?? 0) + 1);
          continue;
        }
        rows.push({ id: n.id, title: n.title, source: n.source, rel: n.rel, tokens: tok(n.text), tags: n.tags });
      }
      return { folders: [...folders.entries()].sort(), notes: rows };
    },
    kbSaveNote: async (_p, id, text) => {
      const n = notes.find((x) => x.id === id);
      if (n) {
        n.text = text;
        n.title = /^#\s+(.*)$/m.exec(text)?.[1] ?? n.title;
      }
    },
    kbCreateNote: async (_p, global, title, body) => {
      const source = global ? "g:3" : "p:2";
      const id = `${source[0]}${notes.filter((x) => x.source[0] === source[0]).length + 1}`;
      notes.push({ id, source, rel: `${title.toLowerCase().replace(/\W+/g, "-")}.md`, title, tags: [], text: `# ${title}\n\n${body}\n` });
      return id;
    },
    kbDeleteNote: async (_p, id) => {
      const i = notes.findIndex((x) => x.id === id);
      if (i >= 0) notes.splice(i, 1);
    },
    kbGraph: async (_p, off): Promise<KbGraph> => {
      const ns = notes.filter((n) => allowed(n, off));
      const ids = new Set(ns.map((n) => n.id));
      const edges: [string, string][] = [];
      for (const n of ns) for (const t of linksOf(n)) {
        const d = byTitle(t);
        if (d && ids.has(d.id) && d.id !== n.id) edges.push([n.id, d.id]);
      }
      const deg = (id: string) => edges.filter((e) => e[0] === id || e[1] === id).length;
      return {
        nodes: ns.map((n) => ({ id: n.id, title: n.title, layer: layerOf(n.source), source: n.source, tokens: tok(n.text), degree: deg(n.id) })),
        edges,
        hidden: 0,
      };
    },
    kbTitles: async (_p, q) => notes.filter((n) => n.title.toLowerCase().includes(q.toLowerCase())).slice(0, 10).map((n) => [n.id, n.title] as [string, string]),
    kbEmbedRetry: async () => {},
  };
}
