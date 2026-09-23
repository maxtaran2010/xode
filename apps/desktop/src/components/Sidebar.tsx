import { createMemo, createSignal, For, Show } from "solid-js";
import {
  Folder,
  FolderOpen,
  FolderPlus,
  FolderTree,
  Library,
  MessagesSquare,
  Pencil,
  Plus,
  Settings,
  SquarePen,
  Trash2,
  ExternalLink,
} from "lucide-solid";
import { openInFileManager, pickFolder } from "../lib/api";
import {
  addProject,
  currentModel,
  deleteSession,
  newChat,
  openKnowledge,
  openSession,
  openSettings,
  persist,
  removeProject,
  renameSession,
  selectProject,
  setState,
  state,
  updateProject,
} from "../lib/store";
import type { Project, SessionInfo } from "../lib/types";
import { Switcher } from "../lib/motion";
import FileTree from "./FileTree";
import { Logo, menuAt } from "./ui";

function relTime(ms: number): string {
  const d = Date.now() - ms;
  if (d < 60_000) return "now";
  if (d < 3_600_000) return `${Math.floor(d / 60_000)}m`;
  if (d < 86_400_000) return `${Math.floor(d / 3_600_000)}h`;
  if (d < 7 * 86_400_000) return `${Math.floor(d / 86_400_000)}d`;
  return `${Math.floor(d / (7 * 86_400_000))}w`;
}

function InlineEdit(props: { value: string; onDone: (v: string | null) => void }) {
  let ref!: HTMLInputElement;
  queueMicrotask(() => {
    ref?.focus();
    ref?.select();
  });
  const done = (v: string | null) => props.onDone(v && v.trim() ? v.trim() : null);
  return (
    <input
      ref={ref}
      class="inline-edit"
      value={props.value}
      onKeyDown={(e) => {
        if (e.key === "Enter") done(e.currentTarget.value);
        if (e.key === "Escape") done(null);
      }}
      onBlur={(e) => done(e.currentTarget.value)}
      onClick={(e) => e.stopPropagation()}
    />
  );
}

function ChatRow(props: { s: SessionInfo }) {
  const [editing, setEditing] = createSignal(false);
  const running = () => !!state.live[props.s.id]?.running;
  return (
    <div
      class="sb-row sb-chat"
      classList={{ active: state.activeSession === props.s.id }}
      onClick={() => openSession(props.s.id)}
      onContextMenu={(e) =>
        menuAt(e, [
          { label: "Rename", icon: Pencil, onSelect: () => setEditing(true) },
          { label: "Delete", icon: Trash2, danger: true, onSelect: () => deleteSession(props.s.id) },
        ])
      }
    >
      <Show when={editing()} fallback={<span class="sb-label">{props.s.title || "New chat"}</span>}>
        <InlineEdit
          value={props.s.title}
          onDone={(v) => {
            setEditing(false);
            if (v && v !== props.s.title) renameSession(props.s.id, v);
          }}
        />
      </Show>
      <Show when={running()} fallback={<span class="sb-time">{relTime(props.s.updated_at)}</span>}>
        <span class="run-dot" />
      </Show>
    </div>
  );
}

function ProjectGroup(props: { p: Project }) {
  const [open, setOpen] = createSignal(true);
  const [all, setAll] = createSignal(false);
  const [editing, setEditing] = createSignal(false);
  const chats = createMemo(() => state.sessions.filter((s) => s.project_id === props.p.id).sort((a, b) => b.updated_at - a.updated_at));
  const shown = () => (all() ? chats() : chats().slice(0, 5));
  const active = () => state.activeProject === props.p.id;

  const addFolder = async () => {
    const dir = await pickFolder();
    if (dir && !props.p.extra_roots.includes(dir)) updateProject({ ...props.p, extra_roots: [...props.p.extra_roots, dir] });
  };

  return (
    <div class="sb-group">
      <div
        class="sb-row sb-project"
        classList={{ current: active() }}
        onClick={() => {
          if (active()) setOpen(!open());
          else {
            selectProject(props.p.id);
            setOpen(true);
          }
        }}
        onContextMenu={(e) =>
          menuAt(e, [
            { label: "New chat", icon: SquarePen, onSelect: () => newChat(props.p.id) },
            { label: "Rename", icon: Pencil, onSelect: () => setEditing(true) },
            { label: "Open in file manager", icon: ExternalLink, onSelect: () => openInFileManager(props.p.root) },
            { label: "Add folder", icon: FolderPlus, onSelect: addFolder },
            { type: "separator" },
            { label: "Remove", icon: Trash2, danger: true, onSelect: () => removeProject(props.p.id) },
          ])
        }
      >
        <span class="sb-icon">
          <Show when={open()} fallback={<Folder size={15} stroke-width={1.6} />}>
            <FolderOpen size={15} stroke-width={1.6} />
          </Show>
        </span>
        <Show when={editing()} fallback={<span class="sb-label">{props.p.name}</span>}>
          <InlineEdit
            value={props.p.name}
            onDone={(v) => {
              setEditing(false);
              if (v && v !== props.p.name) updateProject({ ...props.p, name: v });
            }}
          />
        </Show>
        <button
          class="sb-hover-btn"
          onClick={(e) => {
            e.stopPropagation();
            newChat(props.p.id);
          }}
          aria-label="New chat"
        >
          <SquarePen size={14} stroke-width={1.6} />
        </button>
      </div>
      <Show when={open()}>
        <For each={shown()}>{(s) => <ChatRow s={s} />}</For>
        <Show when={chats().length > 5}>
          <button class="sb-row sb-more" onClick={() => setAll(!all())}>
            {all() ? "Show less" : "Show more"}
          </button>
        </Show>
      </Show>
    </div>
  );
}

