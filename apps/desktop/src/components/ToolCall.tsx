import { createMemo, createSignal, For, Show } from "solid-js";
import { Dynamic } from "solid-js/web";
import { Check, X } from "lucide-solid";
import { fmtMs } from "../lib/format";
import type { ToolBlock } from "../lib/session";
import { isDiff, toolIcon, toolLabel, toolSummary } from "../lib/tools";
import { stickToBottom } from "../lib/autoscroll";
import { Collapse } from "../lib/motion";
import { Spinner } from "./ui";

function Diff(props: { text: string }) {
  const lines = createMemo(() => props.text.split("\n"));
  return (
    <pre class="tool-pre diff">
      <For each={lines()}>
        {(l) => {
          const cls = l.startsWith("+++") || l.startsWith("---") ? "d-file" : l.startsWith("@@") ? "d-hunk" : l.startsWith("+") ? "d-add" : l.startsWith("-") ? "d-del" : "";
          return <div class={cls}>{l || " "}</div>;
        }}
      </For>
    </pre>
  );
}

function argsText(args: unknown): string {
  if (args == null) return "";
  if (typeof args === "string") return args;
  return JSON.stringify(args, null, 2);
}

export default function ToolCall(props: { block: ToolBlock }) {
  const [open, setOpen] = createSignal(false);
  const b = () => props.block;
  const summary = () => toolSummary(b().name, b().args);
  const busy = () => b().status === "pending" || b().status === "running";
  return (
    <div class="tool" classList={{ open: open(), err: b().status === "error" }}>
      <button class="tool-row" onClick={() => setOpen(!open())}>
        <span class="tool-icon">
          <Dynamic component={toolIcon(b().name)} size={15} stroke-width={1.6} />
        </span>
        <span class="tool-name">{toolLabel(b().name)}</span>
        <span class="tool-arg">{summary()}</span>
        <span class="tool-end">
          <Show when={b().ms != null}>
            <span class="tool-ms">{fmtMs(b().ms!)}</span>
          </Show>
          <span class="tool-status">
            <Show when={busy()}>
              <Spinner size={13} />
            </Show>
            <Show when={b().status === "ok"}>
              <Check size={14} stroke-width={1.75} />
            </Show>
            <Show when={b().status === "error"}>
              <X size={14} stroke-width={1.75} />
            </Show>
          </span>
        </span>
      </button>
      <Collapse open={open()}>
        <div class="tool-body">
          <Show when={b().args != null}>
            <pre class="tool-pre args">{argsText(b().args)}</pre>
          </Show>
          <Show when={b().result != null}>
            <Show when={isDiff(b().result!)} fallback={<pre class="tool-pre" ref={(el) => stickToBottom(el)}>{b().result}</pre>}>
              <Diff text={b().result!} />
            </Show>
          </Show>
        </div>
      </Collapse>
    </div>
  );
}
