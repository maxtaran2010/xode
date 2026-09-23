import { createSignal, Show } from "solid-js";
import { FolderOpen } from "lucide-solid";
import { api, pickFile, pickFolder } from "../../lib/api";
import type { Browser, GatewayTest } from "../../lib/types";
import { Spinner } from "../ui";
import { cfg, Field, Group, NumberField, Page, patch, Select, StatusDot, TagInput, TextField, Toggle } from "./controls";

export default function BrowserPage() {
  const b = () => cfg().browser;
  const set = <K extends keyof Browser>(k: K, v: Browser[K]) => patch((c) => (c.browser[k] = v));
  const [test, setTest] = createSignal<GatewayTest | "busy" | null>(null);
  const run = async () => {
    setTest("busy");
    try {
      setTest(await api.browserTest());
    } catch (e) {
      setTest({ ok: false, latency_ms: 0, models: 0, message: String(e) });
    }
  };
  const result = () => {
    const t = test();
    return t && t !== "busy" ? t : null;
  };
  return (
    <Page
      title="Browser"
      actions={
        <>
          <Show when={result()}>
            {(r) => (
              <span class="test-result" classList={{ err: !r().ok }}>
                <StatusDot status={r().ok ? "ok" : "err"} />
                {r().message}
              </span>
            )}
          </Show>
          <button class="btn" onClick={run} disabled={test() === "busy"}>
            <Show when={test() === "busy"} fallback="Test">
              <Spinner size={12} />
            </Show>
          </button>
        </>
      }
    >
      <Group>
        <Field label="Browser">
          <Select value={b().kind} options={["edge", "chrome", "brave", "chromium", "custom"]} onChange={(v) => set("kind", v)} width={160} />
        </Field>
        <Field label="Executable">
          <div class="row-inline">
            <TextField value={b().executable} onChange={(v) => set("executable", v)} mono width={280} placeholder="auto" />
            <button
              class="icon-btn"
              onClick={async () => {
                const f = await pickFile();
                if (f) set("executable", f);
              }}
              aria-label="Browse"
            >
              <FolderOpen size={15} stroke-width={1.6} />
            </button>
          </div>
        </Field>
        <Field label="Mode">
          <Select value={b().mode} options={[{ value: "attach", label: "Attach" }, { value: "launch", label: "Launch" }]} onChange={(v) => set("mode", v)} width={160} />
        </Field>
        <Field label="Debug port">
          <NumberField value={b().debug_port} onChange={(v) => set("debug_port", v ?? 9222)} min={1} max={65535} />
        </Field>
        <Field label="User data dir">
          <div class="row-inline">
            <TextField value={b().user_data_dir} onChange={(v) => set("user_data_dir", v)} mono width={280} placeholder="managed" />
            <button
              class="icon-btn"
              onClick={async () => {
                const d = await pickFolder();
                if (d) set("user_data_dir", d);
              }}
              aria-label="Browse"
            >
              <FolderOpen size={15} stroke-width={1.6} />
            </button>
          </div>
        </Field>
        <Field label="Profile">
          <TextField value={b().profile} onChange={(v) => set("profile", v)} width={200} />
        </Field>
        <Field label="Headless">
          <Toggle value={b().headless} onChange={(v) => set("headless", v)} />
        </Field>
        <Field label="Extra args">
          <TagInput values={b().extra_args} onChange={(v) => set("extra_args", v)} placeholder="--flag" />
        </Field>
        <Field label="Snapshot max tokens">
          <NumberField value={b().snapshot_max_tokens} onChange={(v) => set("snapshot_max_tokens", v ?? 0)} step={500} />
        </Field>
      </Group>
    </Page>
  );
}
