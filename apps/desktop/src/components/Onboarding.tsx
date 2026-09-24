import { createResource, createSignal, Show } from "solid-js";
import { BrainCircuit, Check, FolderGit2, Radar, Sparkles } from "lucide-solid";
import { api, pickFolder } from "../lib/api";
import ImportRows from "./ImportRows";
import { addProject, finishOnboarding, scanForModel, state } from "../lib/store";
import type { ImportSource } from "../lib/types";
import { Logo, Spinner } from "./ui";

export default function Onboarding() {
  const [sources] = createResource(() => api.importDetect().catch(() => [] as ImportSource[]));
  const [scanning, setScanning] = createSignal(false);
  const [scanMsg, setScanMsg] = createSignal("");

  const gateways = () => state.config?.gateways.length ?? 0;

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
