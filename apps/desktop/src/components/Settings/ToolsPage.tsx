import { For } from "solid-js";
import { ChevronDown, ChevronUp } from "lucide-solid";
import { TOOL_NAMES } from "../../lib/defaults";
import type { Tools } from "../../lib/types";
import { cfg, Field, Group, NumberField, Page, patch, Select, TextField, Toggle } from "./controls";

export default function ToolsPage() {
  const t = () => cfg().tools;
  const set = <K extends keyof Tools>(k: K, v: Tools[K]) => patch((c) => (c.tools[k] = v));
  const move = (i: number, d: number) =>
    patch((c) => {
      const l = c.tools.search_order;
      const j = i + d;
      if (j < 0 || j >= l.length) return;
      [l[i], l[j]] = [l[j], l[i]];
    });
  return (
    <Page title="Tools">
      <Group title="Enabled">
        <For each={TOOL_NAMES}>
          {(name) => (
            <Field label={name}>
              <Toggle value={t().enabled[name] ?? true} onChange={(v) => patch((c) => (c.tools.enabled[name] = v))} />
            </Field>
          )}
        </For>
      </Group>
      <Group title="Shell">
        <Field label="Shell">
          <Select value={t().shell} options={["auto", "pwsh", "powershell", "cmd", "bash", "zsh", "sh"]} onChange={(v) => set("shell", v)} width={160} />
        </Field>
        <Field label="Timeout (s)">
          <NumberField value={t().shell_timeout_s} onChange={(v) => set("shell_timeout_s", v ?? 300)} min={1} />
        </Field>
      </Group>
      <Group title="Web search">
        <div class="order-list">
          <For each={t().search_order}>
            {(e, i) => (
              <div class="order-row">
                <span class="order-idx">{i() + 1}</span>
                <span class="order-name">{e}</span>
                <span class="spacer" />
                <button class="icon-btn" onClick={() => move(i(), -1)} disabled={i() === 0} aria-label="Up">
                  <ChevronUp size={14} stroke-width={1.6} />
                </button>
                <button class="icon-btn" onClick={() => move(i(), 1)} disabled={i() === t().search_order.length - 1} aria-label="Down">
                  <ChevronDown size={14} stroke-width={1.6} />
                </button>
              </div>
            )}
          </For>
        </div>
        <Field label="SearXNG URL">
          <TextField value={t().searxng_url} onChange={(v) => set("searxng_url", v)} mono width={260} />
        </Field>
        <Field label="Brave key">
          <TextField value={t().brave_key} onChange={(v) => set("brave_key", v)} password width={260} />
        </Field>
        <Field label="Tavily key">
          <TextField value={t().tavily_key} onChange={(v) => set("tavily_key", v)} password width={260} />
        </Field>
        <Field label="Results">
          <NumberField value={t().search_results} onChange={(v) => set("search_results", v ?? 6)} min={1} max={30} />
        </Field>
        <Field label="Fetch max tokens">
          <NumberField value={t().fetch_max_tokens} onChange={(v) => set("fetch_max_tokens", v ?? 4000)} step={500} />
        </Field>
      </Group>
      <Group title="Code index">
        <Field label="Index">
          <Toggle value={t().index_enabled} onChange={(v) => set("index_enabled", v)} />
        </Field>
        <Field label="Watch files">
          <Toggle value={t().index_watch} onChange={(v) => set("index_watch", v)} />
        </Field>
        <Field label="Max file size (KB)">
          <NumberField value={t().index_max_file_kb} onChange={(v) => set("index_max_file_kb", v ?? 512)} step={64} />
        </Field>
      </Group>
      <Group title="Search">
        <Field label="Grep max results">
          <NumberField value={t().grep_max_results} onChange={(v) => set("grep_max_results", v ?? 80)} />
        </Field>
        <Field label="Glob max results">
          <NumberField value={t().glob_max_results} onChange={(v) => set("glob_max_results", v ?? 200)} />
        </Field>
        <Field label="Fuzzy edit">
          <Toggle value={t().edit_fuzzy} onChange={(v) => set("edit_fuzzy", v)} />
        </Field>
        <Field label="Goal judge">
          <Toggle value={t().goal_judge} onChange={(v) => set("goal_judge", v)} />
        </Field>
      </Group>
    </Page>
  );
}
