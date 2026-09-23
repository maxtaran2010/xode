# Xode

A coding agent built for local models with small context windows (~80k tokens).

- **Self-compaction**: when context fills up, the agent writes a short PATH/STATE summary and continues in a fresh context, indefinitely.
- **Token-lean tools**: tree-sitter code index (`code` tool), deduplicated reads, RTK-style shell output filters.
- **Full reach**: shell, filesystem, web search/fetch, the user's browser over CDP, MCP servers.
- **Two frontends**: Tauri 2 + SolidJS desktop app and a ratatui terminal UI, sharing one engine.

Works with OpenAI-compatible and Anthropic gateways (llama.cpp, Ollama, LM Studio, vLLM, cloud APIs).

## Layout

| Path | What |
|------|------|
| `crates/xode-core` | types, events, config, SQLite store, provider, context, compaction, agent loop |
| `crates/xode-tools` | read / write / edit / glob / grep / shell + RTK filters |
| `crates/xode-index` | tree-sitter code index and the `code` tool |
| `crates/xode-net` | web search, web fetch, browser (CDP), MCP client |
| `crates/xode-db` | embedded database layer (Turso) |
| `crates/xode-engine` | wires everything; the API frontends use |
| `crates/xode-cli` | `xode` terminal binary |
| `apps/desktop` | Tauri app (`src-tauri`) + SolidJS UI (`src`) |

## Build

```sh
# CLI
cargo build --release -p xode-cli

# Desktop
cd apps/desktop && pnpm install && pnpm tauri build
```

Windows is the primary target; macOS and Linux are supported.
