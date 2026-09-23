import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { ArrowUp, Brain, ChevronDown, CornerDownRight, SquareSlash, ExternalLink, File, Folder, FolderPlus, Plus, ShieldAlert, ShieldCheck, Square, X } from "lucide-solid";
import { api, openInFileManager, pickFiles, pickFolder } from "../lib/api";
import { basename, debounce, fmtClock, fmtTokens } from "../lib/format";
import {
  activeLive,
  activeProject,
  addProject,
  currentModel,
  effort,
  EFFORTS,
  openSettings,
  selectProject,
  send,
  setEffort,
  setModel,
  setMode,
  setState,
  state,
  steer,
  stop,
  unqueue,
} from "../lib/store";
import type { Attachment, CommandInfo } from "../lib/types";
import ContextRing from "./ContextRing";
import KbPicker from "./Knowledge/KbPicker";
import { menuFor, now, Segmented, type MenuItem } from "./ui";

export function mimeFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  const map: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", bmp: "image/bmp", pdf: "application/pdf" };
  return map[ext] ?? "";
}

export function addAttachments(list: Attachment[]) {
  setState("ui", "attachments", (a) => [...a, ...list.filter((x) => !a.some((y) => (y.path && y.path === x.path) || (y.data && y.data === x.data)))]);
}

export function readFileAttachment(f: globalThis.File): Promise<Attachment> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result);
      resolve({ path: "", name: f.name || "image.png", mime: f.type || "application/octet-stream", data: s.slice(s.indexOf(",") + 1) });
    };
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}

