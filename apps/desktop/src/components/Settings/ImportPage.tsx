import { createResource, createSignal, For, Show } from "solid-js";
import { Check, FolderGit2, MessagesSquare, RefreshCw } from "lucide-solid";
import { api } from "../../lib/api";
import { finishOnboarding, toast } from "../../lib/store";
import type { ImportSource } from "../../lib/types";
import { Spinner } from "../ui";
import { Group, Page } from "./controls";

export default function ImportPage() {
  const [sources, { refetch }] = createResource(() => api.importDetect().catch(() => [] as ImportSource[]));
  const [chosen, setChosen] = createSignal<Record<string, { projects: boolean; chats: boolean }>>({});
  const [busy, setBusy] = createSignal("");
  const sel = (tool: string, s: ImportSource) => chosen()[tool] ?? { projects: s.projects > 0, chats: false };
  const toggle = (tool: string, s: ImportSource, key: "projects" | "chats") => setChosen((c) => ({ ...c, [tool]: { ...sel(tool, s), [key]: !sel(tool, s)[key] } }));

  const run = async (s: ImportSource) => {
    const opt = sel(s.tool, s);
    if (!opt.projects && !opt.chats) return;
    setBusy(s.tool);
    try {
      const r = await api.importRun(s.tool, opt.projects, opt.chats);
      toast(`${s.label}: imported ${r.projects} project${r.projects === 1 ? "" : "s"}${r.chats ? `, ${r.chats} chats` : ""}`);
      await finishOnboarding(); // refresh projects/sessions without closing settings
    } catch (e) {
      toast(String(e instanceof Error ? e.message : e), "error");
    }
    setBusy("");
  };

  return (
    <Page
      title="Import"
      actions={
        <button class="btn sm" onClick={() => refetch()}>
          <RefreshCw size={14} stroke-width={1.7} />
          Rescan
        </button>
      }
    >
      <Group title="From other agents">
        <Show when={!sources.loading} fallback={<div class="ob-loading"><Spinner size={16} /> Looking…</div>}>
          <For each={sources() ?? []} fallback={<div class="ob-note">No supported agents found.</div>}>
            {(s) => (
              <div class="ob-src" classList={{ off: !s.available }}>
                <div class="ob-src-main">
                  <div class="ob-src-name">{s.label}</div>
                  <div class="ob-src-counts">
                    <Show when={s.available} fallback={"not installed"}>
                      {s.projects} project{s.projects === 1 ? "" : "s"}
                      <Show when={s.chats}> · {s.chats} chats</Show>
                    </Show>
                  </div>
                </div>
                <Show when={s.available}>
                  <label class="ob-chk" classList={{ off: !s.projects }}>
                    <input type="checkbox" checked={sel(s.tool, s).projects} disabled={!s.projects} onChange={() => toggle(s.tool, s, "projects")} />
                    <FolderGit2 size={13} stroke-width={1.7} /> Projects
                  </label>
                  <label class="ob-chk" classList={{ off: !s.chats }}>
                    <input type="checkbox" checked={sel(s.tool, s).chats} disabled={!s.chats} onChange={() => toggle(s.tool, s, "chats")} />
                    <MessagesSquare size={13} stroke-width={1.7} /> Chats
                  </label>
                  <button class="btn sm" onClick={() => run(s)} disabled={busy() === s.tool}>
                    <Show when={busy() === s.tool} fallback={"Import"}>
                      <Spinner size={13} />
                    </Show>
                  </button>
                </Show>
                <Show when={!s.available}>
                  <span class="ob-note">
                    <Check size={13} stroke-width={2} style={{ opacity: 0 }} />
                  </span>
                </Show>
              </div>
            )}
          </For>
        </Show>
      </Group>
      <div class="ob-note" style={{ padding: "4px 2px" }}>
        Claude Code imports projects and full chats. Codex and OpenCode import project folders.
      </div>
    </Page>
  );
}