export default function Sidebar() {
  const kbNotes = () => (state.kb.overview?.sources ?? []).reduce((a, x) => a + x.notes, 0);
  const kbBusy = () => Object.values(state.kb.progress).some((p) => p.stage !== "idle");
  const project = () => state.projects.find((p) => p.id === state.activeProject);
  const add = async () => {
    const dir = await pickFolder();
    if (dir) addProject(dir);
  };
  const setView = (v: "chats" | "files") => setState("ui", "sidebarView", v);
  const model = () => currentModel();

  return (
    <div class="sb">
      <div class="sb-top">
        <div class="sb-brand">
          <Logo size={18} />
          <span>Xode</span>
        </div>
        <button class="sb-row sb-new" classList={{ active: !state.activeSession }} onClick={() => newChat()}>
          <span class="sb-icon">
            <SquarePen size={15} stroke-width={1.6} />
          </span>
          <span class="sb-label">New chat</span>
        </button>
      </div>

      <div class="sb-head">
        <span>{state.ui.sidebarView === "chats" ? "Projects" : project()?.name ?? "Files"}</span>
        <div class="sb-head-actions">
          <button class="icon-btn xs" classList={{ on: state.ui.sidebarView === "chats" }} onClick={() => setView("chats")} aria-label="Chats">
            <MessagesSquare size={14} stroke-width={1.6} />
          </button>
          <button class="icon-btn xs" classList={{ on: state.ui.sidebarView === "files" }} onClick={() => setView("files")} aria-label="Files">
            <FolderTree size={14} stroke-width={1.6} />
          </button>
          <button class="icon-btn xs" onClick={add} aria-label="Add project">
            <Plus size={15} stroke-width={1.6} />
          </button>
        </div>
      </div>

      <div class="sb-scroll">
        <Switcher key={state.ui.sidebarView} motion="side">
          <Show
            when={state.ui.sidebarView === "chats"}
            fallback={
              <Show when={project()} keyed>
                {(p) => <FileTree project={p} />}
              </Show>
            }
          >
            <div class="sb-list">
              <For each={state.projects}>{(p) => <ProjectGroup p={p} />}</For>
            </div>
          </Show>
        </Switcher>
      </div>

      <div class="sb-bottom">
        <Show when={state.config?.knowledge?.enabled}>
          <button class="sb-row sb-settings" onClick={() => openKnowledge()}>
            <span class="sb-icon">
              <Library size={15} stroke-width={1.6} />
            </span>
            <span class="sb-label">Knowledge</span>
            <span class="sb-model" classList={{ "kb-busy": kbBusy() }}>
              {kbNotes() ? kbNotes().toLocaleString() : ""}
            </span>
          </button>
        </Show>
        <button class="sb-row sb-settings" onClick={() => openSettings()}>
          <span class="sb-icon">
            <Settings size={15} stroke-width={1.6} />
          </span>
          <span class="sb-label">Settings</span>
          <span class="sb-model">{model().model || model().gatewayName}</span>
        </button>
      </div>
    </div>
  );
}

export function SidebarResizer() {
  let startX = 0;
  let startW = 0;
  const move = (e: PointerEvent) => {
    const w = Math.max(200, Math.min(420, startW + e.clientX - startX));
    setState("ui", "sidebarWidth", w);
  };
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    document.body.classList.remove("resizing");
    persist("sidebarWidth", state.ui.sidebarWidth);
  };
  return (
    <div
      class="resizer"
      onPointerDown={(e) => {
        startX = e.clientX;
        startW = state.ui.sidebarWidth;
        document.body.classList.add("resizing");
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
      }}
    />
  );
}
