// In-browser mock of the engine, used when window.__TAURI_INTERNALS__ is absent (plain `pnpm dev`).
// Simulates a streaming agent run: thinking/text deltas, tool calls + results, stats ticks,
// a permission prompt, a compaction and a goal check.
import type { Backend } from "./api";
import { defaultConfig } from "./defaults";
import { mockKb } from "./mockKb";
import type {
  AgentEvent,
  CommandInfo,
  Config,
  ContextView,
  FileEntry,
  Gateway,
  LiveStats,
  Message,
  Part,
  PermDecision,
  Project,
  Segment,
  SessionInfo,
  TurnMeta,
} from "./types";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let seq = 0;
const uid = (p = "m") => `${p}-${Date.now().toString(36)}-${(seq++).toString(36)}`;
const now = () => Date.now();

const ROOT = "/Users/dev/xode";

const GATEWAYS: Gateway[] = [
  {
    id: "gw-llama",
    name: "llama-server",
    url: "http://192.168.2.87:8080",
    kind: "openai",
    api_key: "",
    flavor: "llamacpp",
    models: [
      { id: "qwen27b", context: 76800, vision: false },
      { id: "qwen3-coder-30b-a3b", context: 65536, vision: false },
    ],
    enabled: true,
  },
  {
    id: "gw-ollama",
    name: "Ollama",
    url: "http://localhost:11434",
    kind: "openai",
    api_key: "",
    flavor: "ollama",
    models: [
      { id: "gpt-oss:20b", context: 32768, vision: false },
      { id: "gemma3:12b", context: 32768, vision: true },
    ],
    enabled: true,
  },
];

const FILES = [
  "Cargo.toml",
  "README.md",
  "CLAUDE.md",
  "crates/xode-core/Cargo.toml",
  "crates/xode-core/src/lib.rs",
  "crates/xode-core/src/config.rs",
  "crates/xode-core/src/event.rs",
  "crates/xode-core/src/store.rs",
  "crates/xode-core/src/types.rs",
  "crates/xode-core/src/tokens.rs",
  "crates/xode-core/src/agent.rs",
  "crates/xode-core/src/compaction.rs",
  "crates/xode-tools/src/lib.rs",
  "crates/xode-tools/src/edit.rs",
  "crates/xode-tools/src/shell.rs",
  "crates/xode-tools/src/rtk.rs",
  "crates/xode-engine/src/lib.rs",
  "crates/xode-engine/src/engine.rs",
  "crates/xode-engine/src/api.rs",
  "apps/desktop/package.json",
  "apps/desktop/src/App.tsx",
  "apps/desktop/src/main.tsx",
];

function mkMsg(role: Message["role"], parts: Part[], extra: Partial<Message> = {}): Message {
  return { id: uid(), role, parts, kind: "normal", segment: 0, meta: null, created_at: now(), ...extra };
}

function meta(ms: number, pin: number, pout: number, tps: number): TurnMeta {
  return {
    model: "qwen27b",
    prompt_tokens: pin,
    completion_tokens: pout,
    cached_tokens: Math.round(pin * 0.8),
    ttft_ms: 420 + Math.round(Math.random() * 300),
    duration_ms: ms,
    decode_tps: tps,
    prefill_tps: 1850 + Math.random() * 300,
  };
}

const READ_RESULT = `  1 use crate::types::{Message, TurnMeta};
  2 use serde::{Deserialize, Serialize};
  3 use serde_json::Value;
  4
  5 /// Live stats for the speedometer / stats sidebar.
  6 #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
  7 pub struct LiveStats {
  8     pub tps: f64,
  9     pub ttft_ms: u64,
 10     pub prefill_tps: f64,
 11     pub tokens_in: u64,
 12     pub tokens_out: u64,
 13     pub context_used: u64,
 14     pub context_limit: u64,
[... 58 more lines]`;

