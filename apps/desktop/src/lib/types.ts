// Mirrors of the serde types in crates/xode-core and crates/xode-engine.

export type Role = "system" | "user" | "assistant" | "tool";
export type Mode = "normal" | "plan";
export type MsgKind = "normal" | "compaction" | "seed" | "goal" | "note";

export type Part =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "image"; mime: string; data: string }
  | { type: "tool_call"; id: string; name: string; args: unknown }
  | { type: "tool_result"; id: string; name: string; content: string; is_error: boolean };

export interface TurnMeta {
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  ttft_ms: number;
  duration_ms: number;
  decode_tps: number;
  prefill_tps: number;
}

export interface Message {
  id: string;
  role: Role;
  parts: Part[];
  kind: MsgKind;
  segment: number;
  meta: TurnMeta | null;
  created_at: number;
}

export interface LiveStats {
  tps: number;
  ttft_ms: number;
  prefill_tps: number;
  tokens_in: number;
  tokens_out: number;
  context_used: number;
  context_limit: number;
  compactions: number;
  steps: number;
  tool_calls: number;
  elapsed_ms: number;
  total_work_ms: number;
  rtk_saved: number;
  model: string;
  gateway: string;
}

export type AgentEvent =
  | { type: "message"; session: string; message: Message }
  | { type: "turn_start"; session: string; id: string }
  | { type: "text_delta"; session: string; id: string; text: string }
  | { type: "thinking_delta"; session: string; id: string; text: string }
  | { type: "tool_call_start"; session: string; id: string; call_id: string; name: string }
  | { type: "tool_call_args"; session: string; id: string; call_id: string; args: unknown }
  | { type: "tool_result"; session: string; call_id: string; name: string; content: string; is_error: boolean; ms: number }
  | { type: "turn_end"; session: string; message: Message; meta: TurnMeta }
  | { type: "stats"; session: string; stats: LiveStats }
  | { type: "compaction_start"; session: string; used: number; limit: number }
  | { type: "compaction_progress"; session: string; stage: string; tokens: number; expected: number }
  | { type: "compaction_done"; session: string; segment: number; path: string; state: string; before: number; after: number }
  | { type: "goal_check"; session: string; done: boolean; reason: string }
  | { type: "permission_ask"; session: string; req_id: string; tool: string; summary: string }
  | { type: "state"; session: string; running: boolean }
  | { type: "queue"; session: string; items: QueuedMsg[] }
  | { type: "command_done"; session: string; result: CommandResult }
  | { type: "finished"; session: string; stopped: boolean; error: boolean }
  | { type: "notice"; session: string; text: string }
  | { type: "error"; session: string; text: string }
  | { type: "kb_progress"; session: string; source: string; stage: KbStage; done: number; total: number };

// ---------- store

export interface Project {
  id: string;
  name: string;
  root: string;
  extra_roots: string[];
  created_at: number;
  last_used: number;
}

export interface SessionInfo {
  id: string;
  project_id: string;
  title: string;
  created_at: number;
  updated_at: number;
  goal: string | null;
  mode: Mode;
  model: string;
  gateway: string;
  segment: number;
  state: string;
  total_work_ms: number;
  tokens_in: number;
  tokens_out: number;
  tool_calls: number;
  compactions: number;
  cwd: string | null;
  /** Knowledge-base layer keys / source keys switched off for this chat. */
  kb_off?: string[];
}

export interface Segment {
  session_id: string;
  index: number;
  path: string;
  state: string;
  tokens_before: number;
  tokens_after: number;
  created_at: number;
}

// ---------- config

export type ApiKind = "openai" | "anthropic";
export type Perm = "allow" | "ask" | "deny";

export interface ModelInfo {
  id: string;
  context: number;
  vision: boolean;
}

export interface Gateway {
  id: string;
  name: string;
  url: string;
  kind: ApiKind;
  api_key: string;
  flavor: string;
  models: ModelInfo[];
  enabled: boolean;
}

export interface Generation {
  temperature: number | null;
  top_p: number | null;
  top_k: number | null;
  min_p: number | null;
  repeat_penalty: number | null;
  presence_penalty: number | null;
  frequency_penalty: number | null;
  max_tokens: number | null;
  seed: number | null;
  reasoning_effort: string;
  reasoning_budget: { low: number; medium: number; high: number };
  stop: string[];
  extra_body: string;
  keep_thinking: boolean;
  parallel_tool_calls: boolean;
  request_timeout_s: number;
}

export interface Compaction {
  enabled: boolean;
  context_limit: number;
  fallback_context: number;
  threshold_tokens: number;
  threshold_ratio: number;
  hard_ratio: number;
  reserve_tokens: number;
  path_file: string;
  path_max_lines: number;
  path_entry_max_lines: number;
  state_max_words: number;
  keep_recent_messages: number;
  include_working_set: boolean;
  working_set_max_files: number;
  include_repo_map: boolean;
  keep_original_request: boolean;
  fold_old_entries: boolean;
  prompt: string;
  seed_template: string;
}

export interface TokenSaving {
  rtk_enabled: boolean;
  rtk_strip_ansi: boolean;
  rtk_collapse_progress: boolean;
  rtk_dedup_lines: boolean;
  rtk_command_filters: boolean;
  max_tool_output_tokens: number;
  head_lines: number;
  tail_lines: number;
  save_full_output: boolean;
  read_dedup: boolean;
  read_max_lines: number;
  repo_map_tokens: number;
  attach_inline_max_tokens: number;
  drop_old_thinking: boolean;
  stub_old_tool_results_after: number;
}

export interface Rule {
  pattern: string;
  perm: Perm;
}

