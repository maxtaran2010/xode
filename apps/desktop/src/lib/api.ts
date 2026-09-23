// Typed wrapper over the Tauri commands (src-tauri/src/commands.rs).
// Falls back to an in-browser mock when not running inside Tauri.
import type {
  AgentEvent,
  RewindResult,
  Attachment,
  CommandInfo,
  CommandResult,
  Config,
  ContextView,
  FileEntry,
  Gateway,
  GatewayTest,
  LiveStats,
  McpServer,
  McpStatus,
  Message,
  Mode,
  PermDecision,
  Project,
  QueuedMsg,
  SessionInfo,
} from "./types";

export interface Backend {
  config(): Promise<Config>;
  setConfig(cfg: Config): Promise<void>;
  projects(): Promise<Project[]>;
  addProject(root: string): Promise<Project>;
  updateProject(project: Project): Promise<void>;
  removeProject(id: string): Promise<void>;
  sessions(projectId?: string | null): Promise<SessionInfo[]>;
  newSession(projectId: string): Promise<SessionInfo>;
  session(id: string): Promise<SessionInfo>;
  renameSession(id: string, title: string): Promise<void>;
  deleteSession(id: string): Promise<void>;
  messages(sessionId: string): Promise<Message[]>;
  send(sessionId: string, text: string, attachments: Attachment[]): Promise<void>;
  cancel(sessionId: string): Promise<void>;
  steer(sessionId: string): Promise<void>;
  unqueue(sessionId: string, id: string): Promise<void>;
  rewind(sessionId: string, messageId: string, restoreFiles: boolean): Promise<RewindResult>;
  queued(sessionId: string): Promise<QueuedMsg[]>;
  isRunning(sessionId: string): Promise<boolean>;
  command(sessionId: string, line: string): Promise<CommandResult>;
  commands(projectId?: string | null): Promise<CommandInfo[]>;
  setMode(sessionId: string, mode: Mode): Promise<void>;
  setModel(sessionId: string, gatewayId: string, model: string): Promise<void>;
  contextView(sessionId: string): Promise<ContextView>;
  stats(sessionId: string): Promise<LiveStats>;
  permissionReply(reqId: string, decision: PermDecision): Promise<void>;
  detectGateway(url: string, apiKey: string): Promise<Gateway>;
  testGateway(gateway: Gateway): Promise<GatewayTest>;
  scanGateways(extraHosts: string[]): Promise<Gateway[]>;
  refreshModels(gatewayId: string): Promise<Gateway>;
  listDir(path: string): Promise<FileEntry[]>;
  completePath(projectId: string, prefix: string): Promise<string[]>;
  browserTest(): Promise<GatewayTest>;
  mcpTest(server: McpServer): Promise<string[]>;
  mcpStatus(): Promise<McpStatus[]>;
  indexStats(projectId: string): Promise<{ files: number; symbols: number; ready: boolean }>;
  onEvent(cb: (ev: AgentEvent) => void): () => void;
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const call = <T>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);
  return {
    config: () => call("config"),
    setConfig: (cfg) => call("set_config", { cfg }),
    projects: () => call("projects"),
    addProject: (root) => call("add_project", { root }),
    updateProject: (project) => call("update_project", { project }),
    removeProject: (id) => call("remove_project", { id }),
    sessions: (projectId) => call("sessions", { projectId: projectId ?? null }),
    newSession: (projectId) => call("new_session", { projectId }),
    session: (id) => call("session", { id }),
    renameSession: (id, title) => call("rename_session", { id, title }),
    deleteSession: (id) => call("delete_session", { id }),
    messages: (sessionId) => call("messages", { sessionId }),
    send: (sessionId, text, attachments) => call("send", { sessionId, text, attachments }),
    cancel: (sessionId) => call("cancel", { sessionId }),
    steer: (sessionId) => call("steer", { sessionId }),
    unqueue: (sessionId, id) => call("unqueue", { sessionId, id }),
    rewind: (sessionId, messageId, restoreFiles) => call("rewind", { sessionId, messageId, restoreFiles }),
    queued: (sessionId) => call("queued", { sessionId }),
    isRunning: (sessionId) => call("is_running", { sessionId }),
    command: (sessionId, line) => call("command", { sessionId, line }),
    commands: (projectId) => call("commands", { projectId: projectId ?? null }),
    setMode: (sessionId, mode) => call("set_mode", { sessionId, mode }),
    setModel: (sessionId, gatewayId, model) => call("set_model", { sessionId, gatewayId, model }),
    contextView: (sessionId) => call("context_view", { sessionId }),
    stats: (sessionId) => call("stats", { sessionId }),
    permissionReply: (reqId, decision) => call("permission_reply", { reqId, decision }),
    detectGateway: (url, apiKey) => call("detect_gateway", { url, apiKey }),
    testGateway: (gateway) => call("test_gateway", { gateway }),
    scanGateways: (extraHosts) => call("scan_gateways", { extraHosts }),
    refreshModels: (gatewayId) => call("refresh_models", { gatewayId }),
    listDir: (path) => call("list_dir", { path }),
    completePath: (projectId, prefix) => call("complete_path", { projectId, prefix }),
    browserTest: () => call("browser_test"),
    mcpTest: (server) => call("mcp_test", { server }),
    mcpStatus: async () => {
      const v = await call<unknown>("mcp_status");
      return Array.isArray(v) ? (v as McpStatus[]) : [];
    },
    indexStats: (projectId) => call("index_stats", { projectId }),
    onEvent: (cb) => {
      let un: (() => void) | undefined;
      let dead = false;
      listen<AgentEvent>("xode://event", (e) => cb(e.payload)).then((u) => (dead ? u() : (un = u)));
      return () => {
        dead = true;
        un?.();
      };
    },
  };
}

