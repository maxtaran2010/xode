// Scripted product demo for the README recording (desktop app with XODE_DEMO=1, mock engine).
import { api, isTauri, setWindowEffect } from "./api";
import { applyTheme } from "./theme";
import { answerPlan, newChat, openKnowledge, replyPermission, send, setKbTab, setMode, setState, state } from "./store";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const PLAN_PROMPT = "Plan how to rate-limit the login endpoint so we can stop brute-force attempts.";
const BUILD_NOTE = "The agent switches to Normal mode and implements the saved plan.";

async function prepareWindow() {
  if (!isTauri) return;
  const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  await w.setSize(new LogicalSize(1280, 800)).catch(() => {});
  await w.center().catch(() => {});
  // Solid background: the recording keeps only the window, with a transparent surround.
  const k = await setWindowEffect(false).catch(() => "none");
  setState("backdrop", k);
  if (state.config) applyTheme(state.config.theme, k);
}

async function type(text: string) {
  for (let i = 1; i <= text.length; i++) {
    setState("ui", "composerText", text.slice(0, i));
    await sleep(18 + Math.random() * 26);
  }
}

/** Resolves when the active chat's run is over. */
function runEnded(sid: () => string | null) {
  return new Promise<void>((resolve) => {
    const un = api.onEvent((ev) => {
      if (ev.session === sid() && ev.type === "state" && !ev.running) {
        un();
        resolve();
      }
    });
  });
}

async function ask(prompt: string) {
  window.dispatchEvent(new CustomEvent("xode:focus-composer"));
  await type(prompt);
  await sleep(500);
  const done = runEnded(() => state.activeSession);
  setState("ui", "composerText", "");
  await send(prompt, []);
  await done;
}

export async function playReel() {
  await prepareWindow();
  setState("ui", { sidebar: true, stats: true, overlay: null });
  // Auto-approve permission prompts after a beat, like a user would.
  api.onEvent((ev) => {
    if (ev.type === "permission_ask") setTimeout(() => replyPermission(ev.session, ev.req_id, "once"), 1500);
  });

  newChat("p-xode");
  await sleep(2400);

  // --- Plan mode: the agent researches and writes a plan, then offers to build it.
  await setMode("plan");
  await sleep(900);
  await ask(PLAN_PROMPT);
  // The plan card renders; give it a beat to read, then Build.
  await sleep(3400);
  const built = runEnded(() => state.activeSession);
  const plan = [...(state.live[state.activeSession!]?.items ?? [])].reverse().find((i) => i.kind === "plan");
  if (plan) await answerPlan(state.activeSession!, plan.id, "build");
  await setMode("normal"); // flip the composer toggle back to Normal for the build
  void BUILD_NOTE;
  await built; // implementation run (edits + tests) in Normal mode
  await sleep(2200);

  // --- Self-compaction, by hand.
  window.dispatchEvent(new CustomEvent("xode:focus-composer"));
  await type("/compact");
  await sleep(450);
  setState("ui", "composerText", "");
  await send("/compact", []);
  await sleep(2400);
  setState("ui", "overlay", "context");
  await sleep(3200);
  setState("ui", "overlay", null);
  await sleep(700);

  // --- Knowledge base: sources, graph, a note.
  setState("kb", "note", null);
  openKnowledge("sources");
  await sleep(2400);
  setKbTab("graph");
  await sleep(5000);
  setState("kb", "note", "g3");
  setKbTab("notes");
  await sleep(3000);
  setState("ui", "overlay", null);
  await sleep(1600);

  // End of the recording: the recorder stops when the window goes away.
  if (isTauri) (await import("@tauri-apps/api/window")).getCurrentWindow().close();
}
