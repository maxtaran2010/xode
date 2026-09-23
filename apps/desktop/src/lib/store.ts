// Global app state (Solid store) + actions. Engine events are folded into per-session live state.
import { batch } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { api, notify, saveText, setWindowEffect, windowEffect } from "./api";
import { debounce } from "./format";
import { applyEvent, emptyLive, messagesToItems, type SessionLive } from "./session";
import { applyTheme } from "./theme";
import type { AgentEvent, Attachment, CommandInfo, CommandResult, Config, KbOverview, KbProgress, Mode, PermDecision, Project, SessionInfo } from "./types";

export type Overlay = null | "settings" | "context" | "knowledge";
export type SettingsPage =
  | "gateway"
  | "tokens"
  | "permissions"
  | "generation"
  | "compaction"
  | "knowledge"
  | "browser"
  | "mcp"
  | "tools"
  | "notifications"
  | "theme";
export type KbTab = "sources" | "notes" | "graph";

export interface Toast {
  id: number;
  text: string;
  kind: "info" | "error";
  leaving?: boolean;
}

interface State {
  ready: boolean;
  config: Config | null;
  backdrop: string;
  projects: Project[];
  sessions: SessionInfo[];
  activeProject: string | null;
  activeSession: string | null;
  live: Record<string, SessionLive>;
  commands: CommandInfo[];
  kb: {
    overview: KbOverview | null;
    /** Latest indexing progress per source key / store prefix. */
    progress: Record<string, KbProgress>;
    /** Bumped when notes change (lists / graph refetch). */
    rev: number;
    tab: KbTab;
    /** Note to open in the Notes tab. */
    note: string | null;
    /** Selection for a chat that does not exist yet. */
    pendingOff: string[] | null;
  };
  ui: {
    sidebar: boolean;
    stats: boolean;
    sidebarWidth: number;
    sidebarView: "chats" | "files";
    overlay: Overlay;
    settingsPage: SettingsPage;
    composerText: string;
    attachments: Attachment[];
    toasts: Toast[];
    dragging: boolean;
  };
}

const saved = <T>(key: string, def: T): T => {
  try {
    const v = localStorage.getItem(`xode.${key}`);
    return v == null ? def : (JSON.parse(v) as T);
  } catch {
    return def;
  }
};
export const persist = (key: string, v: unknown) => {
  try {
    localStorage.setItem(`xode.${key}`, JSON.stringify(v));
  } catch {
    /* storage unavailable */
  }
};

export const [state, setState] = createStore<State>({
  ready: false,
  config: null,
  backdrop: "none",
  projects: [],
  sessions: [],
  activeProject: null,
  activeSession: null,
  live: {},
  commands: [],
  kb: { overview: null, progress: {}, rev: 0, tab: saved<KbTab>("kbTab", "sources"), note: null, pendingOff: null },
  ui: {
    sidebar: saved("sidebar", true),
    stats: saved("stats", true),
    sidebarWidth: saved("sidebarWidth", 264),
    sidebarView: "chats",
    overlay: null,
    settingsPage: "gateway",
    composerText: "",
    attachments: [],
    toasts: [],
    dragging: false,
  },
});

// ---------- derived helpers

export const activeProject = () => state.projects.find((p) => p.id === state.activeProject) ?? null;
export const activeSessionInfo = () => state.sessions.find((s) => s.id === state.activeSession) ?? null;
export const live = (sid: string | null) => (sid ? state.live[sid] : undefined);
export const activeLive = () => live(state.activeSession);

// ---------- toasts

let toastId = 0;
export function toast(text: string, kind: Toast["kind"] = "info") {
  const id = ++toastId;
  setState("ui", "toasts", (t) => [...t, { id, text, kind }]);
  setTimeout(() => {
    setState("ui", "toasts", (t) => t.id === id, "leaving", true);
    setTimeout(() => setState("ui", "toasts", (t) => t.filter((x) => x.id !== id)), 160);
  }, kind === "error" ? 6000 : 3500);
}
const fail = (e: unknown) => toast(String(e instanceof Error ? e.message : e), "error");

// ---------- event folding with rAF batching of deltas

