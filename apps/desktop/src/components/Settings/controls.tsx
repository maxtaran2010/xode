// Settings form controls. Labels + controls only, no descriptions.
import { createSignal, For, type JSX, Show } from "solid-js";
import { Plus, X } from "lucide-solid";
import { patchConfig, state } from "../../lib/store";
import type { Config, Perm } from "../../lib/types";
import { Segmented } from "../ui";

export const cfg = () => state.config as Config;
export const patch = patchConfig;

export function Page(props: { title: string; actions?: JSX.Element; children: JSX.Element }) {
  return (
    <div class="set-page">
      <div class="set-title">
        <h2>{props.title}</h2>
        <span class="spacer" />
        {props.actions}
      </div>
      {props.children}
    </div>
  );
}

export function Group(props: { title?: string; children: JSX.Element; actions?: JSX.Element }) {
  return (
    <div class="set-group">
      <Show when={props.title || props.actions}>
        <div class="set-group-head">
          <span>{props.title}</span>
          <span class="spacer" />
          {props.actions}
        </div>
      </Show>
      <div class="set-card">{props.children}</div>
    </div>
  );
}

export function Field(props: { label: string; children: JSX.Element; stack?: boolean }) {
  return (
    <div class="field" classList={{ stack: !!props.stack }}>
      <span class="field-label">{props.label}</span>
      <div class="field-control">{props.children}</div>
    </div>
  );
}

export function Toggle(props: { value: boolean; onChange: (v: boolean) => void }) {
  return <button class="toggle" classList={{ on: props.value }} onClick={() => props.onChange(!props.value)} role="switch" aria-checked={props.value} />;
}

export function NumberField(props: {
  value: number | null | undefined;
  onChange: (v: number | null) => void;
  nullable?: boolean;
  min?: number;
  max?: number;
  step?: number;
  placeholder?: string;
  width?: number;
}) {
  return (
    <input
      class="input num"
      type="number"
      style={{ width: `${props.width ?? 96}px` }}
      value={props.value ?? ""}
      min={props.min}
      max={props.max}
      step={props.step ?? 1}
      placeholder={props.placeholder ?? (props.nullable ? "default" : "")}
      onInput={(e) => {
        const raw = e.currentTarget.value;
        if (raw === "") {
          if (props.nullable) props.onChange(null);
          return;
        }
        const n = Number(raw);
        if (!Number.isNaN(n)) props.onChange(n);
      }}
    />
  );
}

export function SliderField(props: { value: number | null; onChange: (v: number | null) => void; min: number; max: number; step: number; nullable?: boolean }) {
  return (
    <div class="slider-field">
      <input
        type="range"
        class="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={props.value ?? props.min}
        classList={{ unset: props.value == null }}
        onInput={(e) => props.onChange(Number(e.currentTarget.value))}
      />
      <NumberField value={props.value} onChange={props.onChange} nullable={props.nullable} min={props.min} max={props.max} step={props.step} width={80} />
    </div>
  );
}

export function TextField(props: { value: string; onChange: (v: string) => void; placeholder?: string; password?: boolean; mono?: boolean; width?: number }) {
  return (
    <input
      class="input"
      classList={{ mono: !!props.mono }}
      type={props.password ? "password" : "text"}
      style={props.width ? { width: `${props.width}px` } : undefined}
      value={props.value}
      placeholder={props.placeholder}
      spellcheck={false}
      onInput={(e) => props.onChange(e.currentTarget.value)}
    />
  );
}

export function TextArea(props: { value: string; onChange: (v: string) => void; rows?: number; mono?: boolean; placeholder?: string }) {
  return (
    <textarea
      class="input textarea"
      classList={{ mono: props.mono !== false }}
      rows={props.rows ?? 6}
      value={props.value}
      placeholder={props.placeholder}
      spellcheck={false}
      onInput={(e) => props.onChange(e.currentTarget.value)}
    />
  );
}

export function Select(props: { value: string; options: (string | { value: string; label: string })[]; onChange: (v: string) => void; width?: number }) {
  return (
    <select class="input select" style={props.width ? { width: `${props.width}px` } : undefined} value={props.value} onChange={(e) => props.onChange(e.currentTarget.value)}>
      <For each={props.options}>
        {(o) => {
          const v = typeof o === "string" ? o : o.value;
          const l = typeof o === "string" ? o : o.label;
          return (
            <option value={v} selected={v === props.value}>
              {l}
            </option>
          );
        }}
      </For>
    </select>
  );
}

export function PermSeg(props: { value: Perm; onChange: (v: Perm) => void }) {
  return (
    <Segmented
      size="sm"
      class="perm-seg"
      value={props.value}
      options={[
        { value: "allow", label: "Allow" },
        { value: "ask", label: "Ask" },
        { value: "deny", label: "Deny" },
      ]}
      onChange={props.onChange}
    />
  );
}

export function TagInput(props: { values: string[]; onChange: (v: string[]) => void; placeholder?: string }) {
  const [draft, setDraft] = createSignal("");
  const add = () => {
    const v = draft().trim();
    if (v && !props.values.includes(v)) props.onChange([...props.values, v]);
    setDraft("");
  };
  return (
    <div class="tags">
      <For each={props.values}>
        {(t, i) => (
          <span class="tag">
            <span class="mono">{t}</span>
            <button onClick={() => props.onChange(props.values.filter((_, j) => j !== i()))} aria-label="Remove">
              <X size={11} stroke-width={1.75} />
            </button>
          </span>
        )}
      </For>
      <input
        class="tag-input"
        value={draft()}
        placeholder={props.placeholder}
        onInput={(e) => setDraft(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            add();
          } else if (e.key === "Backspace" && !draft() && props.values.length) props.onChange(props.values.slice(0, -1));
        }}
        onBlur={add}
      />
    </div>
  );
}

export function KeyValue(props: { value: Record<string, string>; onChange: (v: Record<string, string>) => void; keyLabel?: string; valueLabel?: string; secret?: boolean }) {
  const entries = () => Object.entries(props.value);
  const set = (list: [string, string][]) => props.onChange(Object.fromEntries(list));
  return (
    <div class="kv">
      <For each={entries()}>
        {([k, v], i) => (
          <div class="kv-row">
            <input
              class="input mono"
              value={k}
              placeholder={props.keyLabel ?? "Key"}
              onChange={(e) => set(entries().map((x, j) => (j === i() ? [e.currentTarget.value, x[1]] : x)))}
            />
            <input
              class="input mono"
              type={props.secret ? "password" : "text"}
              value={v}
              placeholder={props.valueLabel ?? "Value"}
              onChange={(e) => set(entries().map((x, j) => (j === i() ? [x[0], e.currentTarget.value] : x)))}
            />
            <button class="icon-btn" onClick={() => set(entries().filter((_, j) => j !== i()))} aria-label="Remove">
              <X size={14} stroke-width={1.6} />
            </button>
          </div>
        )}
      </For>
      <button class="btn ghost sm" onClick={() => set([...entries(), [`KEY_${entries().length + 1}`, ""]])}>
        <Plus size={13} stroke-width={1.75} />
        Add
      </button>
    </div>
  );
}

export function StatusDot(props: { status: "ok" | "err" | "idle" | "busy" }) {
  return <span class={`status-dot ${props.status}`} />;
}
