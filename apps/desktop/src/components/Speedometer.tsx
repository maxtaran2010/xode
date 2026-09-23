import { createMemo, For } from "solid-js";
import { fmtTps } from "../lib/format";

const SCALES = [50, 100, 200, 500, 1000, 2000, 5000];
const START = 135; // degrees, 0 = +x axis, clockwise (SVG)
const SWEEP = 270;

function polar(cx: number, cy: number, r: number, deg: number) {
  const a = (deg * Math.PI) / 180;
  return { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) };
}

function arc(cx: number, cy: number, r: number, from: number, to: number) {
  const a = polar(cx, cy, r, from);
  const b = polar(cx, cy, r, to);
  const large = to - from > 180 ? 1 : 0;
  return `M ${a.x} ${a.y} A ${r} ${r} 0 ${large} 1 ${b.x} ${b.y}`;
}

export default function Speedometer(props: { value: number; peak: number; active: boolean }) {
  const cx = 100;
  const cy = 96;
  const r = 74;
  const max = createMemo(() => SCALES.find((s) => s >= Math.max(props.value, props.peak) * 1.1) ?? SCALES[SCALES.length - 1]);
  const frac = () => Math.max(0, Math.min(1, props.value / max()));
  const len = (SWEEP / 360) * 2 * Math.PI * r;
  const ticks = createMemo(() => [0, 0.25, 0.5, 0.75, 1].map((f) => ({ f, v: Math.round(max() * f) })));
  const needle = () => polar(cx, cy, r, START + SWEEP * frac());

  return (
    <div class="speedo" classList={{ idle: !props.active }}>
      <svg viewBox="0 0 200 170" width="100%">
        <path d={arc(cx, cy, r, START, START + SWEEP)} fill="none" stroke="var(--border-strong)" stroke-width="7" stroke-linecap="round" />
        <path
          class="speedo-fill"
          d={arc(cx, cy, r, START, START + SWEEP)}
          fill="none"
          stroke="var(--accent)"
          stroke-width="7"
          stroke-linecap="round"
          stroke-dasharray={`${len}`}
          stroke-dashoffset={`${len * (1 - frac())}`}
        />
        <For each={ticks()}>
          {(t) => {
            const a = START + SWEEP * t.f;
            const p1 = polar(cx, cy, r - 10, a);
            const p2 = polar(cx, cy, r - 14, a);
            const lp = polar(cx, cy, r - 25, a);
            return (
              <>
                <line x1={p1.x} y1={p1.y} x2={p2.x} y2={p2.y} stroke="var(--muted)" stroke-width="1" opacity="0.6" />
                <text x={lp.x} y={lp.y + 3} text-anchor="middle" class="speedo-tick">
                  {t.v}
                </text>
              </>
            );
          }}
        </For>
        <circle class="speedo-dot" visibility={props.value > 0 ? "visible" : "hidden"} cx={needle().x} cy={needle().y} r="4.5" fill="var(--bg)" stroke="var(--accent)" stroke-width="2" />
        <text x={cx} y={cy + 6} text-anchor="middle" class="speedo-value">
          {fmtTps(props.value)}
        </text>
        <text x={cx} y={cy + 24} text-anchor="middle" class="speedo-unit">
          tok/s
        </text>
      </svg>
    </div>
  );
}

export function Sparkline(props: { data: number[] }) {
  const w = 200;
  const h = 32;
  const path = createMemo(() => {
    const d = props.data;
    if (d.length < 2) return "";
    const max = Math.max(...d, 1);
    const step = w / (d.length - 1);
    return d.map((v, i) => `${i ? "L" : "M"}${(i * step).toFixed(1)},${(h - 2 - (v / max) * (h - 4)).toFixed(1)}`).join(" ");
  });
  return (
    <svg class="sparkline" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" width="100%" height={h}>
      <path d={path()} fill="none" stroke="var(--accent)" stroke-width="1.25" vector-effect="non-scaling-stroke" opacity="0.8" />
    </svg>
  );
}
