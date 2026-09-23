import { createMemo, For, Show, type JSX } from "solid-js";
import { fmtMs, fmtTokens, fmtTps } from "../lib/format";
import { activeLive, currentModel } from "../lib/store";
import Speedometer, { Sparkline } from "./Speedometer";

function Row(props: { label: string; children: JSX.Element }) {
  return (
    <div class="stat-row">
      <span class="stat-label">{props.label}</span>
      <span class="stat-value">{props.children}</span>
    </div>
  );
}

export default function StatsPanel() {
  const l = () => activeLive();
  const st = () => l()?.stats;
  const hist = () => l()?.tpsHistory ?? [];
  const peak = createMemo(() => Math.max(0, ...hist()));
  const ctxFrac = () => {
    const s = st();
    return s && s.context_limit ? Math.min(1, s.context_used / s.context_limit) : 0;
  };
  const cm = () => currentModel();

  return (
    <div class="stats">
      <Speedometer value={st()?.tps ?? 0} peak={peak()} active={!!l()?.running} />
      <div class="spark-wrap">
        <Show when={hist().length > 1} fallback={<div class="spark-empty" />}>
          <Sparkline data={hist()} />
        </Show>
      </div>
      <div class="stat-list">
        <For
          each={[
            ["TTFT", () => (st()?.ttft_ms ? fmtMs(st()!.ttft_ms) : "—")],
            ["Prefill", () => (st()?.prefill_tps ? `${fmtTps(st()!.prefill_tps)} tok/s` : "—")],
            ["Tokens in", () => fmtTokens(st()?.tokens_in ?? 0)],
            ["Tokens out", () => fmtTokens(st()?.tokens_out ?? 0)],
          ] as [string, () => string][]}
        >
          {([label, v]) => <Row label={label}>{v()}</Row>}
        </For>
        <div class="stat-row stat-ctx">
          <span class="stat-label">Context</span>
          <span class="stat-value">
            {fmtTokens(st()?.context_used ?? 0)} / {fmtTokens(st()?.context_limit || cm().context)}
          </span>
          <div class="bar">
            <div class="bar-fill" style={{ width: `${ctxFrac() * 100}%` }} classList={{ warn: ctxFrac() > 0.75 }} />
          </div>
        </div>
        <div class="stat-sep" />
        <Row label="Compactions">{st()?.compactions ?? 0}</Row>
        <Row label="Steps">{st()?.steps ?? 0}</Row>
        <Row label="Tool calls">{st()?.tool_calls ?? 0}</Row>
        <Row label="Elapsed">{st()?.elapsed_ms ? fmtMs(st()!.elapsed_ms) : "—"}</Row>
        <Row label="Work time">{st()?.total_work_ms ? fmtMs(st()!.total_work_ms) : "—"}</Row>
        <Row label="RTK saved">{fmtTokens(st()?.rtk_saved ?? 0)}</Row>
        <div class="stat-sep" />
        <Row label="Model">{st()?.model || cm().model || "—"}</Row>
        <Row label="Gateway">{st()?.gateway || cm().gatewayName || "—"}</Row>
      </div>
    </div>
  );
}