const EDIT_RESULT = `--- a/crates/xode-core/src/compaction.rs
+++ b/crates/xode-core/src/compaction.rs
@@ -41,9 +41,12 @@ pub fn should_compact(used: u64, cfg: &Config, limit: u64) -> bool {
     if !cfg.compaction.enabled {
         return false;
     }
-    let t = (limit as f64 * cfg.compaction.threshold_ratio as f64) as u64;
-    used >= t
+    let t = cfg.threshold(limit);
+    if used >= t {
+        tracing::debug!(used, t, "compaction threshold reached");
+        return true;
+    }
+    false
 }`;

const SHELL_RESULT = `   Compiling xode-core v0.1.0
   Compiling xode-tools v0.1.0
    Finished \`test\` profile [unoptimized + debuginfo] target(s) in 6.42s
     Running unittests src/lib.rs
running 14 tests
test compaction::tests::threshold_ratio ... ok
test compaction::tests::threshold_abs ... ok
test compaction::tests::reserve_clamps ... ok
test tokens::tests::count_basic ... ok
[... 10 more ok]
test result: ok. 14 passed; 0 failed; finished in 0.21s`;

const GREP_RESULT = `crates/xode-core/src/compaction.rs:44: let t = (limit as f64 * cfg.compaction.threshold_ratio as f64) as u64;
crates/xode-core/src/config.rs:412: pub fn threshold(&self, limit: u64) -> u64 {
crates/xode-engine/src/engine.rs:188: let threshold = cfg.threshold(limit);`;

const FINAL_TEXT = `Compaction now uses \`Config::threshold\`, so the reserve for the handoff reply is respected and the absolute \`threshold_tokens\` override works.

**Changes**
- \`crates/xode-core/src/compaction.rs\` — \`should_compact\` delegates to \`cfg.threshold(limit)\` and logs when it triggers.
- Tests cover ratio, absolute and reserve-clamped thresholds.

Report: [compaction-report.html](out/compaction-report.html)

\`\`\`rust
pub fn should_compact(used: u64, cfg: &Config, limit: u64) -> bool {
    cfg.compaction.enabled && used >= cfg.threshold(limit)
}
\`\`\`

| Case | Limit | Threshold |
|---|---|---|
| ratio 0.82 | 76 800 | 62 976 |
| absolute 60k | 76 800 | 60 000 |
| reserve clamp | 8 192 | 6 144 |

All 14 tests pass.`;

const THINK_1 =
  "The user wants compaction to respect the configured threshold. First I should look at how should_compact computes the threshold today and whether Config::threshold already handles the reserve. Let me read the compaction module and grep for threshold usages across the workspace.";
const THINK_2 =
  "should_compact duplicates the ratio math and ignores threshold_tokens and reserve_tokens. Config::threshold already clamps to limit - reserve. The fix is to delegate. I will edit compaction.rs, then run the core tests.";
const THINK_3 = "Tests pass. I should summarize the change briefly with the threshold table.";

