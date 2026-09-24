import { createSignal, For, Show } from "solid-js";
import { Check, FolderGit2, MessagesSquare, Server } from "lucide-solid";
import { api } from "../lib/api";
import { reloadAll, toast } from "../lib/store";
import type { ImportSource } from "../lib/types";
import { Spinner } from "./ui";

type Kind = "projects" | "chats" | "gateways";
type Sel = Record<Kind, boolean>;

const KINDS: { key: Kind; label: string; icon: typeof FolderGit2 }[] = [
  { key: "projects", label: "Projects", icon: FolderGit2 },
  { key: "chats", label: "Chats", icon: MessagesSquare },
  { key: "gateways", label: "Gateways", icon: Server },
];

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** Per-agent import rows: counts, what to bring over, and an Import button. */
export default function ImportRows(props: { sources: ImportSource[] }) {
  const [chosen, setChosen] = createSignal<Record<string, Sel>>({});
  const [busy, setBusy] = createSignal("");
  const [done, setDone] = createSignal<Record<string, string>>({});

  const sel = (s: ImportSource): Sel => chosen()[s.tool] ?? { projects: s.projects > 0, chats: s.chats > 0, gateways: s.gateways > 0 };
  const toggle = (s: ImportSource, k: Kind) => setChosen((c) => ({ ...c, [s.tool]: { ...sel(s), [k]: !sel(s)[k] } }));

  const run = async (s: ImportSource) => {
    const o = sel(s);
    if (!o.projects && !o.chats && !o.gateways) return;
    setBusy(s.tool);
    try {
      const r = await api.importRun(s.tool, o.projects, o.chats, o.gateways);
      const parts = [r.projects && plural(r.projects, "project"), r.chats && plural(r.chats, "chat"), r.gateways && plural(r.gateways, "gateway")].filter(Boolean);
      setDone((d) => ({ ...d, [s.tool]: parts.length ? `Imported ${parts.join(", ")}` : "Up to date" }));
      await reloadAll();
    } catch (e) {
      toast(`${s.label}: ${e instanceof Error ? e.message : e}`, "error");
    }
    setBusy("");
  };

  return (
    <For each={props.sources.filter((s) => s.available)} fallback={<div class="ob-note">No supported agents found</div>}>
      {(s) => (
        <div class="ob-src">
          <div class="ob-src-main">
            <div class="ob-src-name">{s.label}</div>
            <div class="ob-src-counts">
              {[plural(s.projects, "project"), plural(s.chats, "chat"), plural(s.gateways, "gateway")].join(" · ")}
            </div>
          </div>
          <Show
            when={!done()[s.tool]}
            fallback={
              <span class="ob-ok">
                <Check size={14} stroke-width={2} /> {done()[s.tool]}
              </span>
            }
          >
            <For each={KINDS}>
              {(k) => (
                <label class="ob-chk" classList={{ off: !s[k.key] }}>
                  <input type="checkbox" checked={sel(s)[k.key]} disabled={!s[k.key]} onChange={() => toggle(s, k.key)} />
                  <k.icon size={13} stroke-width={1.7} /> {k.label}
                </label>
              )}
            </For>
            <button class="btn sm" onClick={() => run(s)} disabled={busy() === s.tool}>
              <Show when={busy() === s.tool} fallback={"Import"}>
                <Spinner size={13} />
              </Show>
            </button>
          </Show>
        </div>
      )}
    </For>
  );
}