let backend: Backend;

export async function initBackend(): Promise<Backend> {
  if (backend) return backend;
  backend = isTauri ? await tauriBackend() : (await import("./mock")).createMockBackend();
  return backend;
}

/** Engine API. Valid after `initBackend()` resolved (done before the app mounts). */
export const api: Backend = new Proxy({} as Backend, {
  get: (_t, key: keyof Backend) => {
    if (!backend) throw new Error("backend not initialised");
    return backend[key];
  },
});

// ---------- platform helpers (dialogs, window, shell)

export type Platform = "windows" | "macos" | "linux";

export const platform: Platform = (() => {
  const forced = new URLSearchParams(location.search).get("platform");
  if (forced === "windows" || forced === "macos" || forced === "linux") return forced;
  const ua = navigator.userAgent;
  if (/Mac/i.test(ua)) return "macos";
  if (/Win/i.test(ua)) return "windows";
  return "linux";
})();

export async function pickFolder(): Promise<string | null> {
  if (!isTauri) return prompt("Folder path") || null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: true, multiple: false });
  return typeof r === "string" ? r : null;
}

export async function pickFiles(): Promise<string[]> {
  if (!isTauri) return [];
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: false, multiple: true });
  return Array.isArray(r) ? r : r ? [r] : [];
}

export async function pickFile(): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const r = await open({ directory: false, multiple: false });
  return typeof r === "string" ? r : null;
}

export async function saveText(defaultPath: string, contents: string): Promise<void> {
  if (!isTauri) {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([contents], { type: "text/markdown" }));
    a.download = defaultPath.split(/[\\/]/).pop() || "export.md";
    a.click();
    return;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const path = await save({ defaultPath, filters: [{ name: "Markdown", extensions: ["md"] }] });
  if (!path) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("write_text_file", { path, contents });
}

export async function openInFileManager(path: string): Promise<void> {
  if (!isTauri) return;
  const { openPath } = await import("@tauri-apps/plugin-opener");
  await openPath(path);
}

/** OS notification + taskbar flash / dock bounce. */
export async function notify(title: string, body: string, sound: boolean): Promise<void> {
  if (!isTauri) {
    if ("Notification" in window && Notification.permission === "granted") new Notification(title, { body });
    return;
  }
  const n = await import("@tauri-apps/plugin-notification");
  let ok = await n.isPermissionGranted();
  if (!ok) ok = (await n.requestPermission()) === "granted";
  if (ok) n.sendNotification({ title, body, sound: sound ? (platform === "windows" ? "Default" : "default") : undefined });
  const { getCurrentWindow, UserAttentionType } = await import("@tauri-apps/api/window");
  await getCurrentWindow()
    .requestUserAttention(UserAttentionType.Informational)
    .catch(() => {});
}

export async function windowAction(action: "minimize" | "maximize" | "close"): Promise<void> {
  if (!isTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  if (action === "minimize") await w.minimize();
  else if (action === "maximize") await w.toggleMaximize();
  else await w.close();
}

/** Native window backdrop kind: "mica" | "acrylic" | "blur" | "vibrancy" | "none". */
export async function setWindowEffect(enabled: boolean): Promise<string> {
  if (!isTauri) return "none";
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("set_window_effect", { enabled });
}

export async function windowEffect(): Promise<string> {
  if (!isTauri) return "none";
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("window_effect");
}

/** Native file drops (Tauri delivers paths, not File objects). */
export async function onNativeDrop(cb: (paths: string[], over: boolean) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "over" || p.type === "enter") cb([], true);
    else if (p.type === "leave") cb([], false);
    else if (p.type === "drop") cb(p.paths, false);
  });
}
