import { Show } from "solid-js";
import { api } from "../../lib/api";
import { loadKb, openKnowledge, state } from "../../lib/store";
import type { Knowledge } from "../../lib/types";
import { Segmented } from "../ui";
import { cfg, Field, Group, NumberField, Page, patch, Select, StatusDot, TextField, Toggle } from "./controls";

const MODELS = [
  { value: "multilingual-e5-small", label: "Multilingual E5 small" },
  { value: "multilingual-e5-base", label: "Multilingual E5 base" },
  { value: "paraphrase-multilingual-minilm", label: "Paraphrase multilingual MiniLM" },
  { value: "bge-small-en", label: "BGE small EN" },
];

export default function KnowledgePage() {
  const k = () => cfg().knowledge;
  const set = <K extends keyof Knowledge>(key: K, v: Knowledge[K]) => patch((c) => (c.knowledge[key] = v));
  const embed = () => state.kb.overview?.embed;
  const dot = () => {
    const s = embed()?.state;
    return s === "ready" ? "ok" : s === "error" ? "err" : s === "loading" ? "busy" : "idle";
  };
  const retry = async () => {
    await api.kbEmbedRetry().catch(() => {});
    setTimeout(loadKb, 400);
  };
  return (
    <Page
      title="Knowledge"
      actions={
        <button class="btn sm" onClick={() => openKnowledge("sources")}>
          Open
        </button>
      }
    >
      <Group>
        <Field label="Enabled">
          <Toggle value={k().enabled} onChange={(v) => set("enabled", v)} />
        </Field>
      </Group>
      <Group title="Embeddings">
        <Field label="Source">
          <Segmented
            size="sm"
            value={k().embedder}
            options={[
              { value: "builtin", label: "Built-in" },
              { value: "gateway", label: "Gateway" },
              { value: "off", label: "Off" },
            ]}
            onChange={(v) => set("embedder", v)}
          />
        </Field>
        <Show when={k().embedder === "builtin"}>
          <Field label="Model">
            <Select value={k().builtin_model} options={MODELS} onChange={(v) => set("builtin_model", v)} width={260} />
          </Field>
        </Show>
        <Show when={k().embedder === "gateway"}>
          <Field label="Gateway">
            <Select
              value={k().gateway}
              options={[{ value: "", label: "Active gateway" }, ...cfg().gateways.map((g) => ({ value: g.id, label: g.name }))]}
              onChange={(v) => set("gateway", v)}
              width={260}
            />
          </Field>
          <Field label="Model">
            <TextField value={k().gateway_model} onChange={(v) => set("gateway_model", v)} placeholder="nomic-embed-text" mono width={260} />
          </Field>
        </Show>
        <Show when={k().embedder !== "off"}>
          <Field label="Status">
            <span class="kb-embed-status">
              <StatusDot status={dot()} />
              <span class="mono small">{embed()?.state === "error" ? embed()?.error : embed()?.model || embed()?.state || "—"}</span>
              <button class="btn ghost sm" onClick={retry}>
                Retry
              </button>
            </span>
          </Field>
        </Show>
      </Group>
      <Group title="Retrieval">
        <Field label="Results per search">
          <NumberField value={k().k} min={1} max={30} onChange={(v) => v != null && set("k", v)} />
        </Field>
        <Field label="Chunk size">
          <NumberField value={k().chunk_tokens} min={100} max={2000} step={50} onChange={(v) => v != null && set("chunk_tokens", v)} />
        </Field>
        <Field label="Outline notes above">
          <NumberField value={k().outline_tokens} min={100} max={8000} step={100} onChange={(v) => v != null && set("outline_tokens", v)} />
        </Field>
        <Field label="Max tokens per read">
          <NumberField value={k().read_max_tokens} min={300} max={16000} step={100} onChange={(v) => v != null && set("read_max_tokens", v)} />
        </Field>
      </Group>
      <Group title="Agent writes">
        <Field label="Memory">
          <Toggle value={k().ai_write_global} onChange={(v) => set("ai_write_global", v)} />
        </Field>
        <Field label="Project memory">
          <Toggle value={k().ai_write_project} onChange={(v) => set("ai_write_project", v)} />
        </Field>
      </Group>
    </Page>
  );
}
