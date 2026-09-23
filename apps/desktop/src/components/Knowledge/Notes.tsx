import { createEffect, createMemo, createResource, createSignal, For, on, Show } from "solid-js";
import { ArrowLeft, ChevronRight, ExternalLink, FileText, Folder, Pencil, Plus, Search, Trash2, Waypoints, X } from "lucide-solid";
import { api, openInFileManager } from "../../lib/api";
import { debounce, fmtTokens } from "../../lib/format";
import { renderMarkdown } from "../../lib/markdown";
import { loadKb, setKbTab, setState, state, toast } from "../../lib/store";
import type { KbHit, KbLayer, KbNote } from "../../lib/types";
import { Switcher } from "../../lib/motion";
import { menuFor } from "../ui";
import { layerOf, LAYERS, sourceOf, sources } from "./common";

const pid = () => state.activeProject ?? "";
const err = (e: unknown) => toast(String(e instanceof Error ? e.message : e), "error");

/** Strip front matter and turn [[wiki links]] into clickable note links. */
function noteHtml(text: string): string {
  let t = text.replace(/^---\n[\s\S]*?\n(---|\.\.\.)\n/, "");
  t = t.replace(/!?\[\[([^\]|#\n]+)(#[^\]|\n]*)?(?:\|([^\]\n]*))?\]\]/g, (_m, target: string, _h, alias?: string) => {
    const label = (alias || target).replace(/[[\]]/g, "");
    return `[${label}](xode-note:${encodeURIComponent(target.trim())})`;
  });
  return renderMarkdown(t);
}

function LayerDot(props: { layer: KbLayer }) {
  return <i class="kb-dot" style={{ background: layerOf(props.layer).color }} />;
}

export default function Notes() {
  const [q, setQ] = createSignal("");
  const [query, setQuery] = createSignal("");
  const [layers, setLayers] = createSignal<KbLayer[]>([]);
  const [src, setSrc] = createSignal<string | null>(null);
  const [dir, setDir] = createSignal("");
  const [editing, setEditing] = createSignal<string | null>(null);
  const debounced = debounce((v: string) => setQuery(v.trim()), 250);

  const noteId = () => state.kb.note;
  const openNote = (id: string | null) => {
    setEditing(null);
    setState("kb", "note", id);
  };

  const off = () => LAYERS.filter((l) => layers().length && !layers().includes(l.id)).map((l) => l.id as string);
  const [hits] = createResource(
    () => (query() ? { q: query(), off: off(), rev: state.kb.rev } : null),
    (r) => (r ? api.kbSearch(pid(), { q: r.q, off: r.off, k: 40 }) : Promise.resolve([] as KbHit[])),
  );
  const [folder] = createResource(
    () => (src() ? { key: src()!, dir: dir(), rev: state.kb.rev } : null),
    (r) => api.kbList(pid(), r.key, r.dir, ""),
  );
  const [note, { refetch: refetchNote, mutate: setNote }] = createResource(
    () => (noteId() ? { id: noteId()!, rev: state.kb.rev } : null),
    (r) => api.kbNote(pid(), r.id).catch((e) => {
      err(e);
      return null;
    }),
  );
  createEffect(on(() => state.activeProject, () => openNote(null), { defer: true }));

  const toggleLayer = (l: KbLayer) => setLayers((cur) => (cur.includes(l) ? cur.filter((x) => x !== l) : [...cur, l]));

  const newNote = (e: MouseEvent) =>
    menuFor(
      e.currentTarget as HTMLElement,
      [
        { type: "header", label: "New note in" },
        { label: "Project memory", onSelect: () => create(false) },
        { label: "Memory", onSelect: () => create(true) },
      ],
      { align: "right" },
    );
  const create = async (global: boolean) => {
    try {
      const id = await api.kbCreateNote(pid(), global, "Untitled", "");
      await loadKb();
      openNote(id);
      setEditing(id);
    } catch (e) {
      err(e);
    }
  };

  const onBodyClick = async (e: MouseEvent) => {
    const a = (e.target as HTMLElement).closest("a") as HTMLAnchorElement | null;
    if (!a) return;
    const href = a.getAttribute("href") ?? "";
    e.preventDefault();
    if (!href.startsWith("xode-note:")) {
      if (/^https?:/.test(href)) window.open(href, "_blank");
      return;
    }
    const target = decodeURIComponent(href.slice("xode-note:".length)).toLowerCase();
    const n = note();
    const hit = n?.links.find((l) => l.id && l.title.toLowerCase() === target);
    if (hit?.id) return openNote(hit.id);
    const found = await api.kbTitles(pid(), target).catch(() => []);
    const exact = found.find(([, t]) => t.toLowerCase() === target) ?? found[0];
    if (exact) openNote(exact[0]);
    else toast(`No note "${target}"`);
  };

  const crumbs = createMemo(() => (dir() ? dir().split("/") : []));

  return (
    <div class="kb-notes">
      <aside class="kb-nav">
        <div class="kb-search">
          <Search size={14} stroke-width={1.6} />
          <input
            class="kb-search-input"
            value={q()}
            placeholder="Search notes"
            spellcheck={false}
            onInput={(e) => {
              setQ(e.currentTarget.value);
              debounced(e.currentTarget.value);
            }}
          />
          <Show when={q()}>
            <button
              class="icon-btn sm"
              onClick={() => {
                setQ("");
                setQuery("");
              }}
              aria-label="Clear"
            >
              <X size={13} stroke-width={1.6} />
            </button>
          </Show>
          <button class="icon-btn sm" onClick={newNote} aria-label="New note">
            <Plus size={15} stroke-width={1.6} />
          </button>
        </div>
        <div class="kb-chips">
          <For each={LAYERS}>
            {(l) => (
              <button class="kb-chip" classList={{ on: layers().includes(l.id) }} onClick={() => toggleLayer(l.id)}>
                <i class="kb-dot" style={{ background: l.color }} />
                {l.label}
              </button>
            )}
          </For>
        </div>
        <div class="kb-nav-list">
          <Show
            when={query()}
            fallback={
              <Show
                when={src()}
                fallback={
                  <For each={sources().filter((s) => !layers().length || layers().includes(s.layer))}>
                    {(s) => (
                      <button class="kb-row" onClick={() => (setSrc(s.key), setDir(""))}>
                        <LayerDot layer={s.layer} />
                        <span class="kb-row-title">{s.name}</span>
                        <span class="kb-row-meta">{s.notes}</span>
                        <ChevronRight size={14} stroke-width={1.6} class="kb-row-chev" />
                      </button>
                    )}
                  </For>
                }
              >
                <div class="kb-crumbs">
                  <button class="icon-btn sm" onClick={() => (dir() ? setDir(crumbs().slice(0, -1).join("/")) : setSrc(null))} aria-label="Back">
                    <ArrowLeft size={14} stroke-width={1.6} />
                  </button>
                  <button class="kb-crumb" onClick={() => setDir("")}>
                    {sourceOf(src()!)?.name}
                  </button>
                  <For each={crumbs()}>
                    {(c, i) => (
                      <>
                        <span class="kb-crumb-sep">/</span>
                        <button class="kb-crumb" onClick={() => setDir(crumbs().slice(0, i() + 1).join("/"))}>
                          {c}
                        </button>
                      </>
                    )}
                  </For>
                </div>
                <For each={folder()?.folders ?? []}>
                  {([f, n]) => (
                    <button class="kb-row" onClick={() => setDir(f)}>
                      <Folder size={14} stroke-width={1.6} class="kb-row-icon" />
                      <span class="kb-row-title">{f.split("/").pop()}</span>
                      <span class="kb-row-meta">{n}</span>
                    </button>
                  )}
                </For>
                <For each={folder()?.notes ?? []}>
                  {(n) => (
                    <button class="kb-row" classList={{ active: noteId() === n.id }} onClick={() => openNote(n.id)}>
                      <FileText size={14} stroke-width={1.6} class="kb-row-icon" />
                      <span class="kb-row-title">{n.title}</span>
                      <span class="kb-row-meta">{fmtTokens(n.tokens)}</span>
                    </button>
                  )}
                </For>
              </Show>
            }
          >
            <For each={hits() ?? []} fallback={<div class="kb-none">{hits.loading ? "" : "No matches"}</div>}>
              {(h) => (
                <button class="kb-hit" classList={{ active: noteId() === h.id }} onClick={() => openNote(h.id)}>
                  <div class="kb-hit-head">
                    <LayerDot layer={h.layer} />
                    <span class="kb-hit-title">{h.title}</span>
                    <span class="kb-row-meta">{fmtTokens(h.note_tokens)}</span>
                  </div>
                  <Show when={h.heading}>
                    <div class="kb-hit-heading">{h.heading}</div>
                  </Show>
                  <div class="kb-hit-snippet">{h.snippet}</div>
                </button>
              )}
            </For>
          </Show>
        </div>
      </aside>
      <section class="kb-reader">
        <Show when={note()} fallback={<div class="ov-empty">{noteId() ? "" : ""}</div>}>
          {(n) => (
            <Switcher key={n().id} motion="fade">
              <NoteView
                n={n()}
                editing={editing() === n().id}
                onEdit={() => setEditing(n().id)}
                onCancel={() => setEditing(null)}
                onSaved={(text) => {
                  setNote({ ...n(), text });
                  setEditing(null);
                  refetchNote();
                  loadKb();
                }}
                onDeleted={() => {
                  openNote(null);
                  loadKb();
                }}
                onOpen={openNote}
                onBodyClick={onBodyClick}
              />
            </Switcher>
          )}
        </Show>
      </section>
    </div>
  );
}

function NoteView(props: {
  n: KbNote;
  editing: boolean;
  onEdit: () => void;
  onCancel: () => void;
  onSaved: (text: string) => void;
  onDeleted: () => void;
  onOpen: (id: string) => void;
  onBodyClick: (e: MouseEvent) => void;
}) {
  const [draft, setDraft] = createSignal(props.n.text);
  createEffect(on(() => props.editing, (e) => e && setDraft(props.n.text)));
  const save = async () => {
    try {
      await api.kbSaveNote(pid(), props.n.id, draft());
      props.onSaved(draft());
    } catch (e) {
      err(e);
    }
  };
  const del = async () => {
    try {
      await api.kbDeleteNote(pid(), props.n.id);
      props.onDeleted();
    } catch (e) {
      err(e);
    }
  };
  const html = createMemo(() => noteHtml(props.n.text));
  return (
    <div class="kb-note">
      <div class="kb-note-bar">
        <LayerDot layer={props.n.layer} />
        <span class="kb-note-path mono">
          {props.n.source_name} / {props.n.rel}
        </span>
        <span class="spacer" />
        <Show
          when={props.editing}
          fallback={
            <>
              <span class="kb-note-tok">{fmtTokens(props.n.tokens)} tok</span>
              <button class="icon-btn" onClick={() => (setState("kb", "note", props.n.id), setKbTab("graph"))} aria-label="Show in graph">
                <Waypoints size={15} stroke-width={1.6} />
              </button>
              <button class="icon-btn" onClick={() => openInFileManager(props.n.abs)} aria-label="Open file">
                <ExternalLink size={15} stroke-width={1.6} />
              </button>
              <Show when={props.n.writable}>
                <button class="icon-btn" onClick={props.onEdit} aria-label="Edit">
                  <Pencil size={15} stroke-width={1.6} />
                </button>
                <button class="icon-btn" onClick={del} aria-label="Delete">
                  <Trash2 size={15} stroke-width={1.6} />
                </button>
              </Show>
            </>
          }
        >
          <button class="btn ghost sm" onClick={props.onCancel}>
            Cancel
          </button>
          <button class="btn primary sm" onClick={save}>
            Save
          </button>
        </Show>
      </div>
      <Show
        when={props.editing}
        fallback={
          <div class="kb-note-scroll">
            <Show when={props.n.tags.length}>
              <div class="kb-tags">
                <For each={props.n.tags}>{(t) => <span class="kb-tag">#{t}</span>}</For>
              </div>
            </Show>
            <div class="md kb-md" innerHTML={html()} onClick={props.onBodyClick} />
            <Show when={props.n.links.length || props.n.backlinks.length}>
              <div class="kb-links">
                <Show when={props.n.links.length}>
                  <div class="kb-links-col">
                    <div class="kv-label">Links</div>
                    <For each={props.n.links}>
                      {(l) => (
                        <button class="kb-link" classList={{ missing: !l.id }} disabled={!l.id} onClick={() => l.id && props.onOpen(l.id)}>
                          {l.title}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={props.n.backlinks.length}>
                  <div class="kb-links-col">
                    <div class="kv-label">Backlinks</div>
                    <For each={props.n.backlinks}>
                      {(l) => (
                        <button class="kb-link" onClick={() => l.id && props.onOpen(l.id)}>
                          {l.title}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
              </div>
            </Show>
          </div>
        }
      >
        <textarea
          class="kb-editor mono"
          value={draft()}
          spellcheck={false}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "s") {
              e.preventDefault();
              save();
            }
          }}
          ref={(el) => requestAnimationFrame(() => el.focus())}
        />
      </Show>
    </div>
  );
}
