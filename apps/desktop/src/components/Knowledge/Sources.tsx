import { For, Show } from "solid-js";
import { EllipsisVertical, FolderOpen, FolderPlus, RefreshCw, Trash2 } from "lucide-solid";
import { api, openInFileManager, pickFolder } from "../../lib/api";
import { loadKb, openKnowledge, state, toast } from "../../lib/store";
import type { KbLayer, KbSource } from "../../lib/types";
import { Toggle } from "../Settings/controls";
import { menuFor } from "../ui";
import { fmtBytes, fmtCount, LAYERS, progressOf, sources, stageLabel, writable } from "./common";

const pid = () => state.activeProject ?? "";
const err = (e: unknown) => toast(String(e instanceof Error ? e.message : e), "error");

async function addSource(layer: KbLayer) {
  const path = await pickFolder();
  if (!path) return;
  try {
    await api.kbAddSource(pid(), layer, path, "");
    await loadKb();
  } catch (e) {
    err(e);
  }
}

function SourceRow(props: { s: KbSource }) {
  const s = () => props.s;
  const p = () => progressOf(s());
  const pct = () => {
    const x = p();
    return x && x.total ? Math.min(100, (x.done / x.total) * 100) : 0;
  };
  const embedPct = () => {
    const x = p();
    if (x && x.stage === "embed" && x.total) return Math.round((x.done / x.total) * 100);
    return s().chunks ? Math.round((s().embedded / s().chunks) * 100) : 100;
  };
  const more = (e: MouseEvent) =>
    menuFor(
      e.currentTarget as HTMLElement,
      [
        { label: "Browse notes", icon: (i) => <FolderOpen {...i} />, onSelect: () => openKnowledge("notes") },
        { label: "Open folder", icon: (i) => <FolderOpen {...i} />, onSelect: () => openInFileManager(s().path) },
        {
          label: "Reindex",
          icon: (i) => <RefreshCw {...i} />,
          onSelect: () => api.kbReindex(pid(), s().key).catch(err),
        },
        ...(writable(s().layer)
          ? []
          : [
              { type: "separator" as const },
              {
                label: "Remove",
                danger: true,
                icon: (i: { size: number; "stroke-width": number }) => <Trash2 {...i} />,
                onSelect: async () => {
                  await api.kbRemoveSource(pid(), s().key).catch(err);
                  loadKb();
                },
              },
            ]),
      ],
      { align: "right" },
    );
  return (
    <div class="kb-src">
      <div class="kb-src-main">
        <div class="kb-src-name">{s().name}</div>
        <div class="kb-src-path mono" title={s().path}>
          {s().path}
        </div>
        <div class="kb-src-stats">
          <span>{fmtCount(s().notes)} notes</span>
          <span>{fmtBytes(s().bytes)}</span>
          <Show when={s().chunks && embedPct() < 100}>
            <span>{embedPct()}% embedded</span>
          </Show>
        </div>
        <Show when={p()}>
          {(x) => (
            <div class="kb-progress">
              <div class="kb-progress-bar">
                <span style={{ width: `${pct()}%` }} />
              </div>
              <span class="kb-progress-label">
                {stageLabel(x())} {x().total ? `${fmtCount(x().done)} / ${fmtCount(x().total)}` : ""}
              </span>
            </div>
          )}
        </Show>
      </div>
      <Show when={!writable(s().layer)}>
        <span class="kb-src-default" title="On for new chats">
          <Toggle
            value={s().default_on}
            onChange={async (v) => {
              await api.kbUpdateSource(pid(), s().key, null, v).catch(err);
              loadKb();
            }}
          />
        </span>
      </Show>
      <button class="icon-btn" onClick={more} aria-label="More">
        <EllipsisVertical size={15} stroke-width={1.6} />
      </button>
    </div>
  );
}

export default function Sources() {
  return (
    <div class="kb-sources">
      <Show when={!state.kb.overview?.owner}>
        <div class="kb-banner">Read-only: indexed by another Xode window</div>
      </Show>
      <div class="kb-layers">
        <For each={LAYERS}>
          {(l) => {
            const list = () => sources().filter((s) => s.layer === l.id);
            const notes = () => list().reduce((a, s) => a + s.notes, 0);
            return (
              <section class="kb-layer" style={{ "--layer": l.color }}>
                <header class="kb-layer-head">
                  <span class="kb-layer-icon">
                    <l.icon size={16} stroke-width={1.6} />
                  </span>
                  <span class="kb-layer-title">{l.label}</span>
                  <span class="kb-layer-scope">{l.scope}</span>
                  <span class="spacer" />
                  <span class="kb-layer-count">{fmtCount(notes())}</span>
                  <Show when={!writable(l.id)}>
                    <button class="icon-btn" onClick={() => addSource(l.id)} aria-label="Add folder">
                      <FolderPlus size={15} stroke-width={1.6} />
                    </button>
                  </Show>
                </header>
                <div class="kb-layer-body">
                  <Show
                    when={list().length}
                    fallback={
                      <button class="kb-empty-add" onClick={() => addSource(l.id)}>
                        <FolderPlus size={15} stroke-width={1.6} />
                        Add folder
                      </button>
                    }
                  >
                    <For each={list()}>{(s) => <SourceRow s={s} />}</For>
                  </Show>
                </div>
              </section>
            );
          }}
        </For>
      </div>
    </div>
  );
}
