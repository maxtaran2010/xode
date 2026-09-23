// Small shared UI primitives: popover menu, segmented control, logo mark, ticking clock.
import { createEffect, createSignal, For, type JSX, on, onCleanup, onMount, Show, createRoot } from "solid-js";
import { Portal } from "solid-js/web";
import { Check, LoaderCircle } from "lucide-solid";

// ---------- clock (1 Hz tick shared by timers)

export const now = createRoot(() => {
  const [t, setT] = createSignal(Date.now());
  setInterval(() => setT(Date.now()), 500);
  return t;
});

// ---------- menu

export type MenuItem =
  | { type?: "item"; label: string; icon?: (p: { size: number; "stroke-width": number }) => JSX.Element; hint?: string; checked?: boolean; danger?: boolean; disabled?: boolean; onSelect: () => void }
  | { type: "header"; label: string }
  | { type: "separator" };

export interface MenuState {
  x: number;
  y: number;
  /** Anchor rect: menu opens below (or above when `up`) this rect. */
  anchor?: DOMRect;
  up?: boolean;
  align?: "left" | "right";
  items: MenuItem[];
  minWidth?: number;
}

const [menu, setMenu] = createSignal<MenuState | null>(null);
const [closing, setClosing] = createSignal(false);
let closeTimer: ReturnType<typeof setTimeout> | undefined;
export const openMenu = (m: MenuState) => {
  clearTimeout(closeTimer);
  setClosing(false);
  setMenu(m);
};
/** Plays the exit animation, then unmounts. */
export const closeMenu = () => {
  if (!menu() || closing()) return;
  setClosing(true);
  closeTimer = setTimeout(() => {
    setMenu(null);
    setClosing(false);
  }, 100);
};

export function menuAt(e: MouseEvent, items: MenuItem[]) {
  e.preventDefault();
  e.stopPropagation();
  openMenu({ x: e.clientX, y: e.clientY, items });
}

export function menuFor(el: HTMLElement, items: MenuItem[], opts: Partial<MenuState> = {}) {
  const r = el.getBoundingClientRect();
  openMenu({ x: r.left, y: r.bottom, anchor: r, items, ...opts });
}

export function MenuHost() {
  let ref: HTMLDivElement | undefined;
  const [pos, setPos] = createSignal<{ left: number; top: number } | null>(null);

  const [up, setUp] = createSignal(false);
  const place = (m: MenuState) => {
    requestAnimationFrame(() => {
      if (!ref) return;
      const w = ref.offsetWidth;
      const h = ref.offsetHeight;
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      let left = m.x;
      let top = m.y;
      if (m.anchor) {
        left = m.align === "right" ? m.anchor.right - w : m.anchor.left;
        top = m.up ? m.anchor.top - h - 6 : m.anchor.bottom + 6;
        if (!m.up && top + h > vh - 8) top = m.anchor.top - h - 6;
        setUp(top < m.anchor.top);
      } else setUp(false);
      left = Math.max(8, Math.min(left, vw - w - 8));
      top = Math.max(8, Math.min(top, vh - h - 8));
      setPos({ left, top });
    });
  };

  const onDown = (e: MouseEvent) => {
    if (menu() && ref && !ref.contains(e.target as Node)) closeMenu();
  };
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && closeMenu();
  onMount(() => {
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", closeMenu);
  });
  onCleanup(() => {
    window.removeEventListener("mousedown", onDown, true);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("blur", closeMenu);
  });

  return (
    <Show when={menu()} keyed>
      {(m) => {
        setPos(null);
        place(m);
        return (
          <Portal>
            <div
              ref={ref}
              class="menu"
              classList={{ up: up(), closing: closing() }}
              style={{ left: `${pos()?.left ?? m.x}px`, top: `${pos()?.top ?? m.y}px`, visibility: pos() ? "visible" : "hidden", "min-width": `${m.minWidth ?? 180}px` }}
              onContextMenu={(e) => e.preventDefault()}
            >
              <For each={m.items}>
                {(it) =>
                  it.type === "separator" ? (
                    <div class="menu-sep" />
                  ) : it.type === "header" ? (
                    <div class="menu-header">{it.label}</div>
                  ) : (
                    <button
                      class="menu-item"
                      classList={{ danger: !!it.danger }}
                      disabled={it.disabled}
                      onClick={() => {
                        closeMenu();
                        it.onSelect();
                      }}
                    >
                      <span class="menu-icon">{it.icon ? it.icon({ size: 15, "stroke-width": 1.6 }) : null}</span>
                      <span class="menu-label">{it.label}</span>
                      <Show when={it.hint}>
                        <span class="menu-hint">{it.hint}</span>
                      </Show>
                      <span class="menu-check">
                        <Show when={it.checked}>
                          <Check size={14} stroke-width={1.75} />
                        </Show>
                      </span>
                    </button>
                  )
                }
              </For>
            </div>
          </Portal>
        );
      }}
    </Show>
  );
}

// ---------- segmented

export function Segmented<T extends string>(props: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; size?: "sm" | "md"; class?: string }) {
  let ref!: HTMLDivElement;
  const [thumb, setThumb] = createSignal<{ x: number; w: number; instant: boolean } | null>(null);
  const place = (instant: boolean) => {
    const el = ref?.querySelector<HTMLElement>("button.on");
    if (el && el.offsetWidth) setThumb({ x: el.offsetLeft, w: el.offsetWidth, instant });
    else setThumb(null);
  };
  onMount(() => {
    requestAnimationFrame(() => place(true));
    const ro = new ResizeObserver(() => place(true));
    ro.observe(ref);
    onCleanup(() => ro.disconnect());
  });
  createEffect(on(() => props.value, () => requestAnimationFrame(() => place(false)), { defer: true }));
  return (
    <div ref={ref} class={`seg ${props.size === "sm" ? "seg-sm" : ""} ${props.class ?? ""}`} classList={{ "has-thumb": !!thumb() }}>
      <Show when={thumb()}>
        {(t) => <span class="seg-thumb" classList={{ instant: t().instant }} style={{ transform: `translateX(${t().x}px)`, width: `${t().w}px` }} />}
      </Show>
      <For each={props.options}>
        {(o) => (
          <button classList={{ on: props.value === o.value }} onClick={() => props.onChange(o.value)}>
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}

// ---------- logo mark (brand, not an icon)

export function Logo(props: { size?: number; class?: string }) {
  const s = () => props.size ?? 20;
  return (
    <svg class={props.class} width={s()} height={s()} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="1.5" y="1.5" width="21" height="21" rx="5.5" fill="var(--elev)" stroke="var(--border)" />
      <path d="M8.2 8.2 15.8 15.8" stroke="var(--accent)" stroke-width="2.2" stroke-linecap="round" />
      <path d="M15.8 8.2 8.2 15.8" stroke="var(--text)" stroke-width="2.2" stroke-linecap="round" />
    </svg>
  );
}

export function Spinner(props: { size?: number }) {
  return <LoaderCircle class="spin" size={props.size ?? 14} stroke-width={1.75} />;
}
