import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { ArrowDown, FolderPlus } from "lucide-solid";
import { pickFolder } from "../lib/api";
import { stickToBottom } from "../lib/autoscroll";
import { Copy, History, RotateCcw } from "lucide-solid";
import { activeLive, activeProject, addProject, rewindTo, selectProject, state, toast } from "../lib/store";
import type { Item } from "../lib/session";
import ItemView from "./Message";
import { Logo, menuAt, menuFor, type MenuItem } from "./ui";

/** Plain text of a chat item for copying. */
function itemText(item: Item): string {
  if (item.kind === "user") return item.text;
  if (item.kind === "assistant")
    return item.blocks
      .filter((b) => b.kind === "text")
      .map((b) => (b as { text: string }).text)
      .join("\n\n")
      .trim();
  return "";
}

function itemMenu(e: MouseEvent, item: Item) {
  const t = e.target as HTMLElement;
  const selection = window.getSelection()?.toString() ?? "";
  if (t.closest("input, textarea")) return;
  if (item.kind !== "user" && item.kind !== "assistant") return;
  const text = selection || itemText(item);
  const running = !!activeLive()?.running;
  const pending = item.id.startsWith("pending-") || (item.kind === "assistant" && item.streaming);
  const items: MenuItem[] = [
    {
      label: selection ? "Copy selection" : "Copy",
      icon: Copy,
      disabled: !text,
      onSelect: () => navigator.clipboard.writeText(text).then(() => toast("Copied")),
    },
    { type: "separator" },
    {
      label: item.kind === "user" ? "Rewind to before this" : "Rewind to here",
      icon: History,
      disabled: running || pending,
      onSelect: () => rewindTo(item.id, false),
    },
    {
      label: item.kind === "user" ? "Rewind chat and files" : "Rewind here with files",
      icon: RotateCcw,
      disabled: running || pending,
      onSelect: () => rewindTo(item.id, true),
    },
  ];
  menuAt(e, items);
}

const PAGE = 80;

function EmptyState() {
  const project = () => activeProject();
  const pick = (e: MouseEvent) => {
    const items: MenuItem[] = state.projects.map((p) => ({ label: p.name, checked: p.id === state.activeProject, onSelect: () => selectProject(p.id) }));
    if (items.length) items.push({ type: "separator" });
    items.push({
      label: "Add project",
      icon: FolderPlus,
      onSelect: async () => {
        const dir = await pickFolder();
        if (dir) addProject(dir);
      },
    });
    menuFor(e.currentTarget as HTMLElement, items, { minWidth: 200 });
  };
  return (
    <div class="empty">
      <Logo size={36} class="empty-logo" />
      <div class="empty-title">
        <Show when={project()} fallback={<>What should we work on? <button class="empty-project" onClick={pick}>Choose a project</button></>}>
          What should we work on in{" "}
          <button class="empty-project" onClick={pick}>
            {project()!.name}
          </button>
          ?
        </Show>
      </div>
    </div>
  );
}

export default function Chat() {
  let scroller!: HTMLDivElement;
  const [stick, setStick] = createSignal<ReturnType<typeof stickToBottom>>();
  const [limit, setLimit] = createSignal(PAGE);
  const atBottom = () => stick()?.stuck() ?? true;

  const items = () => activeLive()?.items ?? [];
  const start = () => Math.max(0, items().length - limit());
  const visible = () => items().slice(start());

  const scrollToBottom = () => stick()?.jump();

  createEffect(
    on(
      () => state.activeSession,
      () => {
        setLimit(PAGE);
        requestAnimationFrame(scrollToBottom);
      },
    ),
  );

  onMount(() => {
    setStick(stickToBottom(scroller));
  });

  const empty = () => !state.activeSession || (activeLive()?.loaded && items().length === 0);

  return (
    <div class="chat-wrap">
      <div class="chat" ref={scroller} tabIndex={-1}>
        <div class="chat-inner">
          <Show when={!empty()} fallback={<EmptyState />}>
            <Show when={start() > 0}>
              <button class="show-earlier" onClick={() => setLimit(limit() + PAGE)}>
                Show earlier
              </button>
            </Show>
            <For each={visible()}>
              {(item, i) => (
                <div class="item" data-kind={item.kind} onContextMenu={(e) => itemMenu(e, item)}>
                  <ItemView item={item} next={visible()[i() + 1]} session={state.activeSession!} running={!!activeLive()?.running} />
                </div>
              )}
            </For>
          </Show>
        </div>
      </div>
      <Show when={!atBottom() && !empty()}>
        <button class="jump" onClick={scrollToBottom} aria-label="Jump to bottom">
          <ArrowDown size={15} stroke-width={1.75} />
        </button>
      </Show>
    </div>
  );
}
