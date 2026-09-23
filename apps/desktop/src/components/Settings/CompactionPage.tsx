import { Show } from "solid-js";
import { DEFAULT_COMPACT_PROMPT, DEFAULT_SEED_TEMPLATE } from "../../lib/defaults";
import { fmtTokens } from "../../lib/format";
import { currentModel } from "../../lib/store";
import type { Compaction } from "../../lib/types";
import { cfg, Field, Group, NumberField, Page, patch, SliderField, TextArea, TextField, Toggle } from "./controls";

export default function CompactionPage() {
  const k = () => cfg().compaction;
  const set = <K extends keyof Compaction>(key: K, v: Compaction[K]) => patch((c) => (c.compaction[key] = v));
  const limit = () => k().context_limit || currentModel().context || k().fallback_context;
  const threshold = () => Math.min(k().threshold_tokens > 0 ? k().threshold_tokens : Math.round(limit() * k().threshold_ratio), limit() - k().reserve_tokens);
  const pct = (n: number) => `${Math.max(0, Math.min(100, (n / limit()) * 100))}%`;

  return (
    <Page title="Compaction">
      <Group>
        <Field label="Enabled">
          <Toggle value={k().enabled} onChange={(v) => set("enabled", v)} />
        </Field>
        <div class="field stack">
          <div class="threshold-viz">
            <div class="tv-bar">
              <div class="tv-fill" style={{ width: pct(threshold()) }} />
              <div class="tv-hard" style={{ left: pct(limit() * k().hard_ratio) }} />
              <div class="tv-reserve" style={{ width: pct(k().reserve_tokens) }} />
            </div>
            <div class="tv-labels">
              <span>{fmtTokens(threshold())}</span>
              <span class="muted">{fmtTokens(limit())}</span>
            </div>
          </div>
        </div>
      </Group>
      <Group title="Limits">
        <Field label="Context limit">
          <NumberField value={k().context_limit || null} onChange={(v) => set("context_limit", v ?? 0)} nullable placeholder="auto" step={1024} />
        </Field>
        <Field label="Fallback context">
          <NumberField value={k().fallback_context} onChange={(v) => set("fallback_context", v ?? 0)} step={1024} />
        </Field>
        <Field label="Threshold tokens">
          <NumberField value={k().threshold_tokens || null} onChange={(v) => set("threshold_tokens", v ?? 0)} nullable placeholder="ratio" step={1024} />
        </Field>
        <Show when={!k().threshold_tokens}>
          <Field label="Threshold ratio">
            <SliderField value={k().threshold_ratio} onChange={(v) => set("threshold_ratio", v ?? 0.82)} min={0.3} max={0.98} step={0.01} />
          </Field>
        </Show>
        <Field label="Hard ratio">
          <SliderField value={k().hard_ratio} onChange={(v) => set("hard_ratio", v ?? 0.93)} min={0.5} max={1} step={0.01} />
        </Field>
        <Field label="Reserve tokens">
          <NumberField value={k().reserve_tokens} onChange={(v) => set("reserve_tokens", v ?? 0)} step={256} />
        </Field>
      </Group>
      <Group title="Handoff">
        <Field label="Path file">
          <TextField value={k().path_file} onChange={(v) => set("path_file", v)} mono width={220} />
        </Field>
        <Field label="Path max lines">
          <NumberField value={k().path_max_lines} onChange={(v) => set("path_max_lines", v ?? 0)} />
        </Field>
        <Field label="Entry max lines">
          <NumberField value={k().path_entry_max_lines} onChange={(v) => set("path_entry_max_lines", v ?? 0)} />
        </Field>
        <Field label="State max words">
          <NumberField value={k().state_max_words} onChange={(v) => set("state_max_words", v ?? 0)} />
        </Field>
        <Field label="Keep recent messages">
          <NumberField value={k().keep_recent_messages} onChange={(v) => set("keep_recent_messages", v ?? 0)} />
        </Field>
        <Field label="Working set">
          <Toggle value={k().include_working_set} onChange={(v) => set("include_working_set", v)} />
        </Field>
        <Show when={k().include_working_set}>
          <Field label="Working set max files">
            <NumberField value={k().working_set_max_files} onChange={(v) => set("working_set_max_files", v ?? 0)} />
          </Field>
        </Show>
        <Field label="Repo map">
          <Toggle value={k().include_repo_map} onChange={(v) => set("include_repo_map", v)} />
        </Field>
        <Field label="Keep original request">
          <Toggle value={k().keep_original_request} onChange={(v) => set("keep_original_request", v)} />
        </Field>
        <Field label="Fold old entries">
          <Toggle value={k().fold_old_entries} onChange={(v) => set("fold_old_entries", v)} />
        </Field>
      </Group>
      <Group title="Prompt" actions={<button class="btn ghost sm" onClick={() => set("prompt", DEFAULT_COMPACT_PROMPT)}>Reset</button>}>
        <div class="field stack">
          <TextArea value={k().prompt} onChange={(v) => set("prompt", v)} rows={10} />
        </div>
      </Group>
      <Group title="Seed template" actions={<button class="btn ghost sm" onClick={() => set("seed_template", DEFAULT_SEED_TEMPLATE)}>Reset</button>}>
        <div class="field stack">
          <TextArea value={k().seed_template} onChange={(v) => set("seed_template", v)} rows={10} />
        </div>
      </Group>
    </Page>
  );
}
