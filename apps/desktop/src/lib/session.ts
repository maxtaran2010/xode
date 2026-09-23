// Per-session live state built from history (engine.messages) + streamed AgentEvents.
import type { AgentEvent, LiveStats, Message, PermDecision, QueuedMsg, Segment, TurnMeta } from "./types";
import { toolActivity } from "./tools";

export type ToolStatus = "pending" | "running" | "ok" | "error" | "done";

export interface ToolBlock {
  kind: "tool";
  callId: string;
  name: string;
  args: unknown;
  status: ToolStatus;
  result: string | null;
  ms: number | null;
}

export interface ThinkingBlock {
  kind: "thinking";
  text: string;
  /** ms timestamps; 0 when loaded from history (duration unknown). */
  start: number;
  end: number | null;
}

export interface TextBlock {
  kind: "text";
  text: string;
}

export type Block = ToolBlock | ThinkingBlock | TextBlock;

export type Item =
  | { kind: "user"; id: string; text: string; images: { mime: string; data: string }[]; pending?: boolean }
  | { kind: "assistant"; id: string; blocks: Block[]; meta: TurnMeta | null; streaming: boolean }
  | {
      kind: "compaction";
      id: string;
      segment: number;
      before: number;
      after: number;
      path: string;
      state: string;
      running: boolean;
      /** Live progress while running: prefill | handoff | path | seed. */
      stage?: string;
      tokens?: number;
      expected?: number;
    }
  | { kind: "goal"; id: string; done: boolean; reason: string }
  | { kind: "perm"; id: string; tool: string; summary: string; decision: PermDecision | null }
  | { kind: "notice" | "error"; id: string; text: string };

export interface SessionLive {
  items: Item[];
  loaded: boolean;
  running: boolean;
  runStart: number | null;
  activity: string;
  steps: number;
  stats: LiveStats | null;
  tpsHistory: number[];
  /** Messages sent while running; delivered after the next tool call or on steer. */
  queue: QueuedMsg[];
}

export const emptyLive = (): SessionLive => ({
  items: [],
  loaded: false,
  running: false,
  runStart: null,
  activity: "",
  steps: 0,
  stats: null,
  tpsHistory: [],
  queue: [],
});

let n = 0;
const lid = (p: string) => `${p}-${++n}`;

function partsText(m: Message): string {
  return m.parts
    .filter((p) => p.type === "text")
    .map((p) => (p as { text: string }).text)
    .join("\n");
}

function assistantBlocks(m: Message): Block[] {
  const blocks: Block[] = [];
  for (const p of m.parts) {
    if (p.type === "thinking") blocks.push({ kind: "thinking", text: p.text, start: 0, end: 0 });
    else if (p.type === "text") {
      if (p.text.trim()) blocks.push({ kind: "text", text: p.text });
    } else if (p.type === "tool_call")
      blocks.push({ kind: "tool", callId: p.id, name: p.name, args: p.args, status: "running", result: null, ms: null });
  }
  return blocks;
}

/** Converts stored history into render items (tool results paired to their calls). */
export function messagesToItems(msgs: Message[], segments: Segment[], running: boolean): Item[] {
  const items: Item[] = [];
  const calls = new Map<string, ToolBlock>();
  const segs = new Map(segments.map((s) => [s.index, s]));
  for (const m of msgs) {
    if (m.kind === "compaction" || m.kind === "note" || m.role === "system") continue;
    if (m.kind === "seed") {
      const s = segs.get(m.segment);
      items.push({
        kind: "compaction",
        id: m.id,
        segment: m.segment,
        before: s?.tokens_before ?? 0,
        after: s?.tokens_after ?? 0,
        path: s?.path ?? "",
        state: s?.state ?? partsText(m),
        running: false,
      });
      continue;
    }
    if (m.kind === "goal") {
      items.push({ kind: "goal", id: m.id, done: false, reason: partsText(m) });
      continue;
    }
    if (m.role === "user") {
      const images = m.parts.filter((p) => p.type === "image") as { mime: string; data: string }[];
      items.push({ kind: "user", id: m.id, text: partsText(m), images: images.map((i) => ({ mime: i.mime, data: i.data })) });
    } else if (m.role === "assistant") {
      const blocks = assistantBlocks(m);
      for (const b of blocks) if (b.kind === "tool") calls.set(b.callId, b);
      items.push({ kind: "assistant", id: m.id, blocks, meta: m.meta, streaming: false });
    } else if (m.role === "tool") {
      for (const p of m.parts) {
        if (p.type !== "tool_result") continue;
        const c = calls.get(p.id);
        if (c) {
          c.result = p.content;
          c.status = p.is_error ? "error" : "ok";
        }
      }
    }
  }
  if (!running) for (const c of calls.values()) if (c.status === "running") c.status = "done";
  return items;
}