let queue: AgentEvent[] = [];
let scheduled = false;

function ensureLive(sid: string) {
  if (!state.live[sid]) setState("live", sid, emptyLive());
}

function flush() {
  scheduled = false;
  if (!queue.length) return;
  const evs = queue;
  queue = [];
  batch(() => {
    const bySession = new Map<string, AgentEvent[]>();
    for (const ev of evs) {
      const list = bySession.get(ev.session) ?? [];
      list.push(ev);
      bySession.set(ev.session, list);
    }
    for (const [sid, list] of bySession) {
      ensureLive(sid);
      setState(
        "live",
        sid,
        produce((s) => {
          for (const ev of coalesce(list)) applyEvent(s, ev);
        }),
      );
    }
  });
}

/** Merges consecutive text/thinking deltas for the same turn into one event. */
function coalesce(list: AgentEvent[]): AgentEvent[] {
  const out: AgentEvent[] = [];
  for (const ev of list) {
    const prev = out[out.length - 1];
    if (
      prev &&
      (ev.type === "text_delta" || ev.type === "thinking_delta") &&
      prev.type === ev.type &&
      prev.id === ev.id
    ) {
      out[out.length - 1] = { ...prev, text: prev.text + ev.text };
    } else out.push(ev);
  }
  return out;
}

function onEvent(ev: AgentEvent) {
  if (ev.type === "kb_progress") {
    onKbProgress(ev);
    return;
  }
  queue.push(ev);
  const isDelta = ev.type === "text_delta" || ev.type === "thinking_delta" || ev.type === "stats";
  if (!isDelta) {
    flush();
    afterEvent(ev);
    return;
  }
  if (!scheduled) {
    scheduled = true;
    requestAnimationFrame(flush);
  }
}

const refreshSessions = debounce(async () => {
  try {
    const list = await api.sessions(null);
    setState("sessions", reconcile(list, { key: "id" }));
  } catch (e) {
    console.warn(e);
  }
}, 250);

function afterEvent(ev: AgentEvent) {
  if (ev.type === "state" || (ev.type === "message" && ev.message.role === "user") || ev.type === "turn_end") refreshSessions();
  if (ev.type === "command_done" && ev.session === state.activeSession) handleCommandResult(ev.result);
  if (ev.type === "permission_ask" || ev.type === "finished") notifyFor(ev);
}

/** OS notifications for permission prompts and finished work (settings: Notifications). */
function notifyFor(ev: AgentEvent) {
  const n = state.config?.notifications;
  if (!n) return;
  if (n.only_unfocused && document.hasFocus() && ev.session === state.activeSession) return;
  const title = state.sessions.find((s) => s.id === ev.session)?.title || "Xode";
  if (ev.type === "permission_ask" && n.permission) {
    notify(`Permission needed · ${ev.tool}`, `${title}\n${ev.summary}`.slice(0, 240), n.sound).catch(() => {});
  } else if (ev.type === "finished" && !ev.stopped) {
    if (ev.error && n.errors) notify("Failed", title, n.sound).catch(() => {});
    else if (!ev.error && n.finished) notify("Done", title, n.sound).catch(() => {});
  }
}

// ---------- knowledge base

const reloadKb = debounce(() => loadKb(), 300);

function onKbProgress(ev: Extract<AgentEvent, { type: "kb_progress" }>) {
  const p: KbProgress = { source: ev.source, stage: ev.stage, done: ev.done, total: ev.total };
  setState("kb", "progress", ev.source, p);
  if (ev.stage === "idle") {
    setState("kb", "rev", (r) => r + 1);
    reloadKb();
  }
}

export async function loadKb() {
  const pid = state.activeProject;
  if (!pid) return;
  try {
    setState("kb", "overview", reconcile(await api.kbOverview(pid)));
  } catch (e) {
    console.warn(e);
  }
}

export function openKnowledge(tab?: KbTab, note?: string) {
  batch(() => {
    if (tab) setState("kb", "tab", tab);
    if (note) setState("kb", "note", note);
    setState("ui", "overlay", "knowledge");
  });
  loadKb();
}

