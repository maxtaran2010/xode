import { createResource, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { ChevronRight, RefreshCw, X } from "lucide-solid";
import { api } from "../lib/api";
import { fmtTokens } from "../lib/format";
import { setState, state } from "../lib/store";
import type { ContextView as CV, Segment } from "../lib/types";
import { Collapse, Switcher } from "../lib/motion";
import { Segmented } from "./ui";

export const SECTION_COLORS: Record<string, string> = {
  system: "#6f7fa8",
  tools: "#8a6fd1",
  brief: "#4fa3c7",
  path: "#3fb68b",
  state: "#76c26b",
  working_set: "#d0a24a",
  messages: "#4a8dff",
  tool_results: "#e07b53",
  thinking: "#9aa6c7",
  attachments: "#d46fa3",
};
const color = (k: string) => SECTION_COLORS[k] ?? "#7d8595";

function SegmentRow(props: { seg: Segment }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class="seg-item" classList={{ open: open() }}>
      <button class="seg-row" onClick={() => setOpen(!open())}>
        <ChevronRight class="chev" size={14} stroke-width={1.6} />
        <span class="seg-idx">Segment {props.seg.index}</span>
        <span class="seg-tokens">
          {fmtTokens(props.seg.tokens_before)} → {fmtTokens(props.seg.tokens_after)}
        </span>
        <span class="spacer" />
        <span class="seg-time">{new Date(props.seg.created_at).toLocaleString()}</span>
      </button>
      <Collapse open={open()}>
        <div class="seg-body">
          <div class="kv-label">PATH</div>
          <pre class="tool-pre">{props.seg.path || "—"}</pre>
          <div class="kv-label">STATE</div>
          <pre class="tool-pre">{props.seg.state || "—"}</pre>
        </div>
      </Collapse>
    </div>
  );
}

export default function ContextView() {
  const [tab, setTab] = createSignal<"context" | "history">("context");
  const [selected, setSelected] = createSignal<string | null>(null);
  const [view, { refetch }] = createResource<CV | null, string | null>(
    () => state.activeSession,
    (sid) => (sid ? api.contextView(sid) : Promise.resolve(null)),
  );
  const close = () => setState("ui", "overlay", null);
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  const v = () => view() ?? null;
  const sections = () => v()?.sections ?? [];
  const flat = () => sections().flatMap((s) => [s, ...(s.children ?? [])]);
  const current = () => flat().find((s) => s.key === (selected() ?? sections()[0]?.key));
  const pct = (n: number) => (v() && v()!.limit ? (n / v()!.limit) * 100 : 0);

  return (
    <div class="overlay">
      <div class="ov-head">
        <Segmented value={tab()} options={[{ value: "context", label: "Context" }, { value: "history", label: "History" }]} onChange={setTab} />
        <span class="spacer" />
        <button class="icon-btn" onClick={() => refetch()} aria-label="Refresh">
          <RefreshCw size={15} stroke-width={1.6} />
        </button>
        <button class="icon-btn" onClick={close} aria-label="Close">
          <X size={16} stroke-width={1.6} />
        </button>
      </div>
      <Show when={v()} fallback={<div class="ov-empty">{state.activeSession ? "" : "No chat selected"}</div>}>
        {(cv) => (
          <Switcher key={tab()}>
          <Show
            when={tab() === "context"}
            fallback={
              <div class="ctx-history">
                <div class="ctx-h-col">
                  <div class="kv-label">Segments</div>
                  <Show when={cv().segments.length} fallback={<div class="muted small">No compactions yet</div>}>
                    <For each={cv().segments}>{(s) => <SegmentRow seg={s} />}</For>
                  </Show>
                </div>
                <div class="ctx-h-col">
                  <div class="kv-label">PATH.md</div>
                  <pre class="ctx-content">{cv().path_md || "—"}</pre>
                </div>
              </div>
            }
          >
            <div class="ctx-summary">
              <div class="ctx-numbers">
                <span class="ctx-used">{fmtTokens(cv().used)}</span>
                <span class="ctx-limit">/ {fmtTokens(cv().limit)}</span>
                <span class="ctx-pct">{pct(cv().used).toFixed(1)}%</span>
                <span class="spacer" />
                <span class="ctx-threshold-label">Compaction at {fmtTokens(cv().threshold)}</span>
              </div>
              <div class="ctx-bar">
                <For each={sections().filter((s) => s.tokens > 0)}>
                  {(s) => (
                    <div
                      class="ctx-bar-seg"
                      classList={{ dim: !!selected() && selected() !== s.key }}
                      style={{ width: `${pct(s.tokens)}%`, background: color(s.key) }}
                      onClick={() => setSelected(s.key)}
                      data-tip={`${s.label} ${fmtTokens(s.tokens)}`}
                    />
                  )}
                </For>
                <div class="ctx-threshold" style={{ left: `${pct(cv().threshold)}%` }} />
              </div>
              <div class="ctx-legend">
                <For each={sections()}>
                  {(s) => (
                    <span class="legend-item">
                      <i style={{ background: color(s.key) }} />
                      {s.label}
                      <b>{fmtTokens(s.tokens)}</b>
                    </span>
                  )}
                </For>
              </div>
            </div>
            <div class="ctx-split">
              <div class="ctx-list">
                <For each={sections()}>
                  {(s) => (
                    <>
                      <button class="ctx-list-row" classList={{ active: current()?.key === s.key }} onClick={() => setSelected(s.key)}>
                        <i style={{ background: color(s.key) }} />
                        <span class="ctx-list-label">{s.label}</span>
                        <span class="ctx-list-tokens">{fmtTokens(s.tokens)}</span>
                        <span class="ctx-list-pct">{cv().used ? ((s.tokens / cv().used) * 100).toFixed(0) : 0}%</span>
                      </button>
                      <For each={s.children ?? []}>
                        {(c) => (
                          <button class="ctx-list-row sub" classList={{ active: current()?.key === c.key }} onClick={() => setSelected(c.key)}>
                            <span class="ctx-sub-bar">
                              <span style={{ width: `${s.tokens ? Math.max(4, (c.tokens / s.tokens) * 100) : 0}%`, background: color(s.key) }} />
                            </span>
                            <span class="ctx-list-label mono">{c.label}</span>
                            <span class="ctx-list-tokens">{fmtTokens(c.tokens)}</span>
                            <span class="ctx-list-pct">{cv().used ? ((c.tokens / cv().used) * 100).toFixed(0) : 0}%</span>
                          </button>
                        )}
                      </For>
                    </>
                  )}
                </For>
              </div>
              <Switcher key={current()?.key} motion="fade">
                <pre class="ctx-content">{current()?.content || ""}</pre>
              </Switcher>
            </div>
          </Show>
          </Switcher>
        )}
      </Show>
    </div>
  );
}
