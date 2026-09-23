import { createResource, createSignal, For, Show } from "solid-js";
import { ChevronRight, Plus, Trash2 } from "lucide-solid";
import { api } from "../../lib/api";
import type { McpServer, McpStatus } from "../../lib/types";
import { Segmented, Spinner } from "../ui";
import { cfg, Field, Group, KeyValue, Page, patch, StatusDot, TagInput, TextField, Toggle } from "./controls";

function ServerCard(props: { s: McpServer; index: number; status?: McpStatus }) {
  const [open, setOpen] = createSignal(false);
  const [tools, setTools] = createSignal<string[] | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal("");
  const up = (fn: (s: McpServer) => void) => patch((c) => fn(c.mcp[props.index]));
  const test = async () => {
    setBusy(true);
    setErr("");
    setOpen(true);
    try {
      setTools(await api.mcpTest(JSON.parse(JSON.stringify(props.s))));
    } catch (e) {
      setErr(String(e));
      setTools(null);
    } finally {
      setBusy(false);
    }
  };
  const dot = () => (err() ? "err" : props.status ? (props.status.connected ? "ok" : props.status.error ? "err" : "idle") : "idle");
  const toolCount = () => (typeof props.status?.tools === "number" ? props.status.tools : Array.isArray(props.status?.tools) ? props.status!.tools.length : null);

  return (
    <div class="gw" classList={{ open: open(), disabled: !props.s.enabled }}>
      <div class="gw-head">
        <button class="gw-toggle" onClick={() => setOpen(!open())}>
          <ChevronRight class="chev" size={14} stroke-width={1.6} />
          <StatusDot status={busy() ? "busy" : dot()} />
          <span class="gw-name">{props.s.name || "unnamed"}</span>
          <span class="badge">{props.s.transport}</span>
          <span class="gw-url mono">{props.s.transport === "http" ? props.s.url : [props.s.command, ...props.s.args].join(" ")}</span>
        </button>
        <Show when={toolCount() != null}>
          <span class="gw-count">{toolCount()} tools</span>
        </Show>
        <button class="btn sm" onClick={test} disabled={busy()}>
          <Show when={busy()} fallback="Test">
            <Spinner size={12} />
          </Show>
        </button>
        <Toggle value={props.s.enabled} onChange={(v) => up((s) => (s.enabled = v))} />
        <button class="icon-btn danger" onClick={() => patch((c) => c.mcp.splice(props.index, 1))} aria-label="Remove">
          <Trash2 size={14} stroke-width={1.6} />
        </button>
      </div>
      <Show when={open()}>
        <div class="gw-body">
          <Field label="Name">
            <TextField value={props.s.name} onChange={(v) => up((s) => (s.name = v))} width={260} />
          </Field>
          <Field label="Transport">
            <Segmented size="sm" value={props.s.transport} options={[{ value: "stdio", label: "stdio" }, { value: "http", label: "http" }]} onChange={(v) => up((s) => (s.transport = v))} />
          </Field>
          <Show
            when={props.s.transport === "http"}
            fallback={
              <>
                <Field label="Command">
                  <TextField value={props.s.command} onChange={(v) => up((s) => (s.command = v))} mono width={260} />
                </Field>
                <Field label="Args">
                  <TagInput values={props.s.args} onChange={(v) => up((s) => (s.args = v))} />
                </Field>
                <Field label="Env" stack>
                  <KeyValue value={props.s.env} onChange={(v) => up((s) => (s.env = v))} secret />
                </Field>
              </>
            }
          >
            <Field label="URL">
              <TextField value={props.s.url} onChange={(v) => up((s) => (s.url = v))} mono width={260} />
            </Field>
            <Field label="Headers" stack>
              <KeyValue value={props.s.headers} onChange={(v) => up((s) => (s.headers = v))} secret />
            </Field>
          </Show>
          <Show when={err() || props.status?.error}>
            <div class="field err-text mono small">{err() || props.status?.error}</div>
          </Show>
          <Show when={tools()}>
            <div class="models">
              <div class="models-head">
                <span>Tool</span>
                <span>Enabled</span>
              </div>
              <For each={tools()!}>
                {(t) => (
                  <div class="models-row">
                    <span class="mono">{t}</span>
                    <Toggle
                      value={!props.s.disabled_tools.includes(t)}
                      onChange={(on) =>
                        up((s) => {
                          s.disabled_tools = on ? s.disabled_tools.filter((x) => x !== t) : [...s.disabled_tools, t];
                        })
                      }
                    />
                  </div>
                )}
              </For>
            </div>
          </Show>
        </div>
      </Show>
    </div>
  );
}

export default function McpPage() {
  const [status] = createResource(() => api.mcpStatus().catch(() => [] as McpStatus[]));
  const add = () =>
    patch((c) =>
      c.mcp.push({ name: `server-${c.mcp.length + 1}`, transport: "stdio", command: "", args: [], env: {}, url: "", headers: {}, enabled: true, disabled_tools: [] }),
    );
  return (
    <Page
      title="MCPs"
      actions={
        <button class="btn" onClick={add}>
          <Plus size={14} stroke-width={1.75} />
          Add
        </button>
      }
    >
      <Group>
        <For each={cfg().mcp} fallback={<div class="field muted">No servers</div>}>
          {(s, i) => <ServerCard s={s} index={i()} status={status()?.find((x) => x.name === s.name)} />}
        </For>
      </Group>
    </Page>
  );
}