export function setKbTab(tab: KbTab) {
  setState("kb", "tab", tab);
  persist("kbTab", tab);
}

/** Layer / source keys switched off for the active chat (or the chat about to be created). */
export function kbOff(): string[] {
  const s = activeSessionInfo();
  if (s) return s.kb_off ?? [];
  if (state.kb.pendingOff) return state.kb.pendingOff;
  return (state.kb.overview?.sources ?? []).filter((x) => !x.default_on).map((x) => x.key);
}

export async function setKbOff(off: string[]) {
  const sid = state.activeSession;
  if (!sid) {
    setState("kb", "pendingOff", off);
    return;
  }
  setState("sessions", (s) => s.id === sid, "kb_off", off);
  await api.setSessionKb(sid, off).catch(fail);
}

export function toggleKb(key: string) {
  const off = kbOff();
  setKbOff(off.includes(key) ? off.filter((k) => k !== key) : [...off, key]);
}

// ---------- init

const saveConfig = debounce((cfg: Config) => {
  api.setConfig(cfg).catch(fail);
}, 400);

export async function init() {
  api.onEvent(onEvent);
  const [config, projects, sessions, backdrop] = await Promise.all([
    api.config(),
    api.projects(),
    api.sessions(null),
    windowEffect().catch(() => "none"),
  ]);
  const lastProject = saved<string | null>("project", null);
  const activeProject = projects.find((p) => p.id === lastProject)?.id ?? projects[0]?.id ?? null;
  setState({ config, projects, sessions, backdrop, activeProject, ready: true });
  applyTheme(config.theme, backdrop);
  loadCommands();
  loadKb();
}

export async function loadCommands() {
  try {
    setState("commands", await api.commands(state.activeProject));
  } catch (e) {
    console.warn(e);
  }
}

// ---------- config

/** Mutates the config in place (Solid produce) and saves with debounce. */
export function patchConfig(fn: (c: Config) => void) {
  if (!state.config) return;
  const prevBlur = state.config.theme.blur;
  setState("config", produce((c) => fn(c as Config)));
  const cfg = state.config;
  const plain = JSON.parse(JSON.stringify(cfg)) as Config;
  saveConfig(plain);
  if (cfg.theme.blur !== prevBlur) {
    setWindowEffect(cfg.theme.blur).then((k) => {
      setState("backdrop", k);
      applyTheme(state.config!.theme, k);
    });
  }
  applyTheme(cfg.theme, state.backdrop);
}

// ---------- projects

export function selectProject(id: string) {
  batch(() => {
    setState("activeProject", id);
    setState("activeSession", null);
  });
  persist("project", id);
  loadCommands();
  loadKb();
}

export async function addProject(root: string) {
  try {
    const p = await api.addProject(root);
    setState("projects", (list) => [p, ...list.filter((x) => x.id !== p.id)]);
    selectProject(p.id);
  } catch (e) {
    fail(e);
  }
}

export async function updateProject(p: Project) {
  try {
    await api.updateProject(p);
    setState("projects", (list) => list.map((x) => (x.id === p.id ? p : x)));
  } catch (e) {
    fail(e);
  }
}

export async function removeProject(id: string) {
  try {
    await api.removeProject(id);
    batch(() => {
      setState("projects", (list) => list.filter((x) => x.id !== id));
      setState("sessions", (list) => list.filter((s) => s.project_id !== id));
      if (state.activeProject === id) {
        setState("activeProject", state.projects[0]?.id ?? null);
        setState("activeSession", null);
      }
    });
  } catch (e) {
    fail(e);
  }
}

// ---------- sessions

export function newChat(projectId?: string) {
  const changed = !!projectId && projectId !== state.activeProject;
  batch(() => {
    if (projectId) setState("activeProject", projectId);
    setState("activeSession", null);
    setState("kb", "pendingOff", null);
  });
  if (changed) loadKb();
}

