import { createEffect, createSignal, For, on, onCleanup, onMount, type Component } from "solid-js";
import { Dynamic } from "solid-js/web";
import { AppWindow, Bell, Coins, DownloadCloud, FoldVertical, Library, Palette, Plug, Server, Shield, SlidersHorizontal, Wrench, X } from "lucide-solid";
import { Switcher } from "../../lib/motion";
import { setState, state, type SettingsPage } from "../../lib/store";
import BrowserPage from "./BrowserPage";
import CompactionPage from "./CompactionPage";
import GatewayPage from "./GatewayPage";
import GenerationPage from "./GenerationPage";
import ImportPage from "./ImportPage";
import KnowledgePage from "./KnowledgePage";
import McpPage from "./McpPage";
import NotificationsPage from "./NotificationsPage";
import PermissionsPage from "./PermissionsPage";
import ThemePage from "./ThemePage";
import TokenSavingPage from "./TokenSavingPage";
import ToolsPage from "./ToolsPage";

type Icon = Component<{ size?: number; "stroke-width"?: number }>;

const PAGES: { id: SettingsPage; label: string; icon: Icon; page: Component }[] = [
  { id: "gateway", label: "Gateway", icon: Server, page: GatewayPage },
  { id: "tokens", label: "Token Saving", icon: Coins, page: TokenSavingPage },
  { id: "permissions", label: "Permissions", icon: Shield, page: PermissionsPage },
  { id: "generation", label: "Generation", icon: SlidersHorizontal, page: GenerationPage },
  { id: "compaction", label: "Compaction", icon: FoldVertical, page: CompactionPage },
  { id: "knowledge", label: "Knowledge", icon: Library, page: KnowledgePage },
  { id: "browser", label: "Browser", icon: AppWindow, page: BrowserPage },
  { id: "mcp", label: "MCPs", icon: Plug, page: McpPage },
  { id: "tools", label: "Tools", icon: Wrench, page: ToolsPage },
  { id: "notifications", label: "Notifications", icon: Bell, page: NotificationsPage },
  { id: "theme", label: "Theme", icon: Palette, page: ThemePage },
  { id: "import", label: "Import", icon: DownloadCloud, page: ImportPage },
];

export default function Settings() {
  const close = () => setState("ui", "overlay", null);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) close();
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  const current = () => PAGES.find((p) => p.id === state.ui.settingsPage) ?? PAGES[0];

  // Sliding highlight behind the active nav item.
  let nav!: HTMLElement;
  const [thumb, setThumb] = createSignal<{ y: number; h: number; instant: boolean } | null>(null);
  const place = (instant: boolean) => {
    const el = nav?.querySelector<HTMLElement>(".set-nav-item.active");
    if (el) setThumb({ y: el.offsetTop, h: el.offsetHeight, instant });
  };
  onMount(() => requestAnimationFrame(() => place(true)));
  createEffect(on(() => state.ui.settingsPage, () => requestAnimationFrame(() => place(false)), { defer: true }));

  return (
    <div class="overlay settings">
      <nav class="set-nav" ref={nav} classList={{ "has-thumb": !!thumb() }}>
        <div
          class="set-nav-thumb"
          style={{ transform: `translateY(${thumb()?.y ?? 0}px)`, height: `${thumb()?.h ?? 0}px`, transition: thumb()?.instant ? "none" : undefined, opacity: thumb() ? 1 : 0 }}
        />
        <div class="set-nav-title">Settings</div>
        <For each={PAGES}>
          {(p) => (
            <button class="set-nav-item" classList={{ active: state.ui.settingsPage === p.id }} onClick={() => setState("ui", "settingsPage", p.id)}>
              <Dynamic component={p.icon} size={15} stroke-width={1.6} />
              <span>{p.label}</span>
            </button>
          )}
        </For>
      </nav>
      <div class="set-main">
        <button class="icon-btn set-close" onClick={close} aria-label="Close">
          <X size={16} stroke-width={1.6} />
        </button>
        <div class="set-scroll">
          <Switcher key={current().id}>
            <Dynamic component={current().page} />
          </Switcher>
        </div>
      </div>
    </div>
  );
}
