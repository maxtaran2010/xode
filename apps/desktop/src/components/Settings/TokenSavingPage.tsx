import { For } from "solid-js";
import type { TokenSaving } from "../../lib/types";
import { cfg, Field, Group, NumberField, Page, patch, Toggle } from "./controls";

type BoolKey = { [K in keyof TokenSaving]: TokenSaving[K] extends boolean ? K : never }[keyof TokenSaving];
type NumKey = { [K in keyof TokenSaving]: TokenSaving[K] extends number ? K : never }[keyof TokenSaving];

const RTK: [BoolKey, string][] = [
  ["rtk_enabled", "Output filters"],
  ["rtk_strip_ansi", "Strip ANSI"],
  ["rtk_collapse_progress", "Collapse progress"],
  ["rtk_dedup_lines", "Deduplicate lines"],
  ["rtk_command_filters", "Command filters"],
];
const OUTPUT: [NumKey | BoolKey, string, "num" | "bool"][] = [
  ["max_tool_output_tokens", "Max tool output tokens", "num"],
  ["head_lines", "Head lines", "num"],
  ["tail_lines", "Tail lines", "num"],
  ["save_full_output", "Save full output", "bool"],
];
const READS: [NumKey | BoolKey, string, "num" | "bool"][] = [
  ["read_dedup", "Deduplicate reads", "bool"],
  ["read_max_lines", "Read max lines", "num"],
  ["repo_map_tokens", "Repo map tokens", "num"],
  ["attach_inline_max_tokens", "Inline attachment tokens", "num"],
  ["drop_old_thinking", "Drop old thinking", "bool"],
  ["stub_old_tool_results_after", "Stub tool results after turns", "num"],
];

function Rows(props: { rows: [NumKey | BoolKey, string, "num" | "bool"][] }) {
  const t = () => cfg().token_saving;
  return (
    <For each={props.rows}>
      {([k, label, kind]) => (
        <Field label={label}>
          {kind === "bool" ? (
            <Toggle value={t()[k as BoolKey]} onChange={(v) => patch((c) => ((c.token_saving[k as BoolKey] as boolean) = v))} />
          ) : (
            <NumberField value={t()[k as NumKey]} onChange={(v) => patch((c) => ((c.token_saving[k as NumKey] as number) = v ?? 0))} min={0} />
          )}
        </Field>
      )}
    </For>
  );
}

export default function TokenSavingPage() {
  return (
    <Page title="Token Saving">
      <Group title="RTK">
        <Rows rows={RTK.map(([k, l]) => [k, l, "bool"])} />
      </Group>
      <Group title="Tool output">
        <Rows rows={OUTPUT} />
      </Group>
      <Group title="Context">
        <Rows rows={READS} />
      </Group>
    </Page>
  );
}
