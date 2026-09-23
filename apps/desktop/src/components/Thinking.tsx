import { createSignal, Show } from "solid-js";
import { Brain, ChevronRight } from "lucide-solid";
import { estTokens, fmtTokens } from "../lib/format";
import type { ThinkingBlock } from "../lib/session";
import { stickToBottom } from "../lib/autoscroll";
import { Collapse } from "../lib/motion";
import { now } from "./ui";

export default function Thinking(props: { block: ThinkingBlock }) {
  const [open, setOpen] = createSignal(false);
  const live = () => props.block.end === null;
  const secs = () => {
    const b = props.block;
    if (!b.start) return null;
    const end = b.end ?? now();
    return Math.max(0, Math.round((end - b.start) / 1000));
  };
  const tokens = () => fmtTokens(estTokens(props.block.text));
  return (
    <div class="thinking" classList={{ open: open() }}>
      <button class="thinking-row" onClick={() => setOpen(!open())}>
        <Brain size={15} stroke-width={1.6} />
        <span class="thinking-label" classList={{ shimmer: live() }}>
          {live() ? "Thinking" : "Thought"}
        </span>
        <span class="thinking-meta">
          {tokens()} tokens
          <Show when={secs() !== null}> · {secs()}s</Show>
        </span>
        <ChevronRight class="chev" size={14} stroke-width={1.6} />
      </button>
      <Collapse open={open()}>
        <div class="thinking-body" ref={(el) => stickToBottom(el)}>
          {props.block.text}
        </div>
      </Collapse>
    </div>
  );
}
