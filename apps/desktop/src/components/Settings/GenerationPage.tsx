import { effort, EFFORTS, setEffort } from "../../lib/store";
import type { Generation } from "../../lib/types";
import { Segmented } from "../ui";
import { cfg, Field, Group, NumberField, Page, patch, SliderField, TagInput, TextArea, Toggle } from "./controls";

type NumKey = "temperature" | "top_p" | "min_p" | "repeat_penalty" | "presence_penalty" | "frequency_penalty";

export default function GenerationPage() {
  const g = () => cfg().generation;
  const set = <K extends keyof Generation>(k: K, v: Generation[K]) => patch((c) => (c.generation[k] = v));
  const slider = (k: NumKey, label: string, min: number, max: number, step: number) => (
    <Field label={label}>
      <SliderField value={g()[k]} onChange={(v) => set(k, v)} min={min} max={max} step={step} nullable />
    </Field>
  );
  return (
    <Page title="Generation">
      <Group title="Reasoning">
        <Field label="Effort">
          <Segmented size="sm" value={effort()} options={EFFORTS} onChange={setEffort} />
        </Field>
        <Field label="Low budget">
          <NumberField value={g().reasoning_budget.low} onChange={(v) => patch((c) => (c.generation.reasoning_budget.low = v ?? 2048))} min={256} step={512} width={110} />
        </Field>
        <Field label="Medium budget">
          <NumberField value={g().reasoning_budget.medium} onChange={(v) => patch((c) => (c.generation.reasoning_budget.medium = v ?? 8192))} min={256} step={512} width={110} />
        </Field>
        <Field label="High budget">
          <NumberField value={g().reasoning_budget.high} onChange={(v) => patch((c) => (c.generation.reasoning_budget.high = v ?? 24576))} min={256} step={512} width={110} />
        </Field>
        <Field label="Keep thinking">
          <Toggle value={g().keep_thinking} onChange={(v) => set("keep_thinking", v)} />
        </Field>
      </Group>
      <Group title="Sampling">
        {slider("temperature", "Temperature", 0, 2, 0.05)}
        {slider("top_p", "Top P", 0, 1, 0.01)}
        {slider("min_p", "Min P", 0, 1, 0.01)}
        <Field label="Top K">
          <NumberField value={g().top_k} onChange={(v) => set("top_k", v)} nullable min={0} />
        </Field>
        {slider("repeat_penalty", "Repeat penalty", 0.8, 2, 0.01)}
        {slider("presence_penalty", "Presence penalty", -2, 2, 0.05)}
        {slider("frequency_penalty", "Frequency penalty", -2, 2, 0.05)}
      </Group>
      <Group title="Output">
        <Field label="Max tokens">
          <NumberField value={g().max_tokens} onChange={(v) => set("max_tokens", v)} nullable min={1} step={256} />
        </Field>
        <Field label="Seed">
          <NumberField value={g().seed} onChange={(v) => set("seed", v)} nullable />
        </Field>
        <Field label="Stop sequences">
          <TagInput values={g().stop} onChange={(v) => set("stop", v)} />
        </Field>
        <Field label="Parallel tool calls">
          <Toggle value={g().parallel_tool_calls} onChange={(v) => set("parallel_tool_calls", v)} />
        </Field>
        <Field label="Request timeout (s)">
          <NumberField value={g().request_timeout_s} onChange={(v) => set("request_timeout_s", v ?? 900)} min={1} />
        </Field>
      </Group>
      <Group title="Extra body">
        <div class="field stack">
          <TextArea value={g().extra_body} onChange={(v) => set("extra_body", v)} rows={5} placeholder="{}" />
        </div>
      </Group>
    </Page>
  );
}