export async function openSession(id: string) {
  const info = state.sessions.find((s) => s.id === id);
  batch(() => {
    setState("activeSession", id);
    if (info && info.project_id !== state.activeProject) {
      setState("activeProject", info.project_id);
      persist("project", info.project_id);
      loadCommands();
      loadKb();
    }
  });
  if (state.live[id]?.loaded) return;
  ensureLive(id);
  try {
    const [msgs, running, stats, ctx] = await Promise.all([
      api.messages(id),
      api.isRunning(id),
      api.stats(id),
      api.contextView(id).catch(() => null),
    ]);
    const items = messagesToItems(msgs, ctx?.segments ?? [], running);
    setState("live", id, (s) => ({ ...s, items: [...items, ...s.items.filter((i) => !msgs.some((m) => m.id === i.id))], loaded: true, running, stats }));
  } catch (e) {
    setState("live", id, "loaded", true);
    fail(e);
  }
}

export async function renameSession(id: string, title: string) {
  try {
    await api.renameSession(id, title);
    setState("sessions", (s) => s.id === id, "title", title);
  } catch (e) {
    fail(e);
  }
}

export async function deleteSession(id: string) {
  try {
    await api.deleteSession(id);
    batch(() => {
      setState("sessions", (list) => list.filter((s) => s.id !== id));
      if (state.activeSession === id) setState("activeSession", null);
    });
  } catch (e) {
    fail(e);
  }
}

async function ensureSession(): Promise<string | null> {
  if (state.activeSession) return state.activeSession;
  const pid = state.activeProject;
  if (!pid) {
    toast("Add a project first", "error");
    return null;
  }
  const s = await api.newSession(pid);
  if (state.kb.pendingOff) {
    s.kb_off = state.kb.pendingOff;
    await api.setSessionKb(s.id, s.kb_off).catch(() => {});
    setState("kb", "pendingOff", null);
  }
  const cfg = state.config;
  if (cfg?.selected.gateway && cfg.selected.model) await api.setModel(s.id, cfg.selected.gateway, cfg.selected.model).catch(() => {});
  if (cfg && cfg.selected.mode !== "normal") await api.setMode(s.id, cfg.selected.mode).catch(() => {});
  batch(() => {
    setState("sessions", (list) => [s, ...list]);
    setState("live", s.id, { ...emptyLive(), loaded: true });
    setState("activeSession", s.id);
  });
  return s.id;
}

// ---------- run