/** Mutating reducer, applied inside a Solid `produce`. Deltas are pre-batched by the caller. */
export function applyEvent(s: SessionLive, ev: AgentEvent): void {
  const findItem = (id: string) => {
    for (let i = s.items.length - 1; i >= 0; i--) if (s.items[i].id === id) return s.items[i];
    return undefined;
  };
  const turn = (id: string) => {
    let it = findItem(id);
    if (!it) {
      it = { kind: "assistant", id, blocks: [], meta: null, streaming: true };
      s.items.push(it);
    }
    return it as Extract<Item, { kind: "assistant" }>;
  };
  const findCall = (callId: string): ToolBlock | undefined => {
    for (let i = s.items.length - 1; i >= 0 && i >= s.items.length - 60; i--) {
      const it = s.items[i];
      if (it.kind !== "assistant") continue;
      for (const b of it.blocks) if (b.kind === "tool" && b.callId === callId) return b;
    }
    return undefined;
  };
  const endThinking = (t: Extract<Item, { kind: "assistant" }>) => {
    const last = t.blocks[t.blocks.length - 1];
    if (last?.kind === "thinking" && last.end === null) last.end = Date.now();
  };

  switch (ev.type) {
    case "queue":
      s.queue = ev.items;
      break;
    case "state":
      s.running = ev.running;
      if (ev.running) {
        s.runStart = Date.now();
        s.steps = 0;
        s.tpsHistory = [];
        s.activity = "Thinking…";
      } else {
        s.activity = "";
        for (const it of s.items) {
          if (it.kind === "assistant" && it.streaming) {
            it.streaming = false;
            endThinking(it);
          }
          if (it.kind === "compaction") it.running = false;
          if (it.kind === "assistant")
            for (const b of it.blocks) if (b.kind === "tool" && (b.status === "running" || b.status === "pending")) b.status = "done";
        }
      }
      break;
    case "message": {
      const m = ev.message;
      if (m.kind === "compaction" || m.kind === "note" || m.role === "system") break;
      if (m.kind === "seed") {
        const last = [...s.items].reverse().find((i) => i.kind === "compaction");
        if (last && last.kind === "compaction" && last.segment === m.segment) break;
        s.items.push(...messagesToItems([m], [], true));
        break;
      }
      if (m.kind === "goal") {
        if (s.items[s.items.length - 1]?.kind !== "goal") s.items.push({ kind: "goal", id: m.id, done: false, reason: partsText(m) });
        break;
      }
      if (m.role === "user") {
        s.queue = s.queue.filter((q) => q.id !== m.id);
        const pending = s.items.findIndex((i) => i.kind === "user" && i.pending);
        const images = (m.parts.filter((p) => p.type === "image") as { mime: string; data: string }[]).map((i) => ({ mime: i.mime, data: i.data }));
        const item: Item = { kind: "user", id: m.id, text: partsText(m), images };
        if (pending >= 0) s.items[pending] = item;
        else if (!findItem(m.id)) s.items.push(item);
      } else if (m.role === "tool") {
        for (const p of m.parts) {
          if (p.type !== "tool_result") continue;
          const c = findCall(p.id);
          if (c && c.result === null) {
            c.result = p.content;
            c.status = p.is_error ? "error" : "ok";
          }
        }
      } else if (m.role === "assistant" && !findItem(m.id)) {
        s.items.push({ kind: "assistant", id: m.id, blocks: assistantBlocks(m), meta: m.meta, streaming: false });
      }
      break;
    }
    case "turn_start":
      turn(ev.id);
      s.steps += 1;
      s.activity = "Thinking…";
      break;
    case "thinking_delta": {
      const t = turn(ev.id);
      const last = t.blocks[t.blocks.length - 1];
      if (last?.kind === "thinking" && last.end === null) last.text += ev.text;
      else t.blocks.push({ kind: "thinking", text: ev.text, start: Date.now(), end: null });
      s.activity = "Thinking…";
      break;
    }
    case "text_delta": {
      const t = turn(ev.id);
      endThinking(t);
      const last = t.blocks[t.blocks.length - 1];
      if (last?.kind === "text") last.text += ev.text;
      else t.blocks.push({ kind: "text", text: ev.text });
      s.activity = "Writing…";
      break;
    }
    case "tool_call_start": {
      const t = turn(ev.id);
      endThinking(t);
      t.blocks.push({ kind: "tool", callId: ev.call_id, name: ev.name, args: null, status: "pending", result: null, ms: null });
      s.activity = toolActivity(ev.name, null);
      break;
    }
    case "tool_call_args": {
      const c = findCall(ev.call_id);
      if (c) c.args = ev.args;
      s.activity = toolActivity(c?.name ?? "", ev.args);
      break;
    }
    case "turn_end": {
      const t = turn(findTurnId(s, ev));
      endThinking(t);
      t.streaming = false;
      t.meta = ev.meta;
      const final = assistantBlocks(ev.message);
      if (!t.blocks.length) t.blocks = final;
      for (const b of t.blocks) {
        if (b.kind !== "tool") continue;
        if (b.status === "pending") b.status = "running";
        if (b.args == null) {
          const f = final.find((x) => x.kind === "tool" && x.callId === b.callId) as ToolBlock | undefined;
          if (f) b.args = f.args;
        }
      }
      const firstRunning = t.blocks.find((b) => b.kind === "tool" && b.status === "running") as ToolBlock | undefined;
      if (firstRunning) s.activity = toolActivity(firstRunning.name, firstRunning.args);
      break;
    }
    case "tool_result": {
      const c = findCall(ev.call_id);
      if (c) {
        c.result = ev.content;
        c.status = ev.is_error ? "error" : "ok";
        c.ms = ev.ms;
      }
      const next = findRunningTool(s);
      s.activity = next ? toolActivity(next.name, next.args) : "Thinking…";
      break;
    }
    case "stats":
      s.stats = ev.stats;
      if (s.running && ev.stats.tps > 0) {
        s.tpsHistory.push(ev.stats.tps);
        if (s.tpsHistory.length > 160) s.tpsHistory.splice(0, s.tpsHistory.length - 160);
      }
      break;
    case "compaction_start":
      s.items.push({ kind: "compaction", id: lid("cmp"), segment: 0, before: ev.used, after: 0, path: "", state: "", running: true });
      s.activity = "Compacting context…";
      break;
    case "compaction_progress": {
      const last = [...s.items].reverse().find((i) => i.kind === "compaction" && i.running);
      if (last && last.kind === "compaction") Object.assign(last, { stage: ev.stage, tokens: ev.tokens, expected: ev.expected });
      else s.items.push({ kind: "compaction", id: lid("cmp"), segment: 0, before: 0, after: 0, path: "", state: "", running: true, stage: ev.stage, tokens: ev.tokens, expected: ev.expected });
      s.activity = { prefill: "Compacting · reading context", handoff: "Compacting · writing handoff", path: "Compacting · updating PATH.md", seed: "Compacting · fresh context" }[ev.stage] ?? "Compacting context…";
      break;
    }
    case "compaction_done": {
      const last = [...s.items].reverse().find((i) => i.kind === "compaction" && i.running);
      const data = { segment: ev.segment, before: ev.before, after: ev.after, path: ev.path, state: ev.state, running: false };
      if (last) Object.assign(last, data);
      else s.items.push({ kind: "compaction", id: lid("cmp"), ...data });
      s.activity = "Thinking…";
      break;
    }
    case "goal_check":
      if (!ev.done) s.items.push({ kind: "goal", id: lid("goal"), done: ev.done, reason: ev.reason });
      break;
    case "permission_ask":
      s.items.push({ kind: "perm", id: ev.req_id, tool: ev.tool, summary: ev.summary, decision: null });
      s.activity = "Waiting for approval…";
      break;
    case "notice":
      s.items.push({ kind: "notice", id: lid("notice"), text: ev.text });
      break;
    case "error":
      s.items.push({ kind: "error", id: lid("err"), text: ev.text });
      break;
  }
}

// The turn id used by deltas (TurnStart.id) may differ from the committed message id;
// fall back to the latest streaming assistant item.
function findTurnId(s: SessionLive, ev: Extract<AgentEvent, { type: "turn_end" }>): string {
  for (let i = s.items.length - 1; i >= 0; i--) {
    const it = s.items[i];
    if (it.id === ev.message.id) return it.id;
    if (it.kind === "assistant" && it.streaming) return it.id;
  }
  return ev.message.id;
}

function findRunningTool(s: SessionLive): ToolBlock | undefined {
  const it = s.items[s.items.length - 1];
  if (it?.kind !== "assistant") return undefined;
  return it.blocks.find((b) => b.kind === "tool" && b.status === "running") as ToolBlock | undefined;
}
