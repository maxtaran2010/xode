import { createSignal, For, Match, Show, Switch } from "solid-js";
import { ChevronRight, CircleAlert, ShieldAlert } from "lucide-solid";
import { fmtMs, fmtTokens, fmtTps } from "../lib/format";
import type { Block, Item, ToolBlock } from "../lib/session";
import { replyPermission } from "../lib/store";
import { toolLabel } from "../lib/tools";
import { Collapse } from "../lib/motion";
import Markdown from "./Markdown";
import Thinking from "./Thinking";
import ToolCall from "./ToolCall";

type Of<K extends Item["kind"]> = Extract<Item, { kind: K }>;

function UserMessage(props: { item: Of<"user"> }) {
  return (
    <div class="msg-user" classList={{ pending: !!props.item.pending }}>
      <div class="bubble">
        <Show when={props.item.images.length}>
          <div class="bubble-images">
            <For each={props.item.images}>{(img) => <img src={`data:${img.mime};base64,${img.data}`} alt="" />}</For>
          </div>
        </Show>
        <Show when={props.item.text}>
          <div class="bubble-text">{props.item.text}</div>
        </Show>
      </div>
    </div>
  );
}

function TurnMetaLine(props: { item: Of<"assistant">; always: boolean }) {
  const m = () => props.item.meta;
  return (
    <Show when={m()}>
      {(meta) => (
        <div class="turn-meta" classList={{ always: props.always }}>
          <span>{fmtMs(meta().duration_ms)}</span>
          <span>
            {fmtTokens(meta().prompt_tokens)} in · {fmtTokens(meta().completion_tokens)} out
          </span>
          <span>{fmtTps(meta().decode_tps)} tok/s</span>
          <span>TTFT {fmtMs(meta().ttft_ms)}</span>
        </div>
      )}
    </Show>
  );
}

function AssistantTurn(props: { item: Of<"assistant">; last: boolean }) {
  return (
    <div class="turn">
      <For each={props.item.blocks}>
        {(b) => (
          <Switch>
            <Match when={b.kind === "tool" && (b as ToolBlock)}>{(t) => <ToolCall block={t()} />}</Match>
            <Match when={b.kind === "thinking" && (b as Extract<Block, { kind: "thinking" }>)}>{(t) => <Thinking block={t()} />}</Match>
            <Match when={b.kind === "text" && (b as Extract<Block, { kind: "text" }>)}>{(t) => <Markdown text={t().text} />}</Match>
          </Switch>
        )}
      </For>
      <Show when={!props.item.streaming}>
        <TurnMetaLine item={props.item} always={props.last} />
      </Show>
    </div>
  );
}

const STAGES: Record<string, string> = {
  prefill: "Reading context",
  handoff: "Writing handoff",
  path: "Updating PATH.md",
  seed: "Building fresh context",
};

/** Overall compaction progress 0..1 from the current stage. */
export function compactionRatio(stage: string | undefined, tokens = 0, expected = 0): number {
  switch (stage) {
    case "prefill":
      return 0.06;
    case "handoff":
      return 0.1 + 0.75 * Math.min(1, tokens / Math.max(1, expected));
    case "path":
      return 0.9;
    case "seed":
      return 0.97;
    default:
      return 0.03;
  }
}

