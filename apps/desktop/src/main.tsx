import { render } from "solid-js/web";
import App from "./App";
import { demoMode, initBackend, isTauri } from "./lib/api";
import { preloadHighlighter } from "./lib/markdown";
import { init, newChat, openKnowledge, openSession, openSettings, send, setState, type KbTab, type SettingsPage } from "./lib/store";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/chat.css";
import "./styles/composer.css";
import "./styles/panels.css";
import "./styles/settings.css";
import "./styles/knowledge.css";
import "./styles/motion.css";

async function main() {
  await initBackend();
  await init();
  render(() => <App />, document.getElementById("root")!);
  preloadHighlighter();
  if (demoMode) await (await import("./lib/reel")).playReel();
  else if (!isTauri) await demo();
}

/** Mock-only entry views for development screenshots: ?view=chat|stream|context|settings&page=... */
async function demo() {
  const q = new URLSearchParams(location.search);
  const view = q.get("view");
  if (view === "chat" || view === "context") {
    await openSession("s-demo");
    if (view === "context") setState("ui", "overlay", "context");
  } else if (view === "stream") {
    newChat("p-xode");
    await send("Make the compaction trigger respect `threshold_tokens` and the reserve. Run the core tests after.", []);
  } else if (view === "kb") {
    await openSession("s-demo");
    openKnowledge((q.get("tab") as KbTab) || "sources", q.get("note") || undefined);
  } else if (view === "reel") {
    await (await import("./lib/reel")).playReel();
  } else if (view === "onboarding") {
    setState("ui", "onboarding", true);
  } else if (view === "settings") {
    openSettings((q.get("page") as SettingsPage) || "gateway");
  }
}

main().catch((e) => {
  document.body.textContent = String(e);
});
