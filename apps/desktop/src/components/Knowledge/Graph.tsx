// Obsidian-style link graph: d3-force layout drawn on a canvas (thousands of nodes stay smooth).
import { createEffect, createMemo, createResource, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation, type SimulationNodeDatum } from "d3-force";
import { FileText, Locate, Search } from "lucide-solid";
import { api } from "../../lib/api";
import { fmtTokens } from "../../lib/format";
import { setKbTab, setState, state } from "../../lib/store";
import type { KbGraphNode, KbLayer } from "../../lib/types";
import { layerOf, LAYERS } from "./common";

const LIMIT = 4000;

interface N extends SimulationNodeDatum, KbGraphNode {
  r: number;
  color: string;
}
interface L {
  source: N;
  target: N;
}

const pid = () => state.activeProject ?? "";

export default function Graph() {
  let wrap!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  const [hidden, setHidden] = createSignal<KbLayer[]>([]);
  const [local, setLocal] = createSignal(false);
  const [selected, setSelected] = createSignal<string | null>(state.kb.note);
  const [hover, setHover] = createSignal<string | null>(null);
  const [find, setFind] = createSignal("");

  const [data] = createResource(
    () => ({ off: hidden() as string[], rev: state.kb.rev, p: pid() }),
    (r) => api.kbGraph(r.p, r.off, LIMIT),
  );

  let nodes: N[] = [];
  let links: L[] = [];
  let byId = new Map<string, N>();
  let adj = new Map<string, Set<string>>();
  let sim: Simulation<N, L> | undefined;
  let view = { x: 0, y: 0, k: 1 };
  let raf = 0;
  let touched = false;
  const [ver, setVer] = createSignal(0);
  let colors = { text: "#aab2c0", muted: "#7d8595", edge: "rgba(255,255,255,0.10)", accent: "#4a8dff", bg: "#13161c" };

  const draw = () => {
    if (raf) return;
    raf = requestAnimationFrame(render);
  };

  /** Nodes to show: everything, or the 2-hop neighbourhood of the selection. */
  const visible = createMemo(() => {
    ver();
    const sel = selected();
    if (!local() || !sel) return null;
    const keep = new Set<string>([sel]);
    for (const a of adj.get(sel) ?? []) {
      keep.add(a);
      for (const b of adj.get(a) ?? []) keep.add(b);
    }
    return keep;
  });

  function build() {
    const g = data();
    if (!g) return;
    const old = new Map(nodes.map((n) => [n.id, n]));
    byId = new Map();
    adj = new Map();
    nodes = g.nodes.map((n) => {
      const prev = old.get(n.id);
      const m: N = { ...n, r: 2.5 + Math.sqrt(n.degree) * 1.7, color: layerOf(n.layer).color, x: prev?.x, y: prev?.y, vx: prev?.vx, vy: prev?.vy };
      byId.set(n.id, m);
      adj.set(n.id, new Set());
      return m;
    });
    links = [];
    for (const [a, b] of g.edges) {
      const s = byId.get(a);
      const t = byId.get(b);
      if (!s || !t) continue;
      links.push({ source: s, target: t });
      adj.get(a)!.add(b);
      adj.get(b)!.add(a);
    }
    sim?.stop();
    const big = nodes.length > 1500;
    sim = forceSimulation<N, L>(nodes)
      .force("charge", forceManyBody<N>().strength(big ? -18 : -60).distanceMax(big ? 250 : 420).theta(big ? 1.0 : 0.9))
      .force("link", forceLink<N, L>(links).distance(big ? 22 : 42).strength(0.6))
      .force("x", forceX<N>(0).strength(0.045))
      .force("y", forceY<N>(0).strength(0.045))
      .force("collide", forceCollide<N>((d) => d.r + 1.5).iterations(1))
      .force("center", forceCenter(0, 0))
      .alphaDecay(big ? 0.045 : 0.028)
      .on("tick", draw)
      .on("end", () => {
        fit();
        draw();
      });
    if (old.size) sim.alpha(0.35);
    else {
      // Settle most of the layout before the first frame, then frame it.
      sim.tick(Math.max(20, Math.min(160, Math.floor(80000 / Math.max(1, nodes.length)))));
      fit();
    }
    setVer((v) => v + 1);
    draw();
  }

  /** Zoom / pan so every node fits the canvas (unless the user already moved the view). */
  function fit() {
    if (touched || !nodes.length || !canvas) return;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) {
      x0 = Math.min(x0, n.x!);
      y0 = Math.min(y0, n.y!);
      x1 = Math.max(x1, n.x!);
      y1 = Math.max(y1, n.y!);
    }
    const w = canvas.clientWidth || 800;
    const h = canvas.clientHeight || 600;
    const k = Math.min(3, Math.max(0.08, 0.8 * Math.min(w / Math.max(40, x1 - x0), h / Math.max(40, y1 - y0))));
    view = { k, x: (-(x0 + x1) / 2) * k, y: (-(y0 + y1) / 2) * k };
  }

  createEffect(on(data, build));
  createEffect(on([visible, selected, hover], draw));

  function render() {
    raf = 0;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    ctx.translate(w / 2 + view.x, h / 2 + view.y);
    ctx.scale(view.k, view.k);

    const vis = visible();
    const focus = hover() ?? selected();
    const near = focus ? adj.get(focus) : undefined;
    const shown = (n: N) => !vis || vis.has(n.id);
    const lit = (n: N) => !focus || n.id === focus || !!near?.has(n.id);

    // Edges.
    ctx.lineWidth = 1 / view.k;
    ctx.strokeStyle = colors.edge;
    ctx.beginPath();
    for (const l of links) {
      if (!shown(l.source) || !shown(l.target)) continue;
      if (focus && (l.source.id === focus || l.target.id === focus)) continue;
      ctx.moveTo(l.source.x!, l.source.y!);
      ctx.lineTo(l.target.x!, l.target.y!);
    }
    ctx.globalAlpha = focus ? 0.35 : 1;
    ctx.stroke();
    ctx.globalAlpha = 1;
    if (focus) {
      ctx.strokeStyle = colors.accent;
      ctx.lineWidth = 1.4 / view.k;
      ctx.beginPath();
      for (const l of links) {
        if (l.source.id !== focus && l.target.id !== focus) continue;
        if (!shown(l.source) || !shown(l.target)) continue;
        ctx.moveTo(l.source.x!, l.source.y!);
        ctx.lineTo(l.target.x!, l.target.y!);
      }
      ctx.globalAlpha = 0.75;
      ctx.stroke();
      ctx.globalAlpha = 1;
    }

    // Nodes.
    for (const n of nodes) {
      if (!shown(n)) continue;
      ctx.globalAlpha = lit(n) ? 1 : 0.18;
      ctx.fillStyle = n.color;
      ctx.beginPath();
      ctx.arc(n.x!, n.y!, n.r, 0, Math.PI * 2);
      ctx.fill();
      if (n.id === selected()) {
        ctx.lineWidth = 2 / view.k;
        ctx.strokeStyle = colors.text;
        ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;

    // Labels: focused neighbourhood always; others once zoomed in enough.
    const fs = 11 / view.k;
    ctx.font = `${fs}px ${getComputedStyle(document.body).fontFamily}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (const n of nodes) {
      if (!shown(n)) continue;
      const important = focus ? lit(n) : n.r * view.k > 7 || view.k > 1.6;
      if (!important) continue;
      ctx.fillStyle = n.id === focus ? colors.text : colors.muted;
      ctx.globalAlpha = focus && !lit(n) ? 0.2 : 1;
      const t = n.title.length > 42 ? `${n.title.slice(0, 40)}…` : n.title;
      ctx.fillText(t, n.x!, n.y! + n.r + 3 / view.k);
    }
    ctx.globalAlpha = 1;
  }

  // ---------- interaction

  const toWorld = (cx: number, cy: number) => {
    const r = canvas.getBoundingClientRect();
    return { x: (cx - r.left - r.width / 2 - view.x) / view.k, y: (cy - r.top - r.height / 2 - view.y) / view.k };
  };
  const pick = (cx: number, cy: number): N | undefined => {
    const p = toWorld(cx, cy);
    const vis = visible();
    let best: N | undefined;
    let bd = Infinity;
    for (const n of nodes) {
      if (vis && !vis.has(n.id)) continue;
      const d = Math.hypot(n.x! - p.x, n.y! - p.y);
      if (d < Math.max(n.r + 3 / view.k, 6 / view.k) && d < bd) {
        best = n;
        bd = d;
      }
    }
    return best;
  };

  let drag: { node?: N; x: number; y: number; moved: boolean } | null = null;
  const onDown = (e: PointerEvent) => {
    touched = true;
    canvas.setPointerCapture(e.pointerId);
    const n = pick(e.clientX, e.clientY);
    drag = { node: n, x: e.clientX, y: e.clientY, moved: false };
    if (n) {
      n.fx = n.x;
      n.fy = n.y;
      sim?.alphaTarget(0.25).restart();
    }
  };
  const onMove = (e: PointerEvent) => {
    if (!drag) {
      const n = pick(e.clientX, e.clientY);
      setHover(n?.id ?? null);
      canvas.style.cursor = n ? "pointer" : "grab";
      return;
    }
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    if (drag.node) {
      const p = toWorld(e.clientX, e.clientY);
      drag.node.fx = p.x;
      drag.node.fy = p.y;
    } else {
      view.x += dx;
      view.y += dy;
      drag.x = e.clientX;
      drag.y = e.clientY;
      canvas.style.cursor = "grabbing";
      draw();
    }
  };
  const onUp = () => {
    if (!drag) return;
    const d = drag;
    drag = null;
    if (d.node) {
      d.node.fx = null;
      d.node.fy = null;
      sim?.alphaTarget(0);
      if (!d.moved) setSelected(d.node.id);
    } else if (!d.moved) setSelected(null);
    canvas.style.cursor = "grab";
  };
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    touched = true;
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left - r.width / 2;
    const my = e.clientY - r.top - r.height / 2;
    const k = Math.min(6, Math.max(0.08, view.k * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015))));
    view.x = mx - ((mx - view.x) * k) / view.k;
    view.y = my - ((my - view.y) * k) / view.k;
    view.k = k;
    draw();
  };
  const openNote = (id: string) => {
    setState("kb", "note", id);
    setKbTab("notes");
  };
  const onDbl = (e: MouseEvent) => {
    const n = pick(e.clientX, e.clientY);
    if (n) openNote(n.id);
  };

  const centerOn = (id: string) => {
    const n = byId.get(id);
    if (!n) return;
    view.k = Math.max(view.k, 1.4);
    view.x = -n.x! * view.k;
    view.y = -n.y! * view.k;
    setSelected(id);
    draw();
  };

  const matches = createMemo(() => {
    const q = find().trim().toLowerCase();
    ver();
    if (!q) return [];
    return nodes.filter((n) => n.title.toLowerCase().includes(q)).slice(0, 8);
  });

  onMount(() => {
    const cs = getComputedStyle(document.documentElement);
    const v = (k: string, d: string) => cs.getPropertyValue(k).trim() || d;
    colors = { text: v("--text", colors.text), muted: v("--text-2", colors.muted), edge: v("--border-strong", colors.edge), accent: v("--accent", colors.accent), bg: v("--bg", colors.bg) };
    const ro = new ResizeObserver(draw);
    ro.observe(wrap);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    onCleanup(() => {
      ro.disconnect();
      canvas.removeEventListener("wheel", onWheel);
      sim?.stop();
      cancelAnimationFrame(raf);
    });
  });

  const sel = () => (ver() && selected() ? byId.get(selected()!) : undefined);

  return (
    <div class="kb-graph" ref={wrap}>
      <canvas
        ref={canvas}
        class="kb-canvas"
        onPointerDown={onDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerLeave={() => setHover(null)}
        onDblClick={onDbl}
      />
      <div class="kb-graph-tools">
        <div class="kb-search sm">
          <Search size={13} stroke-width={1.6} />
          <input class="kb-search-input" value={find()} placeholder="Find" spellcheck={false} onInput={(e) => setFind(e.currentTarget.value)} />
        </div>
        <Show when={matches().length}>
          <div class="kb-find-list">
            <For each={matches()}>
              {(n) => (
                <button
                  class="kb-row"
                  onClick={() => {
                    centerOn(n.id);
                    setFind("");
                  }}
                >
                  <i class="kb-dot" style={{ background: n.color }} />
                  <span class="kb-row-title">{n.title}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
        <div class="kb-chips">
          <For each={LAYERS}>
            {(l) => (
              <button
                class="kb-chip"
                classList={{ on: !hidden().includes(l.id) }}
                onClick={() => setHidden((h) => (h.includes(l.id) ? h.filter((x) => x !== l.id) : [...h, l.id]))}
              >
                <i class="kb-dot" style={{ background: l.color }} />
                {l.label}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="kb-graph-count">
        {data()?.nodes.length ?? 0}
        <Show when={data()?.hidden}> + {data()!.hidden}</Show>
      </div>
      <Show when={sel()}>
        {(n) => (
          <div class="kb-graph-card">
            <div class="kb-hit-head">
              <i class="kb-dot" style={{ background: n().color }} />
              <span class="kb-hit-title">{n().title}</span>
            </div>
            <div class="kb-graph-meta">
              {layerOf(n().layer).label} · {fmtTokens(n().tokens)} tok · {n().degree} links
            </div>
            <div class="kb-graph-actions">
              <button class="btn sm" onClick={() => openNote(n().id)}>
                <FileText size={13} stroke-width={1.6} />
                Open
              </button>
              <button class="btn sm" classList={{ on: local() }} onClick={() => setLocal(!local())}>
                <Locate size={13} stroke-width={1.6} />
                Local
              </button>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
}
