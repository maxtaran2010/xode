import { onCleanup, onMount, Show } from "solid-js";
import { X } from "lucide-solid";
import { Switcher } from "../../lib/motion";
import { loadKb, setKbTab, setState, state, type KbTab } from "../../lib/store";
import { Segmented, Spinner } from "../ui";
import Graph from "./Graph";
import Notes from "./Notes";
import Sources from "./Sources";

export default function Knowledge() {
  const close = () => setState("ui", "overlay", null);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) close();
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    loadKb();
  });
  onCleanup(() => window.removeEventListener("keydown", onKey));
  const busy = () => Object.values(state.kb.progress).some((p) => p.stage !== "idle");
  return (
    <div class="overlay kb">
      <div class="ov-head">
        <Segmented
          value={state.kb.tab}
          options={[
            { value: "sources", label: "Sources" },
            { value: "notes", label: "Notes" },
            { value: "graph", label: "Graph" },
          ]}
          onChange={(v) => setKbTab(v as KbTab)}
        />
        <Show when={busy()}>
          <span class="kb-busy-dot">
            <Spinner size={13} />
          </span>
        </Show>
        <span class="spacer" />
        <button class="icon-btn" onClick={close} aria-label="Close">
          <X size={16} stroke-width={1.6} />
        </button>
      </div>
      <Show when={state.kb.overview} fallback={<div class="ov-empty">{state.config?.knowledge.enabled ? "" : "Knowledge base is off"}</div>}>
        <Switcher key={state.kb.tab} class="kb-body">
          <Show when={state.kb.tab === "sources"}>
            <Sources />
          </Show>
          <Show when={state.kb.tab === "notes"}>
            <Notes />
          </Show>
          <Show when={state.kb.tab === "graph"}>
            <Graph />
          </Show>
        </Switcher>
      </Show>
    </div>
  );
}
