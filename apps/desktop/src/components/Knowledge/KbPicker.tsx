// Composer control: switch knowledge-base layers / sources on or off for the current chat.
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { BookOpen } from "lucide-solid";
import { kbOff, loadKb, openKnowledge, setKbOff, state } from "../../lib/store";
import { Toggle } from "../Settings/controls";
import { LAYERS, sources } from "./common";

export default function KbPicker() {
  let btn!: HTMLButtonElement;
  let pop: HTMLDivElement | undefined;
  const [open, setOpen] = createSignal(false);
  const [pos, setPos] = createSignal({ left: 0, bottom: 0 });

  const off = () => kbOff();
  const enabled = () => sources().filter((s) => !off().includes(s.key) && !off().includes(s.layer));
  const toggle = (key: string) => setKbOff(off().includes(key) ? off().filter((k) => k !== key) : [...off(), key]);

  const show = () => {
    const r = btn.getBoundingClientRect();
    setPos({ left: Math.max(8, Math.min(r.left, window.innerWidth - 308)), bottom: window.innerHeight - r.top + 6 });
    setOpen(true);
    loadKb();
  };
  const onDown = (e: MouseEvent) => {
    if (open() && pop && !pop.contains(e.target as Node) && !btn.contains(e.target as Node)) setOpen(false);
  };
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
  onMount(() => {
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
  });
  onCleanup(() => {
    window.removeEventListener("mousedown", onDown, true);
    window.removeEventListener("keydown", onKey);
  });

  return (
    <Show when={state.config?.knowledge.enabled && sources().length}>
      <button
        ref={btn}
        class="model-btn kb-btn"
        classList={{ off: !enabled().length, active: open() }}
        onClick={() => (open() ? setOpen(false) : show())}
        aria-label="Knowledge"
      >
        <BookOpen size={14} stroke-width={1.6} />
        <Show when={enabled().length !== sources().length}>
          <span>
            {enabled().length}/{sources().length}
          </span>
        </Show>
      </button>
      <Show when={open()}>
        <Portal>
          <div ref={pop} class="kb-pop popup" style={{ left: `${pos().left}px`, bottom: `${pos().bottom}px` }}>
            <For each={LAYERS}>
              {(l) => {
                const list = () => sources().filter((s) => s.layer === l.id);
                const layerOn = () => !off().includes(l.id);
                return (
                  <Show when={list().length}>
                    <div class="kb-pop-layer">
                      <div class="kb-pop-head">
                        <span class="kb-pop-icon" style={{ color: l.color }}>
                          <l.icon size={14} stroke-width={1.6} />
                        </span>
                        <span class="kb-pop-label">{l.label}</span>
                        <Toggle value={layerOn()} onChange={() => toggle(l.id)} />
                      </div>
                      <Show when={list().length > 1 || !l.id.includes("memory")}>
                        <For each={list()}>
                          {(s) => (
                            <button class="kb-pop-src" classList={{ dim: !layerOn() }} disabled={!layerOn()} onClick={() => toggle(s.key)}>
                              <span class="kb-check" classList={{ on: !off().includes(s.key) }} />
                              <span class="kb-pop-name">{s.name}</span>
                              <span class="kb-pop-count">{s.notes.toLocaleString()}</span>
                            </button>
                          )}
                        </For>
                      </Show>
                    </div>
                  </Show>
                );
              }}
            </For>
            <div class="menu-sep" />
            <button
              class="menu-item"
              onClick={() => {
                setOpen(false);
                openKnowledge("sources");
              }}
            >
              <span class="menu-label">Manage</span>
            </button>
          </div>
        </Portal>
      </Show>
    </Show>
  );
}