function QueueList() {
  const q = () => activeLive()?.queue ?? [];
  return (
    <Show when={q().length}>
      <div class="queue">
        <For each={q()}>
          {(m, i) => (
            <div class="queue-item">
              <Show when={m.command} fallback={<CornerDownRight size={13} stroke-width={1.7} class="queue-icon" />}>
                <SquareSlash size={13} stroke-width={1.7} class="queue-icon cmd" />
              </Show>
              <span class="queue-text" classList={{ mono: !!m.command }}>{m.text}</span>
              <Show when={i() === 0 && (!m.command || m.text === "/compact")}>
                <button class="queue-steer" onClick={() => steer()}>
                  Steer
                </button>
              </Show>
              <button class="queue-x" onClick={() => unqueue(m.id)} aria-label="Remove">
                <X size={13} stroke-width={1.8} />
              </button>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}

function WorkingRow() {
  const l = () => activeLive();
  const elapsed = () => {
    const s = l();
    if (!s) return 0;
    return s.runStart ? now() - s.runStart : (s.stats?.elapsed_ms ?? 0);
  };
  return (
    <Show when={l()?.running}>
      <div class="working">
        <span class="working-text shimmer">{l()!.activity || "Working…"}</span>
        <span class="working-meta">{fmtClock(elapsed())}</span>
        <Show when={l()!.steps > 0}>
          <span class="working-meta">step {l()!.steps}</span>
        </Show>
        <span class="spacer" />
        <button class="working-stop" onClick={stop}>
          <Square size={11} stroke-width={2} />
          <span>Stop</span>
        </button>
      </div>
    </Show>
  );
}

function Toasts() {
  return (
    <div class="toasts">
      <For each={state.ui.toasts}>{(t) => <div class="toast" classList={{ error: t.kind === "error", leaving: !!t.leaving }}>{t.text}</div>}</For>
    </div>
  );
}

type Popup = { kind: "slash"; items: CommandInfo[] } | { kind: "mention"; items: string[]; start: number; end: number };

export default function Composer() {
  let ta!: HTMLTextAreaElement;
  const [popup, setPopup] = createSignal<Popup | null>(null);
  const [sel, setSel] = createSignal(0);
  const text = () => state.ui.composerText;
  const setText = (v: string) => setState("ui", "composerText", v);
  const running = () => !!activeLive()?.running;
  const cfg = () => state.config;
  const model = () => currentModel();

  const autosize = () => {
    ta.style.height = "auto";
    ta.style.height = `${Math.min(ta.scrollHeight, Math.round(window.innerHeight * 0.4))}px`;
  };
  createEffect(on(text, () => queueMicrotask(autosize)));

  const focus = () => ta?.focus();
  // Esc twice (within 1.5s) stops the running model.
  let lastEsc = 0;
  const onGlobalKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !running() || popup() || state.ui.overlay) return;
    const t = Date.now();
    if (t - lastEsc < 1500) {
      lastEsc = 0;
      stop();
    } else {
      lastEsc = t;
    }
  };
  onMount(() => window.addEventListener("keydown", onGlobalKey));
  onCleanup(() => window.removeEventListener("keydown", onGlobalKey));

  onMount(() => {
    window.addEventListener("xode:focus-composer", focus);
    onCleanup(() => window.removeEventListener("xode:focus-composer", focus));
    focus();
  });
  createEffect(on(() => state.activeSession, () => queueMicrotask(focus), { defer: true }));

  // ---------- popups
  const fetchMentions = debounce(async (q: string, start: number, end: number) => {
    const pid = state.activeProject;
    if (!pid) return;
    const items = await api.completePath(pid, q).catch(() => [] as string[]);
    const cur = popup();
    if (cur && cur.kind !== "mention") return;
    setPopup(items.length ? { kind: "mention", items: items.slice(0, 12), start, end } : null);
    setSel(0);
  }, 80);

  const updatePopup = () => {
    const v = ta.value;
    const caret = ta.selectionStart ?? v.length;
    const slash = /^\/(\S*)$/.exec(v.slice(0, caret));
    if (slash && !v.includes("\n")) {
      const q = slash[1].toLowerCase();
      const items = state.commands.filter((c) => c.name.toLowerCase().includes(q)).sort((a, b) => Number(!a.name.startsWith(q)) - Number(!b.name.startsWith(q)));
      setPopup(items.length ? { kind: "slash", items } : null);
      setSel(0);
      return;
    }
    const at = /(^|\s)@([^\s@]*)$/.exec(v.slice(0, caret));
    if (at) {
      const start = caret - at[2].length - 1;
      fetchMentions(at[2], start, caret);
      return;
    }
    setPopup(null);
  };

  const choose = (i: number) => {
    const p = popup();
    if (!p) return;
    if (p.kind === "slash") {
      const c = p.items[i];
      if (!c) return;
      setPopup(null);
      if (c.args) {
        setText(`/${c.name} `);
        queueMicrotask(() => ta.setSelectionRange(ta.value.length, ta.value.length));
      } else {
        setText("");
        send(`/${c.name}`, []);
      }
    } else {
      const path = p.items[i];
      if (!path) return;
      const v = text();
      const next = `${v.slice(0, p.start)}@${path} ${v.slice(p.end)}`;
      setText(next);
      setPopup(null);
      const caret = p.start + path.length + 2;
      queueMicrotask(() => ta.setSelectionRange(caret, caret));
    }
  };

  const submit = async () => {
    const t = text();
    const atts = state.ui.attachments;
    if (!t.trim() && !atts.length) return;
    setText("");
    setState("ui", "attachments", []);
    setPopup(null);
    const ok = await send(t, [...atts]);
    if (!ok) {
      setText(t);
      setState("ui", "attachments", atts);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const p = popup();
    if (p) {
      const n = p.items.length;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSel((sel() + 1) % n);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSel((sel() - 1 + n) % n);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        choose(sel());
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setPopup(null);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  };

  const onPaste = async (e: ClipboardEvent) => {
    const files = [...(e.clipboardData?.items ?? [])].filter((i) => i.kind === "file").map((i) => i.getAsFile()).filter(Boolean) as globalThis.File[];
    if (!files.length) return;
    e.preventDefault();
    addAttachments(await Promise.all(files.map(readFileAttachment)));
  };

  const attach = async () => {
    const paths = await pickFiles();
    addAttachments(paths.map((p) => ({ path: p, name: basename(p), mime: mimeFor(p), data: "" })));
  };

  // ---------- menus
  const projectMenu = (e: MouseEvent) => {
    const items: MenuItem[] = [{ type: "header", label: "Projects" }];
    for (const p of state.projects) items.push({ label: p.name, checked: p.id === state.activeProject, onSelect: () => selectProject(p.id) });
    items.push({ type: "separator" });
    items.push({
      label: "Add project",
      icon: FolderPlus,
      onSelect: async () => {
        const d = await pickFolder();
        if (d) addProject(d);
      },
    });
    const p = activeProject();
    if (p) items.push({ label: "Open in file manager", icon: ExternalLink, onSelect: () => openInFileManager(p.root) });
    menuFor(e.currentTarget as HTMLElement, items, { up: true, minWidth: 220 });
  };

  const modelMenu = (e: MouseEvent) => {
    const items: MenuItem[] = [];
    const m = model();
    for (const g of cfg()?.gateways ?? []) {
      if (!g.enabled) continue;
      items.push({ type: "header", label: g.name || g.url });
      for (const mi of g.models)
        items.push({ label: mi.id, hint: mi.context ? fmtTokens(mi.context) : undefined, checked: g.id === m.gateway && mi.id === m.model, onSelect: () => setModel(g.id, mi.id) });
    }
    if (!items.length) items.push({ label: "Add a gateway", onSelect: () => openSettings("gateway") });
    menuFor(e.currentTarget as HTMLElement, items, { up: true, align: "right", minWidth: 240 });
  };

  const effortMenu = (e: MouseEvent) => {
    const items: MenuItem[] = [{ type: "header", label: "Reasoning" }];
    for (const o of EFFORTS) items.push({ label: o.label, checked: effort() === o.value, onSelect: () => setEffort(o.value) });
    menuFor(e.currentTarget as HTMLElement, items, { up: true, align: "right", minWidth: 160 });
  };
  const effortLabel = () => EFFORTS.find((o) => o.value === effort())?.label ?? "Auto";

  // ---------- context ring numbers
  const ctx = createMemo(() => {
    const st = activeLive()?.stats;
    const limit = st?.context_limit || model().context || cfg()?.compaction.fallback_context || 0;
    const c = cfg()?.compaction;
    let threshold = c ? (c.threshold_tokens > 0 ? c.threshold_tokens : Math.round(limit * c.threshold_ratio)) : limit;
    if (c) threshold = Math.min(threshold, limit - c.reserve_tokens);
    return { used: st?.context_used ?? 0, limit, threshold };
  });

  const fullAccess = () => !!cfg()?.permissions.full_access;

  return (
    <div class="composer-wrap">
      <Toasts />
      <QueueList />
      <WorkingRow />
      <div class="composer">
        <Show when={popup()}>
          {(p) => (
            <div class="popup">
              <Show
                when={p().kind === "slash"}
                fallback={
                  <For each={(p() as Extract<Popup, { kind: "mention" }>).items}>
                    {(path, i) => (
                      <button class="popup-item" classList={{ sel: sel() === i() }} onMouseDown={(e) => (e.preventDefault(), choose(i()))} onMouseEnter={() => setSel(i())}>
                        <File size={14} stroke-width={1.6} />
                        <span class="popup-name mono">{path}</span>
                      </button>
                    )}
                  </For>
                }
              >
                <For each={(p() as Extract<Popup, { kind: "slash" }>).items}>
                  {(c, i) => (
                    <button class="popup-item" classList={{ sel: sel() === i() }} onMouseDown={(e) => (e.preventDefault(), choose(i()))} onMouseEnter={() => setSel(i())}>
                      <span class="popup-name">/{c.name}</span>
                      <Show when={c.args}>
                        <span class="popup-args">{c.args}</span>
                      </Show>
                      <span class="popup-desc">{c.description}</span>
                    </button>
                  )}
                </For>
              </Show>
            </div>
          )}
        </Show>

        <div class="chips">
          <button class="chip chip-project" onClick={projectMenu}>
            <Folder size={13} stroke-width={1.6} />
            <span>{activeProject()?.name ?? "No project"}</span>
            <ChevronDown size={12} stroke-width={1.6} />
          </button>
          <For each={state.ui.attachments}>
            {(a, i) => (
              <span class="chip chip-file">
                <Show when={a.data && a.mime.startsWith("image/")} fallback={<File size={13} stroke-width={1.6} />}>
                  <img class="chip-thumb" src={`data:${a.mime};base64,${a.data}`} alt="" />
                </Show>
                <span class="chip-name">{a.name || basename(a.path)}</span>
                <button class="chip-x" onClick={() => setState("ui", "attachments", (l) => l.filter((_, j) => j !== i()))} aria-label="Remove">
                  <X size={12} stroke-width={1.75} />
                </button>
              </span>
            )}
          </For>
        </div>

        <textarea
          ref={ta}
          class="composer-input"
          rows={1}
          placeholder="Ask anything"
          value={text()}
          onInput={(e) => {
            setText(e.currentTarget.value);
            updatePopup();
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => setTimeout(() => setPopup(null), 120)}
          spellcheck={false}
        />

        <div class="composer-bar">
          <button class="icon-btn" onClick={attach} aria-label="Attach">
            <Plus size={16} stroke-width={1.6} />
          </button>
          <Segmented size="sm" value={cfg()?.selected.mode ?? "normal"} options={[{ value: "normal", label: "Normal" }, { value: "plan", label: "Plan" }]} onChange={(v) => setMode(v)} />
          <button class="access" classList={{ full: fullAccess() }} onClick={() => openSettings("permissions")}>
            <Show when={fullAccess()} fallback={<ShieldCheck size={14} stroke-width={1.6} />}>
              <ShieldAlert size={14} stroke-width={1.6} />
            </Show>
            <span>{fullAccess() ? "Full access" : "Ask"}</span>
          </button>
          <span class="spacer" />
          <button class="model-btn" onClick={modelMenu}>
            <span>{model().model || "No model"}</span>
            <ChevronDown size={13} stroke-width={1.6} />
          </button>
          <button class="model-btn effort-btn" classList={{ off: effort() === "off" }} onClick={effortMenu} aria-label="Reasoning effort">
            <Brain size={14} stroke-width={1.6} />
            <span>{effortLabel()}</span>
          </button>
          <KbPicker />
          <ContextRing used={ctx().used} limit={ctx().limit} threshold={ctx().threshold} onClick={() => setState("ui", "overlay", "context")} />
          <Show
            when={running()}
            fallback={
              <button class="send" disabled={!text().trim() && !state.ui.attachments.length} onClick={submit} aria-label="Send">
                <ArrowUp size={16} stroke-width={2} />
              </button>
            }
          >
            <button class="send stop" onClick={stop} aria-label="Stop">
              <Square size={11} stroke-width={2.5} fill="currentColor" />
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}

