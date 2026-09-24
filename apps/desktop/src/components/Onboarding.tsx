import { createResource, createSignal, For, Show } from "solid-js";
import { BrainCircuit, Check, FolderGit2, MessagesSquare, Radar, Sparkles } from "lucide-solid";
import { api, pickFolder } from "../lib/api";
import { addProject, finishOnboarding, scanForModel, state } from "../lib/store";
import type { ImportSource } from "../lib/types";
import { Logo, Spinner } from "./ui";

export default function Onboarding() {
  const [sources] = createResource(() => api.importDetect().catch(() => [] as ImportSource[]));
  const [scanning, setScanning] = createSignal(false);
  const [scanMsg, setScanMsg] = createSignal("");
  const [chosen, setChosen] = createSignal<Record<string, { projects: boolean; chats: boolean }>>({});
  const [importing, setImporting] = createSignal("");
  const [done, setDone] = createSignal<Record<string, string>>({});

  const gateways = () => state.config?.gateways.length ?? 0;

  const scan = async () => {
    setScanning(true);
    setScanMsg("");
    const n = await scanForModel();
    setScanning(false);
    setScanMsg(n ? `Found ${n} model server${n === 1 ? "" : "s"}` : "No local server found — you can add one in Settings later");
  };

  const toggle = (tool: string, key: "projects" | "chats", def: boolean) => {
    setChosen((c) => {
      const cur = c[tool] ?? { projects: def && key !== "chats", chats: false };
      return { ...c, [tool]: { ...cur, [key]: !((c[tool] ?? { projects: true, chats: false })[key]) } };
    });
  };
  const pick = (tool: string, key: "projects" | "chats") => chosen()[tool]?.[key] ?? false;

  const runImport = async (s: ImportSource) => {
    const sel = chosen()[s.tool] ?? { projects: true, chats: false };
    if (!sel.projects && !sel.chats) return;
    setImporting(s.tool);
    try {
      const r = await api.importRun(s.tool, sel.projects, sel.chats);
      setDone((d) => ({ ...d, [s.tool]: `Imported ${r.projects} project${r.projects === 1 ? "" : "s"}${r.chats ? `, ${r.chats} chats` : ""}` }));
    } catch (e) {
      setDone((d) => ({ ...d, [s.tool]: String(e instanceof Error ? e.message : e) }));
    }
    setImporting("");
  };

  return (
    <div class="overlay onboarding">
      <div class="ob-scroll">
        <div class="ob-hero">
          <Logo size={40} />
          <h1>Welcome to Xode</h1>
          <p>A coding agent for local models. Let’s get you set up — this takes a few seconds.</p>
        </div>

        <section class="ob-card">
          <div class="ob-card-head">
            <BrainCircuit size={18} stroke-width={1.7} />
            <span>Connect a model</span>
            <span class="spacer" />
            <Show when={gateways()}>
              <span class="ob-ok">
                <Check size={14} stroke-width={2} /> {gateways()} gateway{gateways() === 1 ? "" : "s"}
              </span>
            </Show>
          </div>
          <p class="ob-sub">Xode works with any OpenAI-compatible or Anthropic endpoint (llama.cpp, Ollama, LM Studio, vLLM, cloud).</p>
          <div class="ob-row">
            <button class="btn" onClick={scan} disabled={scanning()}>
              <Show when={scanning()} fallback={<Radar size={15} stroke-width={1.7} />}>
                <Spinner size={15} />
              </Show>
              Scan for local models
            </button>
            <Show when={scanMsg()}>
              <span class="ob-note">{scanMsg()}</span>
            </Show>
          </div>
        </section>

        <section class="ob-card">
          <div class="ob-card-head">
            <Sparkles size={18} stroke-width={1.7} />
            <span>Import from another agent</span>
          </div>
          <p class="ob-sub">Bring your projects and chats over from the tools you already use.</p>
          <Show when={!sources.loading} fallback={<div class="ob-loading"><Spinner size={16} /> Looking for installed agents…</div>}>
            <For each={sources()?.filter((s) => s.available) ?? []} fallback={<div class="ob-note">No supported agents found on this machine.</div>}>
              {(s) => (
                <div class="ob-src">
                  <div class="ob-src-main">
                    <div class="ob-src-name">{s.label}</div>
                    <div class="ob-src-counts">
                      {s.projects} project{s.projects === 1 ? "" : "s"}
                      <Show when={s.chats}> · {s.chats} chats</Show>
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
                    <label class="ob-chk" classList={{ off: !s.projects }}>
                      <input type="checkbox" checked={pick(s.tool, "projects") || (chosen()[s.tool] === undefined && s.projects > 0)} disabled={!s.projects} onChange={() => toggle(s.tool, "projects", true)} />
                      <FolderGit2 size={13} stroke-width={1.7} /> Projects
                    </label>
                    <label class="ob-chk" classList={{ off: !s.chats }}>
                      <input type="checkbox" checked={pick(s.tool, "chats")} disabled={!s.chats} onChange={() => toggle(s.tool, "chats", false)} />
                      <MessagesSquare size={13} stroke-width={1.7} /> Chats
                    </label>
                    <button class="btn sm" onClick={() => runImport(s)} disabled={importing() === s.tool}>
                      <Show when={importing() === s.tool} fallback={"Import"}>
                        <Spinner size={13} />
                      </Show>
                    </button>
                  </Show>
                </div>
              )}
            </For>
          </Show>
        </section>

        <section class="ob-card">
          <div class="ob-card-head">
            <FolderGit2 size={18} stroke-width={1.7} />
            <span>Or add a project folder</span>
          </div>
          <div class="ob-row">
            <button
              class="btn"
              onClick={async () => {
                const d = await pickFolder();
                if (d) await addProject(d);
              }}
            >
              Choose folder…
            </button>
          </div>
        </section>

        <div class="ob-actions">
          <button class="btn primary lg" onClick={finishOnboarding}>
            Start using Xode
          </button>
        </div>
      </div>
    </div>
  );
}
