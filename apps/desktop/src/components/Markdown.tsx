import { createMemo } from "solid-js";
import { handleCodeCopy, highlightVersion, linkBaseVersion, renderMarkdown } from "../lib/markdown";

export default function Markdown(props: { text: string; class?: string }) {
  const html = createMemo(() => {
    highlightVersion();
    linkBaseVersion();
    return renderMarkdown(props.text);
  });
  return <div class={`md ${props.class ?? ""}`} innerHTML={html()} onClick={handleCodeCopy} />;
}
