import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { Copy, Minus, PanelLeft, PanelRight, Square, X } from "lucide-solid";
import { isTauri, platform, windowAction } from "../lib/api";
import { activeSessionInfo, state, toggleSidebar, toggleStats } from "../lib/store";

function WindowControls() {
  const [maximized, setMaximized] = createSignal(false);
  let un: (() => void) | undefined;
  onCleanup(() => un?.());
  onMount(async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const w = getCurrentWindow();
    setMaximized(await w.isMaximized());
    un = await w.onResized(async () => setMaximized(await w.isMaximized()));
  });
  return (
    <div class="caption">
      <button class="caption-btn" onClick={() => windowAction("minimize")} aria-label="Minimize">
        <Minus size={16} stroke-width={1.25} />
      </button>
      <button class="caption-btn" onClick={() => windowAction("maximize")} aria-label="Maximize">
        <Show when={maximized()} fallback={<Square size={13} stroke-width={1.25} />}>
          <Copy size={13} stroke-width={1.25} style={{ transform: "scaleX(-1)" }} />
        </Show>
      </button>
      <button class="caption-btn close" onClick={() => windowAction("close")} aria-label="Close">
        <X size={16} stroke-width={1.25} />
      </button>
    </div>
  );
}

export default function Titlebar() {
  const mac = platform === "macos";
  return (
    <div class="titlebar" data-tauri-drag-region>
      <div class="tb-left" classList={{ open: state.ui.sidebar }} data-tauri-drag-region style={{ width: state.ui.sidebar ? `${state.ui.sidebarWidth}px` : "auto" }}>
        <Show when={mac}>
          <div class="traffic-space" data-tauri-drag-region />
        </Show>
        <button class="icon-btn" onClick={toggleSidebar} aria-label="Toggle sidebar">
          <PanelLeft size={16} stroke-width={1.5} />
        </button>
      </div>
      <div class="tb-title" data-tauri-drag-region>
        {activeSessionInfo()?.title ?? ""}
      </div>
      <div class="tb-right" data-tauri-drag-region>
        <button class="icon-btn" classList={{ on: state.ui.stats }} onClick={toggleStats} aria-label="Toggle stats">
          <PanelRight size={16} stroke-width={1.5} />
        </button>
        <Show when={!mac}>
          <WindowControls />
        </Show>
      </div>
    </div>
  );
}
