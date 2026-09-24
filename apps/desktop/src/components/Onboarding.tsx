import { createResource, createSignal, Show } from "solid-js";
import { BrainCircuit, Check, FolderGit2, Plus, Radar, Sparkles } from "lucide-solid";
import { api, pickFolder } from "../lib/api";
import ImportRows from "./ImportRows";
import { addProject, finishOnboarding, patchConfig, scanForModel, state, toast } from "../lib/store";
import type { ImportSource } from "../lib/types";
import { Logo, Spinner } from "./ui";

export default function Onboarding() {
  const [sources] = createResource(() => api.importDetect().catch(() => [] as ImportSource[]));
  const [scanning, setScanning] = createSignal(false);
  const [scanMsg, setScanMsg] = createSignal("");

  const gateways = () => state.config?.gateways.length ?? 0;
  const [gwUrl, setGwUrl] = createSignal("");
  const [gwKey, setGwKey] = createSignal("");
  const [detecting, setDetecting] = createSignal(false);

  const detect = async () => {
    if (!gwUrl().trim()) return;
    setDetecting(true);
    try {
      const g = await api.detectGateway(gwUrl().trim(), gwKey());
      patchConfig((c) => {
        if (!c.gateways.some((x) => x.url.replace(/\/+$/, "") === g.url.replace(/\/+$/, ""))) c.gateways.push(g);
        if (!c.selected.gateway && g.models[0]) {
          c.selected.gateway = g.id;
          c.selected.model = g.models[0].id;
        }
      });
      setGwUrl("");
      setGwKey("");
      setScanMsg(`Added ${g.name || g.url}`);
    } catch (e) {
      toast(String(e instanceof Error ? e.message : e), "error");
    }
    setDetecting(false);
  };

  const scan = async () => {
    setScanning(true);
    setScanMsg("");
    const n = await scanForModel();
    setScanning(false);
    setScanMsg(n ? `Found ${n} model server${n === 1 ? "" : "s"}` : "No local server found — you can add one in Settings later");
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
          <div class="ob-gw">
            <input class="input mono grow" placeholder="http://localhost:8080" value={gwUrl()} onInput={(e) => setGwUrl(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && detect()} />
            <input class="input" type="password" placeholder="API key (optional)" value={gwKey()} onInput={(e) => setGwKey(e.currentTarget.value)} />
            <button class="btn primary" onClick={detect} disabled={detecting() || !gwUrl().trim()}>
              <Show when={detecting()} fallback={<Plus size={14} stroke-width={1.75} />}>
                <Spinner size={13} />
              </Show>
              Add
            </button>
          </div>
        </section>

        <section class="ob-card">
          <div class="ob-card-head">
            <Sparkles size={18} stroke-width={1.7} />
            <span>Import from another agent</span>
          </div>
          <p class="ob-sub">Bring your projects, chats and gateways over from the tools you already use.</p>
          <Show when={!sources.loading} fallback={<div class="ob-loading"><Spinner size={16} /> Looking for installed agents…</div>}>
            <ImportRows sources={sources() ?? []} />
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
