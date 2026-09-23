// Motion helpers: mount/unmount with enter/exit animations, height collapse, keyed crossfade.
// Animations themselves live in styles/motion.css and key off `data-phase`.
import { createEffect, createMemo, createSignal, type JSX, on, onCleanup, Show } from "solid-js";

export type Phase = "in" | "out";

const reduced = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Keeps content mounted for `exitMs` after `show` turns false so an exit animation can play. */
export function createPresence(show: () => boolean, exitMs = 180) {
  const [mounted, setMounted] = createSignal(show());
  const [phase, setPhase] = createSignal<Phase>("in");
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(show, (v) => {
      clearTimeout(timer);
      if (v) {
        setPhase("in");
        setMounted(true);
      } else if (mounted()) {
        setPhase("out");
        const ms = reduced() ? 0 : exitMs;
        timer = setTimeout(() => setMounted(false), ms);
      }
    }),
  );
  onCleanup(() => clearTimeout(timer));
  return { mounted, phase };
}

/** Wrapper (display: contents) exposing `data-phase` to its child for CSS enter/exit animations. */
export function Presence(props: { when: boolean; exitMs?: number; name?: string; children: JSX.Element }) {
  const p = createPresence(() => props.when, props.exitMs);
  return (
    <Show when={p.mounted()}>
      <div class="presence" data-motion={props.name} data-phase={p.phase()}>
        {props.children}
      </div>
    </Show>
  );
}

/** Height-animated disclosure. Content mounts on open and unmounts after collapsing. */
export function Collapse(props: { open: boolean; class?: string; children: JSX.Element }) {
  const p = createPresence(() => props.open, 200);
  const [expanded, setExpanded] = createSignal(false);
  createEffect(
    on(p.phase, (ph) => {
      if (ph === "in") requestAnimationFrame(() => requestAnimationFrame(() => setExpanded(true)));
      else setExpanded(false);
    }),
  );
  return (
    <Show when={p.mounted()}>
      <div class="collapse" classList={{ open: expanded() }}>
        <div class={`collapse-inner ${props.class ?? ""}`}>{props.children}</div>
      </div>
    </Show>
  );
}

/** Re-mounts its content with an enter animation whenever `key` changes (tab/page switches). */
export function Switcher<K>(props: { key: K; class?: string; motion?: string; children: JSX.Element }) {
  const key = createMemo(() => props.key);
  const boxed = createMemo(() => [key()]);
  return (
    <Show when={boxed()} keyed>
      {(_k) => (
        <div class={`switcher ${props.class ?? ""}`} data-motion={props.motion ?? "fade-up"}>
          {props.children}
        </div>
      )}
    </Show>
  );
}