export async function send(text: string, attachments: Attachment[]) {
  const trimmed = text.trim();
  if (!trimmed && !attachments.length) return false;
  try {
    const sid = await ensureSession();
    if (!sid) return false;
    if (trimmed.startsWith("/") && !attachments.length) {
      await runCommand(sid, trimmed);
      return true;
    }
    const images = attachments.filter((a) => a.data && a.mime.startsWith("image/")).map((a) => ({ mime: a.mime, data: a.data }));
    const names = attachments.filter((a) => !a.data).map((a) => a.name);
    const display = names.length ? `${trimmed}${trimmed ? "\n" : ""}${names.map((n) => `[${n}]`).join(" ")}` : trimmed;
    if (!state.live[sid]?.running)
      setState("live", sid, "items", (items) => [...items, { kind: "user" as const, id: `pending-${Date.now()}`, text: display, images, pending: true }]);
    await api.send(sid, trimmed, attachments);
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

export async function runCommand(sid: string, line: string) {
  let r: CommandResult;
  try {
    r = await api.command(sid, line);
  } catch (e) {
    fail(e);
    return;
  }
  await handleCommandResult(r);
}

export async function handleCommandResult(r: CommandResult) {
  switch (r.type) {
    case "notice":
      toast(r.text);
      break;
    case "switch_session":
      await refreshSessionsNow();
      await openSession(r.session_id);
      break;
    case "open": {
      const [panel, page] = r.panel.split(":");
      if (panel === "settings") openSettings((page as SettingsPage) || "gateway");
      else if (panel === "context") setState("ui", "overlay", "context");
      else if (panel === "knowledge") openKnowledge((page as KbTab) || undefined);
      else if (panel === "sessions" || panel === "project") setState("ui", { sidebar: true, sidebarView: "chats" });
      break;
    }
    case "export":
      await saveText(r.path || "chat.md", r.markdown).catch(fail);
      break;
    default:
      break;
  }
}

async function refreshSessionsNow() {
  try {
    setState("sessions", reconcile(await api.sessions(null), { key: "id" }));
  } catch (e) {
    fail(e);
  }
}

/** Rewinds the active chat to a message; a rewound user message goes back into the composer. */
export async function rewindTo(messageId: string, restoreFiles: boolean) {
  const sid = state.activeSession;
  if (!sid) return;
  try {
    const r = await api.rewind(sid, messageId, restoreFiles);
    setState("live", sid, emptyLive());
    await openSession(sid);
    if (r.text) {
      setState("ui", "composerText", r.text);
      window.dispatchEvent(new CustomEvent("xode:focus-composer"));
    }
    if (restoreFiles) toast(r.files ? `Restored ${r.files} file${r.files === 1 ? "" : "s"}` : "No file changes to restore");
    refreshSessions();
  } catch (e) {
    fail(e);
  }
}

/** Deliver queued messages now (interrupts the current generation). */
export async function steer() {
  const sid = state.activeSession;
  if (sid) await api.steer(sid).catch(fail);
}

export async function unqueue(id: string) {
  const sid = state.activeSession;
  if (sid) await api.unqueue(sid, id).catch(fail);
}

export async function stop() {
  const sid = state.activeSession;
  if (sid) await api.cancel(sid).catch(fail);
}

export async function replyPermission(sid: string, reqId: string, decision: PermDecision) {
  setState("live", sid, "items", (i) => i.id === reqId, produce((i) => {
    if (i.kind === "perm") i.decision = decision;
  }));
  await api.permissionReply(reqId, decision).catch(fail);
}

export async function setMode(mode: Mode) {
  patchConfig((c) => (c.selected.mode = mode));
  const sid = state.activeSession;
  if (sid) {
    await api.setMode(sid, mode).catch(fail);
    setState("sessions", (s) => s.id === sid, "mode", mode);
  }
}

export async function setModel(gatewayId: string, model: string) {
  patchConfig((c) => {
    c.selected.gateway = gatewayId;
    c.selected.model = model;
  });
  const sid = state.activeSession;
  if (sid) {
    await api.setModel(sid, gatewayId, model).catch(fail);
    setState("sessions", (s) => s.id === sid, { gateway: gatewayId, model });
  }
}

export const EFFORTS = [
  { value: "", label: "Auto" },
  { value: "off", label: "Off" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

/** Normalized reasoning effort ("" = model default). */
export function effort(): string {
  const v = (state.config?.generation.reasoning_effort ?? "").trim();
  if (v === "none" || v === "minimal") return "off";
  if (v === "max") return "high";
  return EFFORTS.some((e) => e.value === v) ? v : "";
}

export function setEffort(v: string) {
  patchConfig((c) => (c.generation.reasoning_effort = v));
}

// ---------- ui

export function openSettings(page: SettingsPage = "gateway") {
  setState("ui", { overlay: "settings", settingsPage: page });
}

export function toggleSidebar() {
  setState("ui", "sidebar", (v) => !v);
  persist("sidebar", state.ui.sidebar);
}

export function toggleStats() {
  setState("ui", "stats", (v) => !v);
  persist("stats", state.ui.stats);
}

export function insertMention(path: string) {
  setState("ui", "composerText", (t) => `${t}${t && !t.endsWith(" ") ? " " : ""}@${path} `);
  window.dispatchEvent(new CustomEvent("xode:focus-composer"));
}

/** Currently selected gateway/model for display (session override, else config). */
export function currentModel(): { gateway: string; model: string; gatewayName: string; context: number } {
  const cfg = state.config;
  const s = activeSessionInfo();
  const gwId = (s?.gateway || cfg?.selected.gateway) ?? "";
  const gw = cfg?.gateways.find((g) => g.id === gwId || g.name === gwId) ?? cfg?.gateways.find((g) => g.enabled);
  const model = s?.model || cfg?.selected.model || gw?.models[0]?.id || "";
  const info = gw?.models.find((m) => m.id === model);
  return { gateway: gw?.id ?? "", gatewayName: gw?.name ?? "", model, context: info?.context ?? 0 };
}
