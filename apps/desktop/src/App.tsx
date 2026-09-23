import { createEffect, onCleanup, onMount, Show } from "solid-js";
import Chat from "./components/Chat";
import Composer, { addAttachments, mimeFor, readFileAttachment } from "./components/Composer";
import ContextView from "./components/ContextView";
import Knowledge from "./components/Knowledge/Knowledge";
import Settings from "./components/Settings/Settings";
import Sidebar, { SidebarResizer } from "./components/Sidebar";
import StatsPanel from "./components/StatsPanel";
import Titlebar from "./components/Titlebar";
import { MenuHost } from "./components/ui";
import { api, isTauri, onNativeDrop, platform } from "./lib/api";
import { basename } from "./lib/format";
import { Presence } from "./lib/motion";
import { setLinkBase } from "./lib/markdown";
import { activeProject, activeSessionInfo, addProject, setState, state } from "./lib/store";

async function handlePaths(paths: string[]) {
  const files: string[] = [];
  for (const p of paths) {
    const isDir = await api
      .listDir(p)
      .then(() => true)
      .catch(() => false);
    if (isDir) await addProject(p);
    else files.push(p);
  }
  if (files.length) addAttachments(files.map((p) => ({ path: p, name: basename(p), mime: mimeFor(p), data: "" })));
}

export default function App() {
  // Relative file links in replies resolve against the chat's working folder.
  createEffect(() => setLinkBase(activeSessionInfo()?.cwd || activeProject()?.root || ""));
  let un: (() => void) | undefined;
  onCleanup(() => un?.());
  onMount(async () => {
    un = await onNativeDrop((paths, over) => {
      setState("ui", "dragging", over);
      if (paths.length) handlePaths(paths);
    });
  });

  // Browser (mock) drag & drop: File objects only.
  const onDragOver = (e: DragEvent) => {
    if (isTauri || !e.dataTransfer?.types.includes("Files")) return;
    e.preventDefault();
    setState("ui", "dragging", true);
  };
  const onDrop = async (e: DragEvent) => {
    if (isTauri) return;
    e.preventDefault();
    setState("ui", "dragging", false);
    const files = [...(e.dataTransfer?.files ?? [])];
    if (files.length) addAttachments(await Promise.all(files.map(readFileAttachment)));
  };

  return (
    <div
      class="app"
      classList={{ "has-overlay": !!state.ui.overlay }}
      data-platform={platform}
      onDragOver={onDragOver}
      onDragLeave={(e) => e.relatedTarget === null && setState("ui", "dragging", false)}
      onDrop={onDrop}
      onContextMenu={(e) => {
        const t = e.target as HTMLElement;
        if (!t.closest("input, textarea, .md, pre")) e.preventDefault();
      }}
    >
      <Titlebar />
      <div class="cols">
        <Presence when={state.ui.sidebar} name="col">
          <aside class="col-sidebar" style={{ width: `${state.ui.sidebarWidth}px`, "--col-w": `${state.ui.sidebarWidth}px` }}>
            <Sidebar />
            <SidebarResizer />
          </aside>
        </Presence>
        <main class="col-main">
          <Chat />
          <Composer />
        </main>
        <Presence when={state.ui.stats} name="col">
          <aside class="col-stats">
            <StatsPanel />
          </aside>
        </Presence>
      </div>
      <Presence when={state.ui.overlay === "settings"}>
        <Settings />
      </Presence>
      <Presence when={state.ui.overlay === "context"}>
        <ContextView />
      </Presence>
      <Presence when={state.ui.overlay === "knowledge"}>
        <Knowledge />
      </Presence>
      <Show when={state.ui.dragging}>
        <div class="drop-hint" />
      </Show>
      <MenuHost />
    </div>
  );
}