function historyFor(sessionId: string): Message[] {
  const t0 = now() - 1000 * 60 * 42;
  const m: Message[] = [];
  const at = (i: number) => t0 + i * 20000;
  m.push(mkMsg("user", [{ type: "text", text: "Make the compaction trigger respect `threshold_tokens` and the reserve. Run the core tests after." }], { created_at: at(0) }));
  m.push(
    mkMsg(
      "assistant",
      [
        { type: "thinking", text: THINK_1 },
        { type: "text", text: "Let me check how the threshold is computed now." },
        { type: "tool_call", id: "c1", name: "read", args: { path: "crates/xode-core/src/event.rs", offset: 1, limit: 80 } },
        { type: "tool_call", id: "c2", name: "grep", args: { pattern: "threshold", path: "crates" } },
      ],
      { meta: meta(8400, 18250, 312, 38.4), created_at: at(1) },
    ),
  );
  m.push(
    mkMsg(
      "tool",
      [
        { type: "tool_result", id: "c1", name: "read", content: READ_RESULT, is_error: false },
        { type: "tool_result", id: "c2", name: "grep", content: GREP_RESULT, is_error: false },
      ],
      { created_at: at(2) },
    ),
  );
  m.push(
    mkMsg("user", [{ type: "text", text: "[compacted]" }], { kind: "seed", segment: 1, created_at: at(3) }),
  );
  m.push(
    mkMsg(
      "assistant",
      [
        { type: "thinking", text: THINK_2 },
        { type: "tool_call", id: "c3", name: "edit", args: { path: "crates/xode-core/src/compaction.rs", old: "let t = ...", new: "let t = cfg.threshold(limit);" } },
      ],
      { meta: meta(6100, 9120, 244, 41.2), segment: 1, created_at: at(4) },
    ),
  );
  m.push(mkMsg("tool", [{ type: "tool_result", id: "c3", name: "edit", content: EDIT_RESULT, is_error: false }], { segment: 1, created_at: at(5) }));
  m.push(
    mkMsg(
      "assistant",
      [{ type: "tool_call", id: "c4", name: "shell", args: { command: "cargo test -p xode-core" } }],
      { meta: meta(2100, 10480, 38, 40.1), segment: 1, created_at: at(6) },
    ),
  );
  m.push(mkMsg("tool", [{ type: "tool_result", id: "c4", name: "shell", content: SHELL_RESULT, is_error: false }], { segment: 1, created_at: at(7) }));
  m.push(
    mkMsg("assistant", [{ type: "thinking", text: THINK_3 }, { type: "text", text: FINAL_TEXT }], {
      meta: meta(9800, 11320, 402, 39.6),
      segment: 1,
      created_at: at(8),
    }),
  );
  void sessionId;
  return m;
}

const PATH_MD = `# PATH
- read event.rs, grep threshold: should_compact duplicates ratio math (compaction.rs:44)
- Config::threshold clamps to limit - reserve_tokens (config.rs:412)
- edited compaction.rs: should_compact -> cfg.threshold(limit)
- cargo test -p xode-core: 14 passed`;

const STATE = `goal: compaction trigger respects threshold_tokens + reserve
done: found duplicate threshold math in compaction.rs
now: about to edit should_compact
next: edit compaction.rs; run cargo test -p xode-core
notes: Config::threshold in config.rs:412 already handles reserve`;

