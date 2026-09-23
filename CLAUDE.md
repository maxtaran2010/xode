# Xode

Rust coding agent for low-context local models. Desktop (Tauri 2 + SolidJS) and CLI (ratatui).

Crates:
- `crates/xode-core` — types, events, config, store (SQLite), Tool trait, provider, context, compaction, agent loop.
- `crates/xode-tools` — read/write/edit/glob/grep/shell + RTK output filters.
- `crates/xode-index` — tree-sitter + SQLite code index, `code` tool, repo map, `Outliner` impl.
- `crates/xode-net` — web_search, web_fetch, browser (CDP), MCP client.
- `crates/xode-engine` — wires everything; the only API frontends use (see lib.rs doc comment).
- `crates/xode-cli` — `xode` terminal binary.
- `apps/desktop` — Tauri app (`src-tauri`) + SolidJS UI (`src`).

Rules:
- Windows is the primary target; never assume a POSIX shell in tools. Paths shown to the model use `/`.
- Tool schemas and tool outputs must be compact: every token matters (models have ~80k usable context).
- UI: clean Codex-like, lucide icons only, no emoji, no helper text/hotkey hints. Motion: short transform/opacity animations via `lib/motion.tsx` + `styles/motion.css` (enter/exit, tab switches, collapse, press feedback); respect reduced motion.
- Rust toolchain on macOS is at ~/.cargo/bin (not on PATH by default).
- Windows test box: `ssh max@192.168.2.87` (Rust installed, llama-server on :8080 model `qwen27b`, n_ctx 76800).
