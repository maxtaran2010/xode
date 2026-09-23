import { createMemo } from "solid-js";
import { handleCodeCopy, highlightVersion, linkBaseVersion, renderMarkdown } from "../lib/markdown";

export default function Markdown(props: { text: string; class?: string }) {
  const html = createMemo(() => {
    highlightVersion();
    linkBaseVersion();
    return renderMarkdown(props.text);
  });
  // A missing local image turns into a plain file link instead of a broken-image icon.
  const onError = (e: Event) => {
    const img = e.target as HTMLImageElement;
    if (img.tagName !== "IMG" || !img.dataset.file) return;
    const a = document.createElement("a");
    a.href = "#";
    a.className = "file-link";
    a.dataset.file = img.dataset.file;
    a.textContent = img.alt || img.dataset.file.split(/[\\/]/).pop() || "image";
    img.replaceWith(a);
  };
  return (
    <div
      class={`md ${props.class ?? ""}`}
      innerHTML={html()}
      onClick={handleCodeCopy}
      ref={(el) => el.addEventListener("error", onError, true)}
    />
  );
}
