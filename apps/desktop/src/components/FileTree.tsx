import { createResource, createSignal, For, Show } from "solid-js";
import { ChevronRight, File, Folder } from "lucide-solid";
import { api } from "../lib/api";
import { relPath } from "../lib/format";
import { insertMention } from "../lib/store";
import type { FileEntry, Project } from "../lib/types";

function Node(props: { entry: FileEntry; depth: number; root: string }) {
  const [open, setOpen] = createSignal(false);
  const pad = () => ({ "padding-left": `${10 + props.depth * 14}px` });
  if (!props.entry.is_dir)
    return (
      <button class="sb-row ft-row" style={pad()} onClick={() => insertMention(relPath(props.root, props.entry.path))} title={props.entry.path}>
        <span class="ft-chev" />
        <span class="sb-icon">
          <File size={14} stroke-width={1.5} />
        </span>
        <span class="sb-label">{props.entry.name}</span>
      </button>
    );
  return (
    <>
      <button class="sb-row ft-row" style={pad()} onClick={() => setOpen(!open())}>
        <span class="ft-chev" classList={{ open: open() }}>
          <ChevronRight size={13} stroke-width={1.6} />
        </span>
        <span class="sb-icon">
          <Folder size={14} stroke-width={1.5} />
        </span>
        <span class="sb-label">{props.entry.name}</span>
      </button>
      <Show when={open()}>
        <Dir path={props.entry.path} depth={props.depth + 1} root={props.root} />
      </Show>
    </>
  );
}

function Dir(props: { path: string; depth: number; root: string }) {
  const [entries] = createResource(
    () => props.path,
    (p) => api.listDir(p).catch(() => [] as FileEntry[]),
  );
  return <For each={entries() ?? []}>{(e) => <Node entry={e} depth={props.depth} root={props.root} />}</For>;
}

export default function FileTree(props: { project: Project }) {
  return (
    <div class="ft">
      <Dir path={props.project.root} depth={0} root={props.project.root} />
      <For each={props.project.extra_roots}>
        {(r) => (
          <Node entry={{ name: r.split(/[\\/]/).pop() || r, path: r, is_dir: true, size: 0 }} depth={0} root={r} />
        )}
      </For>
    </div>
  );
}
