import { createSignal, For, Show } from "solid-js";
import { createStore } from "solid-js/store";
import { ChevronRight, Plus, RefreshCw, Radar, Trash2 } from "lucide-solid";
import { api } from "../../lib/api";
import { toast } from "../../lib/store";
import type { Gateway, GatewayTest } from "../../lib/types";
import { Spinner } from "../ui";
import { cfg, Field, Group, NumberField, Page, patch, Select, StatusDot, TagInput, TextField, Toggle } from "./controls";

type TestState = GatewayTest | "busy" | undefined;

function GatewayCard(props: { g: Gateway; index: number; test: TestState; onTest: () => void }) {
  const [open, setOpen] = createSignal(false);
  const [refreshing, setRefreshing] = createSignal(false);
  const up = (fn: (g: Gateway) => void) => patch((c) => fn(c.gateways[props.index]));
  const status = () => {
    const t = props.test;
    if (t === "busy") return "busy";
    if (!t) return "idle";
    return t.ok ? "ok" : "err";
  };
  const isDefault = () => cfg().selected.gateway === props.g.id;

  const refresh = async () => {
    setRefreshing(true);
    try {
      const g = await api.refreshModels(props.g.id);
      up((x) => (x.models = g.models));
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <div class="gw" classList={{ open: open(), disabled: !props.g.enabled }}>
      <div class="gw-head">
        <button class="gw-toggle" onClick={() => setOpen(!open())}>
          <ChevronRight class="chev" size={14} stroke-width={1.6} />
          <StatusDot status={status()} />
          <span class="gw-name">{props.g.name || props.g.url}</span>
          <span class="badge">{props.g.kind}</span>
          <Show when={props.g.flavor}>
            <span class="badge subtle">{props.g.flavor}</span>
          </Show>
          <span class="gw-url mono">{props.g.url}</span>
        </button>
        <span class="gw-count">{props.g.models.length} models</span>
        <Show when={props.test && props.test !== "busy"}>
          <span class="gw-test" classList={{ err: !(props.test as GatewayTest).ok }}>
            {(props.test as GatewayTest).ok ? `${(props.test as GatewayTest).latency_ms} ms` : (props.test as GatewayTest).message}
          </span>
        </Show>
        <button class="btn sm" onClick={props.onTest} disabled={props.test === "busy"}>
          <Show when={props.test === "busy"} fallback="Test">
            <Spinner size={12} />
          </Show>
        </button>
        <button class="icon-btn" onClick={refresh} aria-label="Refresh models">
          <Show when={refreshing()} fallback={<RefreshCw size={14} stroke-width={1.6} />}>
            <Spinner size={13} />
          </Show>
        </button>
        <Toggle value={props.g.enabled} onChange={(v) => up((x) => (x.enabled = v))} />
        <button class="icon-btn danger" onClick={() => patch((c) => c.gateways.splice(props.index, 1))} aria-label="Remove">
          <Trash2 size={14} stroke-width={1.6} />
        </button>
      </div>
      <Show when={open()}>
        <div class="gw-body">
          <Field label="Name">
            <TextField value={props.g.name} onChange={(v) => up((x) => (x.name = v))} width={260} />
          </Field>
          <Field label="URL">
            <TextField value={props.g.url} onChange={(v) => up((x) => (x.url = v))} mono width={260} />
          </Field>
          <Field label="API key">
            <TextField value={props.g.api_key} onChange={(v) => up((x) => (x.api_key = v))} password width={260} />
          </Field>
          <Field label="Type">
            <Select value={props.g.kind} options={["openai", "anthropic"]} onChange={(v) => up((x) => (x.kind = v as Gateway["kind"]))} width={140} />
          </Field>
          <Field label="Default model">
            <Select
              value={isDefault() ? cfg().selected.model : ""}
              options={[{ value: "", label: "—" }, ...props.g.models.map((m) => m.id)]}
              onChange={(v) =>
                patch((c) => {
                  if (v) {
                    c.selected.gateway = props.g.id;
                    c.selected.model = v;
                  }
                })
              }
              width={260}
            />
          </Field>
          <div class="models">
            <div class="models-head">
              <span>Model</span>
              <span>Context</span>
            </div>
            <For each={props.g.models}>
              {(m, i) => (
                <div class="models-row">
                  <span class="mono">{m.id}</span>
                  <span class="models-ctx">
                    <Show when={m.vision}>
                      <span class="badge subtle">vision</span>
                    </Show>
                    <NumberField value={m.context} onChange={(v) => up((x) => (x.models[i()].context = v ?? 0))} step={1024} width={96} />
                  </span>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}

export default function GatewayPage() {
  const [url, setUrl] = createSignal("");
  const [key, setKey] = createSignal("");
  const [detecting, setDetecting] = createSignal(false);
  const [tests, setTests] = createStore<Record<string, TestState>>({});
  const [scanOpen, setScanOpen] = createSignal(false);
  const [hosts, setHosts] = createSignal<string[]>([]);
  const [scanning, setScanning] = createSignal(false);
  const [found, setFound] = createSignal<Gateway[] | null>(null);

  const addGateway = (g: Gateway) =>
    patch((c) => {
      c.gateways.push(g);
      if (!c.selected.gateway && g.models[0]) {
        c.selected.gateway = g.id;
        c.selected.model = g.models[0].id;
      }
    });

  const detect = async () => {
    if (!url().trim()) return;
    setDetecting(true);
    try {
      const g = await api.detectGateway(url().trim(), key());
      addGateway(g);
      setUrl("");
      setKey("");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setDetecting(false);
    }
  };

  const test = async (g: Gateway) => {
    setTests(g.id, "busy");
    try {
      setTests(g.id, await api.testGateway(JSON.parse(JSON.stringify(g))));
    } catch (e) {
      setTests(g.id, { ok: false, latency_ms: 0, models: 0, message: String(e) });
    }
  };

  const scan = async () => {
    setScanning(true);
    try {
      setFound(await api.scanGateways(hosts()));
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setScanning(false);
    }
  };

  const known = (g: Gateway) => cfg().gateways.some((x) => x.url.replace(/\/+$/, "") === g.url.replace(/\/+$/, ""));

  return (
    <Page
      title="Gateway"
      actions={
        <button class="btn" classList={{ on: scanOpen() }} onClick={() => setScanOpen(!scanOpen())}>
          <Radar size={14} stroke-width={1.6} />
          Scan
        </button>
      }
    >
      <Show when={scanOpen()}>
        <Group title="Scan">
          <Field label="Extra hosts">
            <TagInput values={hosts()} onChange={setHosts} placeholder="192.168.1.20" />
          </Field>
          <div class="field">
            <span class="spacer" />
            <button class="btn primary" onClick={scan} disabled={scanning()}>
              <Show when={scanning()} fallback="Scan">
                <Spinner size={12} />
                Scanning
              </Show>
            </button>
          </div>
          <Show when={found()}>
            <For each={found()!} fallback={<div class="field muted">Nothing found</div>}>
              {(g) => (
                <div class="field scan-row">
                  <span class="gw-name">{g.name || g.flavor}</span>
                  <span class="badge subtle">{g.flavor || g.kind}</span>
                  <span class="gw-url mono">{g.url}</span>
                  <span class="gw-count">{g.models.length} models</span>
                  <span class="spacer" />
                  <button class="btn sm" disabled={known(g)} onClick={() => addGateway(g)}>
                    {known(g) ? "Added" : "Add"}
                  </button>
                </div>
              )}
            </For>
          </Show>
        </Group>
      </Show>

      <Group title="Add gateway">
        <div class="field add-gw">
          <input class="input mono grow" placeholder="http://localhost:8080" value={url()} onInput={(e) => setUrl(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && detect()} />
          <input class="input" type="password" placeholder="API key" value={key()} onInput={(e) => setKey(e.currentTarget.value)} style={{ width: "180px" }} />
          <button class="btn primary" onClick={detect} disabled={detecting() || !url().trim()}>
            <Show when={detecting()} fallback={<Plus size={14} stroke-width={1.75} />}>
              <Spinner size={12} />
            </Show>
            Detect
          </button>
        </div>
      </Group>

      <Group title="Gateways">
        <For each={cfg().gateways} fallback={<div class="field muted">No gateways</div>}>
          {(g, i) => <GatewayCard g={g} index={i()} test={tests[g.id]} onTest={() => test(g)} />}
        </For>
      </Group>
    </Page>
  );
}