export function createMockBackend(): Backend {
  let config: Config = defaultConfig();
  config.gateways = structuredClone(GATEWAYS);
  config.selected = { gateway: "gw-llama", model: "qwen27b", mode: "normal" };
  config.permissions.tools.shell = "ask";
  config.mcp = [
    {
      name: "github",
      transport: "stdio",
      command: "npx",
      args: ["-y", "@modelcontextprotocol/server-github"],
      env: { GITHUB_TOKEN: "ghp_xxx" },
      url: "",
      headers: {},
      enabled: true,
      disabled_tools: [],
    },
  ];

  const projects: Project[] = [
    { id: "p-xode", name: "xode", root: ROOT, extra_roots: [], created_at: now(), last_used: now() },
    { id: "p-web", name: "webshop", root: "/Users/dev/webshop", extra_roots: [], created_at: now(), last_used: now() - 1e6 },
    { id: "p-dot", name: "dotfiles", root: "/Users/dev/dotfiles", extra_roots: [], created_at: now(), last_used: now() - 2e6 },
  ];

  const mkSession = (id: string, project_id: string, title: string, ago: number): SessionInfo => ({
    id,
    project_id,
    title,
    created_at: now() - ago,
    updated_at: now() - ago,
    goal: null,
    mode: "normal",
    model: "qwen27b",
    gateway: "gw-llama",
    segment: 0,
    state: "",
    total_work_ms: 0,
    tokens_in: 0,
    tokens_out: 0,
    tool_calls: 0,
    compactions: 0,
    cwd: null,
  });

  const sessions: SessionInfo[] = [
    mkSession("s-demo", "p-xode", "Compaction threshold respects reserve", 60_000),
    mkSession("s-2", "p-xode", "Add RTK filter for cargo output", 3_600_000),
    mkSession("s-3", "p-xode", "Tree-sitter repo map for TypeScript", 7_200_000),
    mkSession("s-4", "p-xode", "Fix Windows path normalization in glob", 86_400_000),
    mkSession("s-5", "p-xode", "Browser tool: attach to existing Edge", 2 * 86_400_000),
    mkSession("s-6", "p-xode", "MCP stdio transport handshake", 3 * 86_400_000),
    mkSession("s-7", "p-xode", "Settings page for gateways", 4 * 86_400_000),
    mkSession("s-8", "p-web", "Checkout form validation", 5_000_000),
    mkSession("s-9", "p-web", "Migrate to Vite 6", 9_000_000),
  ];
  const messages = new Map<string, Message[]>();
  messages.set("s-demo", historyFor("s-demo"));

  const segments = new Map<string, Segment[]>();
  segments.set("s-demo", [
    { session_id: "s-demo", index: 1, path: PATH_MD, state: STATE, tokens_before: 63120, tokens_after: 6420, created_at: now() - 1000 * 60 * 40 },
  ]);

  const stats = new Map<string, LiveStats>();
  const baseStats = (): LiveStats => ({
    tps: 0,
    ttft_ms: 0,
    prefill_tps: 0,
    tokens_in: 0,
    tokens_out: 0,
    context_used: 0,
    context_limit: 76800,
    compactions: 0,
    steps: 0,
    tool_calls: 0,
    elapsed_ms: 0,
    total_work_ms: 0,
    rtk_saved: 0,
    model: "qwen27b",
    gateway: "llama-server",
  });
  stats.set("s-demo", {
    ...baseStats(),
    tps: 39.6,
    ttft_ms: 512,
    prefill_tps: 1942,
    tokens_in: 49170,
    tokens_out: 996,
    context_used: 11722,
    compactions: 1,
    steps: 4,
    tool_calls: 4,
    elapsed_ms: 26400,
    total_work_ms: 26400,
    rtk_saved: 3810,
  });

  const listeners = new Set<(e: AgentEvent) => void>();
  const emit = (e: AgentEvent) => listeners.forEach((l) => l(e));
  const queues = new Map<string, { id: string; text: string }[]>();
  const running = new Map<string, { cancelled: boolean }>();
  const pendingPerms = new Map<string, (d: PermDecision) => void>();

  const push = (sid: string, msg: Message) => {
    if (!messages.has(sid)) messages.set(sid, []);
    messages.get(sid)!.push(msg);
  };

  async function stream(sid: string, id: string, kind: "text" | "thinking", text: string, ctl: { cancelled: boolean }) {
    let i = 0;
    while (i < text.length && !ctl.cancelled) {
      const n = 2 + Math.floor(Math.random() * 6);
      const chunk = text.slice(i, i + n);
      i += n;
      emit(kind === "text" ? { type: "text_delta", session: sid, id, text: chunk } : { type: "thinking_delta", session: sid, id, text: chunk });
      const st = stats.get(sid)!;
      st.tokens_out += 1;
      await sleep(22 + Math.random() * 20);
    }
  }

  async function run(sid: string, userText: string) {
    const ctl = { cancelled: false };
    running.set(sid, ctl);
    const st = stats.get(sid) ?? baseStats();
    stats.set(sid, st);
    const startAt = now();
    st.elapsed_ms = 0;
    const tpsWalk = { v: 36 };
    const ticker = setInterval(() => {
      tpsWalk.v = Math.max(18, Math.min(62, tpsWalk.v + (Math.random() - 0.5) * 6));
      st.tps = Math.round(tpsWalk.v * 10) / 10;
      st.elapsed_ms = now() - startAt;
      st.total_work_ms += 250;
      emit({ type: "stats", session: sid, stats: { ...st } });
    }, 250);

    emit({ type: "state", session: sid, running: true });
    const user = mkMsg("user", [{ type: "text", text: userText }]);
    push(sid, user);
    emit({ type: "message", session: sid, message: user });
    const sess = sessions.find((s) => s.id === sid);
    if (sess && (!sess.title || sess.title === "New chat")) sess.title = userText.slice(0, 60);

    type Step = { think?: string; text?: string; calls?: { name: string; args: unknown; result: string; ms: number; ask?: string }[] };
    const steps: Step[] = [
      {
        think: THINK_1,
        text: "Let me check how the threshold is computed now.",
        calls: [
          {
            name: "kb",
            args: { action: "search", q: "compaction threshold reserve" },
            result:
              "p1 Compaction · L1-6 · 90/140 tok\n  PATH.md + STATE handoff, working set of touched files…\np4 Turso for the index · L1-3 · 60/60 tok\n  Moved the code index and the KB to Turso: vectors + FTS…",
            ms: 34,
          },
          { name: "read", args: { path: "crates/xode-core/src/compaction.rs", offset: 1, limit: 80 }, result: READ_RESULT, ms: 18 },
          { name: "grep", args: { pattern: "threshold", path: "crates" }, result: GREP_RESULT, ms: 41 },
          { name: "glob", args: { pattern: "crates/*/src/**/*.rs" }, result: FILES.filter((f) => f.endsWith(".rs")).join("\n"), ms: 9 },
        ],
      },
      { think: THINK_2, calls: [{ name: "edit", args: { path: "crates/xode-core/src/compaction.rs", old: "let t = ...", new: "let t = cfg.threshold(limit);" }, result: EDIT_RESULT, ms: 12 }] },
      {
        calls: [
          { name: "shell", args: { command: "cargo test -p xode-core" }, result: SHELL_RESULT, ms: 6840 },
          { name: "shell", args: { command: "git commit -am \"compaction: respect threshold\"" }, result: "[main 3f2a91c] compaction: respect threshold\n 1 file changed, 6 insertions(+), 2 deletions(-)", ms: 140, ask: "git commit -am \"compaction: respect threshold\"" },
        ],
      },
      { think: THINK_3, text: FINAL_TEXT },
    ];

    try {
      await sleep(350);
      for (let si = 0; si < steps.length && !ctl.cancelled; si++) {
        const step = steps[si];
        const id = uid("turn");
        const t0 = now();
        emit({ type: "turn_start", session: sid, id });
        st.steps += 1;
        st.ttft_ms = 380 + Math.round(Math.random() * 400);
        st.prefill_tps = 1800 + Math.round(Math.random() * 400);
        st.tokens_in += 9000 + Math.round(Math.random() * 3000);
        st.context_used = Math.min(st.context_limit, st.context_used + 4200);
        await sleep(st.ttft_ms);
        const parts: Part[] = [];
        if (step.think) {
          await stream(sid, id, "thinking", step.think, ctl);
          parts.push({ type: "thinking", text: step.think });
        }
        if (step.text) {
          await stream(sid, id, "text", step.text, ctl);
          parts.push({ type: "text", text: step.text });
        }
        const calls = (step.calls ?? []).map((c) => ({ ...c, id: uid("call") }));
        for (const c of calls) {
          if (ctl.cancelled) break;
          emit({ type: "tool_call_start", session: sid, id, call_id: c.id, name: c.name });
          await sleep(160);
          emit({ type: "tool_call_args", session: sid, id, call_id: c.id, args: c.args });
          parts.push({ type: "tool_call", id: c.id, name: c.name, args: c.args });
        }
        const m = mkMsg("assistant", parts, { meta: meta(now() - t0, st.tokens_in, 200, st.tps) });
        push(sid, m);
        emit({ type: "turn_end", session: sid, message: m, meta: m.meta! });
        const results: Part[] = [];
        for (const c of calls) {
          if (ctl.cancelled) break;
          let denied = false;
          if (c.ask) {
            const req_id = uid("perm");
            emit({ type: "permission_ask", session: sid, req_id, tool: c.name, summary: c.ask });
            const d = await new Promise<PermDecision>((r) => pendingPerms.set(req_id, r));
            denied = d === "deny";
          }
          await sleep(Math.min(c.ms, 2400));
          st.tool_calls += 1;
          st.rtk_saved += 120 + Math.round(Math.random() * 400);
          const content = denied ? "denied by user" : c.result;
          emit({ type: "tool_result", session: sid, call_id: c.id, name: c.name, content, is_error: denied, ms: c.ms });
          results.push({ type: "tool_result", id: c.id, name: c.name, content, is_error: denied });
        }
        if (results.length) push(sid, mkMsg("tool", results));
      }
      if (!ctl.cancelled) emit({ type: "goal_check", session: sid, done: true, reason: "tests pass" });
    } finally {
      clearInterval(ticker);
      st.tps = 0;
      emit({ type: "stats", session: sid, stats: { ...st } });
      running.delete(sid);
      emit({ type: "state", session: sid, running: false });
    }
  }

  const COMMANDS: CommandInfo[] = [
    { name: "compact", args: "", description: "Compact context now", source: "builtin" },
    { name: "goal", args: "<text>", description: "Set a goal the agent must reach", source: "builtin" },
    { name: "plan", args: "", description: "Switch to plan mode", source: "builtin" },
    { name: "model", args: "<id>", description: "Switch model", source: "builtin" },
    { name: "context", args: "", description: "Show context usage", source: "builtin" },
    { name: "export", args: "", description: "Export chat as markdown", source: "builtin" },
    { name: "settings", args: "", description: "Open settings", source: "builtin" },
    { name: "clear", args: "", description: "Start a new chat", source: "builtin" },
    { name: "review", args: "", description: "Review uncommitted changes", source: "custom" },
  ];

  const sectionContent: Record<string, string> = {
    system: "You are Xode, a coding agent working in the user's repository.\nBe terse. Use tools. Paths use '/'.\n...",
    tools: JSON.stringify([{ name: "read", parameters: { path: "string", offset: "int", limit: "int" } }, { name: "edit" }, { name: "shell" }], null, 2),
    brief: "Project: xode (Rust workspace)\nOS: Windows 10\nShell: pwsh",
    path: PATH_MD,
    state: STATE,
    working_set: "crates/xode-core/src/compaction.rs (edited)\ncrates/xode-core/src/config.rs (read 400-440)",
    messages: "user: Make the compaction trigger respect threshold_tokens ...\nassistant: Let me check how the threshold is computed now.",
    tool_results: SHELL_RESULT,
    thinking: THINK_3,
    attachments: "",
  };

  const backend: Backend = {
    config: async () => structuredClone(config),
    setConfig: async (cfg) => {
      config = structuredClone(cfg);
    },
    projects: async () => structuredClone(projects),
    addProject: async (root) => {
      const name = root.split(/[\\/]/).filter(Boolean).pop() || root;
      const p: Project = { id: uid("p"), name, root, extra_roots: [], created_at: now(), last_used: now() };
      projects.unshift(p);
      return p;
    },
    updateProject: async (p) => {
      const i = projects.findIndex((x) => x.id === p.id);
      if (i >= 0) projects[i] = structuredClone(p);
    },
    removeProject: async (id) => {
      const i = projects.findIndex((x) => x.id === id);
      if (i >= 0) projects.splice(i, 1);
    },
    sessions: async (pid) => structuredClone(sessions.filter((s) => !pid || s.project_id === pid).sort((a, b) => b.updated_at - a.updated_at)),
    newSession: async (pid) => {
      const s = mkSession(uid("s"), pid, "New chat", 0);
      s.kb_off = ["g:2"];
      sessions.unshift(s);
      return structuredClone(s);
    },
    session: async (id) => {
      const s = sessions.find((x) => x.id === id);
      if (!s) throw new Error("no such session");
      return structuredClone(s);
    },
    renameSession: async (id, title) => {
      const s = sessions.find((x) => x.id === id);
      if (s) s.title = title;
    },
    deleteSession: async (id) => {
      const i = sessions.findIndex((x) => x.id === id);
      if (i >= 0) sessions.splice(i, 1);
    },
    messages: async (sid) => structuredClone(messages.get(sid) ?? []),
    send: async (sid, text) => {
      if (running.has(sid)) {
        const q = queues.get(sid) ?? [];
        q.push({ id: `q-${Date.now()}`, text });
        queues.set(sid, q);
        emit({ type: "queue", session: sid, items: [...q] });
        return;
      }
      const s = sessions.find((x) => x.id === sid);
      if (s) s.updated_at = now();
      void run(sid, text);
    },
    steer: async (sid) => {
      const q = queues.get(sid) ?? [];
      queues.set(sid, []);
      for (const m of q) emit({ type: "message", session: sid, message: { id: m.id, role: "user", parts: [{ type: "text", text: m.text }], kind: "normal", segment: 0, meta: null, created_at: now() } as any });
      emit({ type: "queue", session: sid, items: [] });
    },
    unqueue: async (sid, id) => {
      const q = (queues.get(sid) ?? []).filter((m) => m.id !== id);
      queues.set(sid, q);
      emit({ type: "queue", session: sid, items: [...q] });
    },
    queued: async (sid) => queues.get(sid) ?? [],
    rewind: async (sid, id) => {
      const list = messages.get(sid) ?? [];
      const i = list.findIndex((m) => m.id === id);
      if (i < 0) throw new Error("message not found");
      const user = list[i].role === "user";
      const text = user ? (list[i].parts.find((p: any) => p.type === "text") as any)?.text ?? "" : "";
      const keep = user ? i : i + 1;
      const removed = list.length - keep;
      messages.set(sid, list.slice(0, keep));
      return { text, removed, files: 0 };
    },
    cancel: async (sid) => {
      const r = running.get(sid);
      if (r) r.cancelled = true;
      pendingPerms.forEach((res) => res("deny"));
      pendingPerms.clear();
    },
    isRunning: async (sid) => running.has(sid),
    command: async (sid, line) => {
      const [name] = line.slice(1).split(/\s+/);
      if (name === "settings") return { type: "open", panel: "settings" };
      if (name === "context") return { type: "open", panel: "context" };
      if (name === "export") return { type: "export", markdown: "# Export\n", path: "chat.md" };
      if (name === "compact") {
        const st = stats.get(sid) ?? baseStats();
        const used = Math.max(st.context_used, 38400);
        emit({ type: "compaction_start", session: sid, used, limit: 76800 });
        emit({ type: "compaction_progress", session: sid, stage: "prefill", tokens: 0, expected: 420 });
        await sleep(700);
        for (let t = 0; t <= 420; t += 30) {
          emit({ type: "compaction_progress", session: sid, stage: "handoff", tokens: t, expected: 420 });
          await sleep(90);
        }
        emit({ type: "compaction_progress", session: sid, stage: "path", tokens: 420, expected: 420 });
        await sleep(350);
        emit({ type: "compaction_progress", session: sid, stage: "seed", tokens: 420, expected: 420 });
        await sleep(350);
        const seg = (segments.get(sid)?.length ?? 0) + 1;
        segments.set(sid, [...(segments.get(sid) ?? []), { session_id: sid, index: seg, path: PATH_MD, state: STATE, tokens_before: used, tokens_after: 5200, created_at: now() }]);
        st.compactions += 1;
        st.context_used = 5200;
        stats.set(sid, st);
        emit({ type: "stats", session: sid, stats: { ...st } });
        emit({ type: "compaction_done", session: sid, segment: seg, path: PATH_MD, state: STATE, before: used, after: 5200 });
        return { type: "done" };
      }
      return { type: "notice", text: `/${name}: ok` };
    },
    commands: async () => COMMANDS,
    setMode: async (sid, mode) => {
      const s = sessions.find((x) => x.id === sid);
      if (s) s.mode = mode;
    },
    setModel: async (sid, gw, model) => {
      const s = sessions.find((x) => x.id === sid);
      if (s) Object.assign(s, { gateway: gw, model });
    },
    contextView: async (sid): Promise<ContextView> => {
      const used = stats.get(sid)?.context_used || 11722;
      const weights: [string, string, number][] = [
        ["system", "System", 0.1],
        ["tools", "Tools", 0.14],
        ["brief", "Brief", 0.03],
        ["path", "PATH", 0.05],
        ["state", "State", 0.03],
        ["working_set", "Working set", 0.07],
        ["messages", "Messages", 0.22],
        ["tool_results", "Tool results", 0.3],
        ["thinking", "Thinking", 0.06],
        ["attachments", "Attachments", 0],
      ];
      return {
        used,
        limit: 76800,
        threshold: 62976,
        sections: weights.map(([key, label, w]) => ({
          key,
          label,
          tokens: Math.round(used * w),
          content: sectionContent[key] ?? "",
          children:
            key === "tool_results"
              ? ([["shell", 0.55, 3], ["read", 0.3, 4], ["grep", 0.1, 2], ["edit", 0.05, 1]] as [string, number, number][]).map(([n, f, c]) => ({
                  key: `tool_results:${n}`,
                  label: `${n} ×${c}`,
                  tokens: Math.round(used * w * f),
                  content: n === "shell" ? SHELL_RESULT : "",
                }))
              : undefined,
        })),
        segments: structuredClone(segments.get(sid) ?? []),
        path_md: PATH_MD,
      };
    },
    stats: async (sid) => ({ ...(stats.get(sid) ?? baseStats()) }),
    permissionReply: async (req, d) => {
      pendingPerms.get(req)?.(d);
      pendingPerms.delete(req);
    },
    detectGateway: async (url, key) => {
      await sleep(600);
      return { id: uid("gw"), name: new URL(url).host, url, kind: "openai", api_key: key, flavor: "vllm", models: [{ id: "Qwen3-32B", context: 40960, vision: false }], enabled: true };
    },
    testGateway: async (g) => {
      await sleep(400);
      return { ok: true, latency_ms: 38, models: g.models.length, message: "ok" };
    },
    scanGateways: async () => {
      await sleep(1200);
      return [
        { id: uid("gw"), name: "LM Studio", url: "http://localhost:1234", kind: "openai", api_key: "", flavor: "lmstudio", models: [{ id: "qwen2.5-coder-14b", context: 32768, vision: false }], enabled: true },
      ];
    },
    refreshModels: async (id) => {
      await sleep(400);
      return structuredClone(config.gateways.find((g) => g.id === id)!);
    },
    listDir: async (path): Promise<FileEntry[]> => {
      const rel = path.replace(ROOT, "").replace(/^\/+/, "");
      const prefix = rel ? rel + "/" : "";
      const seen = new Map<string, FileEntry>();
      for (const f of FILES) {
        if (!f.startsWith(prefix)) continue;
        const rest = f.slice(prefix.length);
        const [head, ...tail] = rest.split("/");
        const isDir = tail.length > 0;
        if (!seen.has(head)) seen.set(head, { name: head, path: `${ROOT}/${prefix}${head}`, is_dir: isDir, size: isDir ? 0 : 2048 });
      }
      return [...seen.values()].sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.name.localeCompare(b.name));
    },
    completePath: async (_pid, prefix) => FILES.filter((f) => f.toLowerCase().includes(prefix.toLowerCase())).slice(0, 12),
    browserTest: async () => {
      await sleep(500);
      return { ok: true, latency_ms: 120, models: 0, message: "Edge 128 on :9222" };
    },
    mcpTest: async () => {
      await sleep(700);
      return ["create_issue", "list_issues", "get_pull_request", "search_code", "create_pull_request"];
    },
    mcpStatus: async () => [{ name: "github", connected: true, tools: 5, error: null }],
    indexStats: async () => ({ files: 214, symbols: 3810, ready: true }),
    ...mockKb(emit),
    setSessionKb: async (sid, off) => {
      const s = sessions.find((x) => x.id === sid);
      if (s) s.kb_off = [...off];
    },
    onEvent: (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
  return backend;
}