export interface Permissions {
  full_access: boolean;
  tools: Record<string, Perm>;
  shell_rules: Rule[];
  path_rules: Rule[];
  allow_outside_project: Perm;
  network: Perm;
  browser: Perm;
}

export interface Browser {
  kind: string;
  executable: string;
  debug_port: number;
  mode: string;
  user_data_dir: string;
  profile: string;
  headless: boolean;
  extra_args: string[];
  snapshot_max_tokens: number;
}

export interface McpServer {
  name: string;
  transport: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  url: string;
  headers: Record<string, string>;
  enabled: boolean;
  disabled_tools: string[];
}

export interface Tools {
  enabled: Record<string, boolean>;
  shell: string;
  shell_timeout_s: number;
  search_order: string[];
  searxng_url: string;
  brave_key: string;
  tavily_key: string;
  search_results: number;
  fetch_max_tokens: number;
  index_enabled: boolean;
  index_watch: boolean;
  index_max_file_kb: number;
  grep_max_results: number;
  glob_max_results: number;
  edit_fuzzy: boolean;
  goal_judge: boolean;
}

export interface Theme {
  preset: string;
  blur: boolean;
  tokens: Record<string, string>;
  font_size: number;
  mono_font: string;
}

export interface Selection {
  gateway: string;
  model: string;
  mode: Mode;
}

export interface Config {
  gateways: Gateway[];
  selected: Selection;
  generation: Generation;
  compaction: Compaction;
  token_saving: TokenSaving;
  permissions: Permissions;
  browser: Browser;
  mcp: McpServer[];
  tools: Tools;
  theme: Theme;
  notifications: Notifications;
  knowledge: Knowledge;
  system_prompt_extra: string;
}

export interface Knowledge {
  enabled: boolean;
  /** builtin | gateway | off */
  embedder: string;
  builtin_model: string;
  gateway: string;
  gateway_model: string;
  chunk_tokens: number;
  k: number;
  outline_tokens: number;
  read_max_tokens: number;
  ai_write_global: boolean;
  ai_write_project: boolean;
}

export interface Notifications {
  permission: boolean;
  finished: boolean;
  errors: boolean;
  sound: boolean;
  only_unfocused: boolean;
}

// ---------- engine api

export interface Attachment {
  path: string;
  name: string;
  mime: string;
  data: string;
}

export interface ContextSection {
  key: string;
  label: string;
  tokens: number;
  content: string;
  /** Tool results per tool (not counted again in the total). */
  children?: ContextSection[];
}

export interface ContextView {
  used: number;
  limit: number;
  threshold: number;
  sections: ContextSection[];
  segments: Segment[];
  path_md: string;
}

export interface GatewayTest {
  ok: boolean;
  latency_ms: number;
  models: number;
  message: string;
}

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

export interface CommandInfo {
  name: string;
  args: string;
  description: string;
  source: string;
}

export type PermDecision = "once" | "always" | "deny";

export type CommandResult =
  | { type: "done" }
  | { type: "notice"; text: string }
  | { type: "switch_session"; session_id: string }
  | { type: "open"; panel: string }
  | { type: "export"; markdown: string; path: string }
  | { type: "exit" };

export interface McpStatus {
  name: string;
  connected: boolean;
  tools: number | string[];
  error?: string | null;
}

export interface RewindResult {
  text: string;
  removed: number;
  files: number;
}

export interface QueuedMsg {
  id: string;
  text: string;
  /** A slash command waiting its turn (e.g. /compact). */
  command?: boolean;
}

// ---------- knowledge base

export type KbLayer = "library" | "docs" | "memory" | "project_memory";
export type KbStage = "scan" | "index" | "embed" | "idle";

export interface KbSource {
  /** `g:3` (global store) / `p:1` (project store). */
  key: string;
  id: number;
  layer: KbLayer;
  name: string;
  path: string;
  default_on: boolean;
  notes: number;
  chunks: number;
  embedded: number;
  bytes: number;
}

export interface KbEmbedStatus {
  state: "off" | "loading" | "ready" | "error";
  model: string;
  error: string;
}

export interface KbOverview {
  sources: KbSource[];
  embed: KbEmbedStatus;
  owner: boolean;
}

export interface KbHit {
  id: string;
  title: string;
  heading: string;
  line_start: number;
  line_end: number;
  tokens: number;
  note_tokens: number;
  snippet: string;
  score: number;
  layer: KbLayer;
  source: string;
  rel: string;
}

export interface KbLink {
  id: string | null;
  title: string;
}

export interface KbNote {
  id: string;
  title: string;
  source: string;
  source_name: string;
  layer: KbLayer;
  rel: string;
  abs: string;
  tags: string[];
  summary: string;
  tokens: number;
  headings: [number, string, number][];
  links: KbLink[];
  backlinks: KbLink[];
  text: string;
  writable: boolean;
}

export interface KbNoteRow {
  id: string;
  title: string;
  source: string;
  rel: string;
  tokens: number;
  tags: string[];
}

export interface KbFolder {
  folders: [string, number][];
  notes: KbNoteRow[];
}

export interface KbGraphNode {
  id: string;
  title: string;
  layer: KbLayer;
  source: string;
  tokens: number;
  degree: number;
}

export interface KbGraph {
  nodes: KbGraphNode[];
  edges: [string, string][];
  hidden: number;
}

export interface KbSearchReq {
  q: string;
  layer?: KbLayer | null;
  tag?: string | null;
  k?: number | null;
  off?: string[];
}

export interface KbProgress {
  source: string;
  stage: KbStage;
  done: number;
  total: number;
}
