import { For, Show } from "solid-js";
import { Plus, X } from "lucide-solid";
import { TOOL_NAMES } from "../../lib/defaults";
import type { Perm, Rule } from "../../lib/types";
import { cfg, Field, Group, Page, patch, PermSeg, Toggle } from "./controls";

function Rules(props: { rules: Rule[]; set: (fn: (r: Rule[]) => void) => void; placeholder: string }) {
  return (
    <>
      <For each={props.rules}>
        {(r, i) => (
          <div class="field rule-row">
            <input
              class="input mono grow"
              value={r.pattern}
              placeholder={props.placeholder}
              onInput={(e) => {
                const v = e.currentTarget.value;
                props.set((l) => (l[i()].pattern = v));
              }}
            />
            <PermSeg value={r.perm} onChange={(v) => props.set((l) => (l[i()].perm = v))} />
            <button class="icon-btn" onClick={() => props.set((l) => l.splice(i(), 1))} aria-label="Remove">
              <X size={14} stroke-width={1.6} />
            </button>
          </div>
        )}
      </For>
      <div class="field">
        <button class="btn ghost sm" onClick={() => props.set((l) => l.push({ pattern: "", perm: "ask" }))}>
          <Plus size={13} stroke-width={1.75} />
          Add rule
        </button>
      </div>
    </>
  );
}

export default function PermissionsPage() {
  const p = () => cfg().permissions;
  return (
    <Page title="Permissions">
      <Group>
        <Field label="Full access">
          <Toggle value={p().full_access} onChange={(v) => patch((c) => (c.permissions.full_access = v))} />
        </Field>
      </Group>
      <Show when={!p().full_access}>
        <Group title="Tools">
          <For each={TOOL_NAMES}>
            {(t) => (
              <Field label={t}>
                <PermSeg value={p().tools[t] ?? "allow"} onChange={(v: Perm) => patch((c) => (c.permissions.tools[t] = v))} />
              </Field>
            )}
          </For>
        </Group>
        <Group title="Access">
          <Field label="Outside project">
            <PermSeg value={p().allow_outside_project} onChange={(v) => patch((c) => (c.permissions.allow_outside_project = v))} />
          </Field>
          <Field label="Network">
            <PermSeg value={p().network} onChange={(v) => patch((c) => (c.permissions.network = v))} />
          </Field>
          <Field label="Browser">
            <PermSeg value={p().browser} onChange={(v) => patch((c) => (c.permissions.browser = v))} />
          </Field>
        </Group>
      </Show>
      <Group title="Shell rules">
        <Rules rules={p().shell_rules} placeholder="git push*" set={(fn) => patch((c) => fn(c.permissions.shell_rules))} />
      </Group>
      <Group title="Path rules">
        <Rules rules={p().path_rules} placeholder="**/.env" set={(fn) => patch((c) => fn(c.permissions.path_rules))} />
      </Group>
    </Page>
  );
}