function CompactionDivider(props: { item: Of<"compaction"> }) {
  const [open, setOpen] = createSignal(false);
  const stageLabel = () => {
    const i = props.item;
    const base = STAGES[i.stage ?? ""] ?? "Compacting context";
    return i.stage === "handoff" && i.tokens ? `${base} · ${i.tokens} tokens` : base;
  };
  const label = () =>
    props.item.running
      ? stageLabel()
      : props.item.before
        ? `Context compacted · ${fmtTokens(props.item.before)} → ${fmtTokens(props.item.after)}`
        : "Context compacted";
  return (
    <div class="divider-wrap" classList={{ open: open() }}>
      <button class="divider" onClick={() => !props.item.running && setOpen(!open())}>
        <span class="divider-line" />
        <span class="divider-label" classList={{ shimmer: props.item.running }}>
          {label()}
          <Show when={!props.item.running}>
            <ChevronRight class="chev" size={12} stroke-width={1.75} />
          </Show>
        </span>
        <span class="divider-line" />
      </button>
      <Show when={props.item.running}>
        <div class="comp-progress">
          <div class="comp-progress-fill" style={{ transform: `scaleX(${compactionRatio(props.item.stage, props.item.tokens, props.item.expected)})` }} />
        </div>
      </Show>
      <Collapse open={open()}>
        <div class="divider-body">
          <Show when={props.item.path}>
            <div class="kv-label">PATH</div>
            <pre class="tool-pre">{props.item.path}</pre>
          </Show>
          <Show when={props.item.state}>
            <div class="kv-label">STATE</div>
            <pre class="tool-pre">{props.item.state}</pre>
          </Show>
        </div>
      </Collapse>
    </div>
  );
}

function GoalDivider(props: { item: Of<"goal"> }) {
  return (
    <div class="divider-wrap">
      <div class="divider static">
        <span class="divider-line" />
        <span class="divider-label">Goal not complete: {props.item.reason}</span>
        <span class="divider-line" />
      </div>
    </div>
  );
}

function PermissionCard(props: { item: Of<"perm">; session: string }) {
  const decided = () => props.item.decision;
  const word = () => (decided() === "deny" ? "Denied" : decided() === "always" ? "Always allowed" : "Allowed once");
  return (
    <Show
      when={!decided()}
      fallback={
        <div class="perm-done">
          {word()} · {toolLabel(props.item.tool)} · <span class="mono">{props.item.summary}</span>
        </div>
      }
    >
      <div class="perm-card">
        <div class="perm-head">
          <ShieldAlert size={15} stroke-width={1.6} />
          <span>
            Allow <b>{toolLabel(props.item.tool)}</b>
          </span>
        </div>
        <pre class="perm-summary">{props.item.summary}</pre>
        <div class="perm-actions">
          <button class="btn ghost" onClick={() => replyPermission(props.session, props.item.id, "deny")}>
            Deny
          </button>
          <button class="btn" onClick={() => replyPermission(props.session, props.item.id, "always")}>
            Always
          </button>
          <button class="btn primary" onClick={() => replyPermission(props.session, props.item.id, "once")}>
            Allow once
          </button>
        </div>
      </div>
    </Show>
  );
}

export default function ItemView(props: { item: Item; next?: Item; session: string; running: boolean }) {
  const lastOfRun = () => props.next?.kind !== "assistant" && !(props.running && !props.next);
  return (
    <Switch>
      <Match when={props.item.kind === "user" && (props.item as Of<"user">)}>{(i) => <UserMessage item={i()} />}</Match>
      <Match when={props.item.kind === "assistant" && (props.item as Of<"assistant">)}>{(i) => <AssistantTurn item={i()} last={lastOfRun()} />}</Match>
      <Match when={props.item.kind === "compaction" && (props.item as Of<"compaction">)}>{(i) => <CompactionDivider item={i()} />}</Match>
      <Match when={props.item.kind === "goal" && (props.item as Of<"goal">)}>{(i) => <GoalDivider item={i()} />}</Match>
      <Match when={props.item.kind === "perm" && (props.item as Of<"perm">)}>{(i) => <PermissionCard item={i()} session={props.session} />}</Match>
      <Match when={props.item.kind === "notice" && (props.item as Extract<Item, { kind: "notice" | "error" }>)}>{(i) => <div class="notice">{i().text}</div>}</Match>
      <Match when={props.item.kind === "error" && (props.item as Extract<Item, { kind: "notice" | "error" }>)}>
        {(i) => (
          <div class="notice error">
            <CircleAlert size={14} stroke-width={1.75} />
            <span>{i().text}</span>
          </div>
        )}
      </Match>
    </Switch>
  );
}
