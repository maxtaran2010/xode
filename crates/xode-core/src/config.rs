use crate::types::Mode;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let d = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("xode");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApiKind {
    #[default]
    Openai,
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct ModelInfo {
    pub id: String,
    /// Context window reported by the server (0 = unknown).
    pub context: u64,
    pub vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Gateway {
    pub id: String,
    pub name: String,
    pub url: String,
    pub kind: ApiKind,
    pub api_key: String,
    /// Server flavour detected (llamacpp, ollama, lmstudio, vllm, openai, anthropic ...).
    pub flavor: String,
    pub models: Vec<ModelInfo>,
    pub enabled: bool,
}

impl Default for Gateway {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            url: String::new(),
            kind: ApiKind::Openai,
            api_key: String::new(),
            flavor: String::new(),
            models: vec![],
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Generation {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub max_tokens: Option<u32>,
    pub seed: Option<i64>,
    /// "" (model default) | off | low | medium | high
    pub reasoning_effort: String,
    /// Thinking token budgets per effort level (Anthropic `budget_tokens`, llama.cpp/vLLM hints).
    pub reasoning_budget: ReasoningBudget,
    pub stop: Vec<String>,
    /// Raw JSON merged into the request body.
    pub extra_body: String,
    /// Send prior thinking back to the model.
    pub keep_thinking: bool,
    pub parallel_tool_calls: bool,
    pub request_timeout_s: u64,
}

impl Default for Generation {
    fn default() -> Self {
        Self {
            temperature: None,
            top_p: None,
            top_k: None,
            min_p: None,
            repeat_penalty: None,
            presence_penalty: None,
            frequency_penalty: None,
            max_tokens: Some(16384),
            seed: None,
            reasoning_effort: String::new(),
            reasoning_budget: ReasoningBudget::default(),
            stop: vec![],
            extra_body: String::new(),
            keep_thinking: false,
            parallel_tool_calls: true,
            request_timeout_s: 900,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReasoningBudget {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
}

impl Default for ReasoningBudget {
    fn default() -> Self {
        Self { low: 2048, medium: 8192, high: 24576 }
    }
}

impl Generation {
    /// Normalized effort: "" | off | low | medium | high.
    pub fn effort(&self) -> &str {
        match self.reasoning_effort.trim() {
            "none" | "off" | "minimal" => "off",
            "low" => "low",
            "medium" => "medium",
            "high" | "max" => "high",
            _ => "",
        }
    }
    /// Thinking budget for the current effort (None = model default or off).
    pub fn thinking_budget(&self) -> Option<u32> {
        match self.effort() {
            "low" => Some(self.reasoning_budget.low),
            "medium" => Some(self.reasoning_budget.medium),
            "high" => Some(self.reasoning_budget.high),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Notifications {
    pub permission: bool,
    pub finished: bool,
    pub errors: bool,
    pub sound: bool,
    /// Only notify while the window/terminal is not focused.
    pub only_unfocused: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self { permission: true, finished: true, errors: true, sound: true, only_unfocused: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Compaction {
    pub enabled: bool,
    /// Context window override (0 = use model-reported value, else fallback).
    pub context_limit: u64,
    pub fallback_context: u64,
    /// Compact when used tokens reach this (absolute). 0 = use ratio.
    pub threshold_tokens: u64,
    pub threshold_ratio: f32,
    /// Emergency compaction if a tool result pushes past this ratio.
    pub hard_ratio: f32,
    /// Tokens reserved for the compaction reply.
    pub reserve_tokens: u64,
    pub path_file: String,
    pub path_max_lines: usize,
    pub path_entry_max_lines: usize,
    pub state_max_words: usize,
    pub keep_recent_messages: usize,
    pub include_working_set: bool,
    pub working_set_max_files: usize,
    pub include_repo_map: bool,
    pub keep_original_request: bool,
    pub fold_old_entries: bool,
    pub prompt: String,
    pub seed_template: String,
}

pub const DEFAULT_COMPACT_PROMPT: &str = "CONTEXT LIMIT REACHED. Stop working and write a handoff for yourself. The context will be cleared and you will continue from this handoff only. Do not call tools.\nReply with exactly:\n<path>\n- up to {path_lines} one-line bullets: what you did since the last handoff (files changed, key findings, commands that worked). Terse, no filler.\n</path>\n<state>\ngoal: <one line>\ndone: <one line>\nnow: <what you were doing at the cut>\nnext: <next 1-3 concrete steps>\nnotes: <facts needed to continue: paths, symbols, decisions, gotchas>\n</state>\nMax {state_words} words inside <state>.";

pub const DEFAULT_SEED_TEMPLATE: &str = "[Context was compacted. Continue the task from here without redoing finished work. Do not re-read files listed in the working set unless you must edit a part you have not seen.]\n\n{request}\n\n## Project path (.xode/PATH.md)\n{path}\n\n## State\n{state}\n\n{goal}\n\n{working_set}";

impl Default for Compaction {
    fn default() -> Self {
        Self {
            enabled: true,
            context_limit: 0,
            fallback_context: 98_304,
            threshold_tokens: 0,
            threshold_ratio: 0.82,
            hard_ratio: 0.93,
            reserve_tokens: 2048,
            path_file: ".xode/PATH.md".into(),
            path_max_lines: 60,
            path_entry_max_lines: 6,
            state_max_words: 180,
            keep_recent_messages: 2,
            include_working_set: true,
            working_set_max_files: 16,
            include_repo_map: true,
            keep_original_request: true,
            fold_old_entries: true,
            prompt: DEFAULT_COMPACT_PROMPT.into(),
            seed_template: DEFAULT_SEED_TEMPLATE.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TokenSaving {
    pub rtk_enabled: bool,
    pub rtk_strip_ansi: bool,
    pub rtk_collapse_progress: bool,
    pub rtk_dedup_lines: bool,
    pub rtk_command_filters: bool,
    /// Max tokens of a single tool result before head/tail truncation.
    pub max_tool_output_tokens: u64,
    pub head_lines: usize,
    pub tail_lines: usize,
    pub save_full_output: bool,
    pub read_dedup: bool,
    pub read_max_lines: usize,
    pub repo_map_tokens: u64,
    pub attach_inline_max_tokens: u64,
    pub drop_old_thinking: bool,
    /// Replace tool results older than N turns with a one-line stub.
    pub stub_old_tool_results_after: usize,
}

impl Default for TokenSaving {
    fn default() -> Self {
        Self {
            rtk_enabled: true,
            rtk_strip_ansi: true,
            rtk_collapse_progress: true,
            rtk_dedup_lines: true,
            rtk_command_filters: true,
            max_tool_output_tokens: 4000,
            head_lines: 60,
            tail_lines: 80,
            save_full_output: true,
            read_dedup: true,
            read_max_lines: 400,
            repo_map_tokens: 1200,
            attach_inline_max_tokens: 6000,
            drop_old_thinking: true,
            stub_old_tool_results_after: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Perm {
    #[default]
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rule {
    /// Glob-like pattern (`*` wildcard) for commands or paths.
    pub pattern: String,
    pub perm: Perm,
}

impl Default for Rule {
    fn default() -> Self {
        Self { pattern: String::new(), perm: Perm::Ask }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Permissions {
    pub full_access: bool,
    /// Per tool default.
    pub tools: BTreeMap<String, Perm>,
    pub shell_rules: Vec<Rule>,
    pub path_rules: Vec<Rule>,
    pub allow_outside_project: Perm,
    pub network: Perm,
    pub browser: Perm,
}

impl Default for Permissions {
    fn default() -> Self {
        let mut tools = BTreeMap::new();
        for t in ["read", "glob", "grep", "code", "web_search", "web_fetch"] {
            tools.insert(t.to_string(), Perm::Allow);
        }
        for t in ["edit", "write", "shell", "browser"] {
            tools.insert(t.to_string(), Perm::Allow);
        }
        Self {
            full_access: true,
            tools,
            shell_rules: vec![
                Rule { pattern: "rm -rf /*".into(), perm: Perm::Ask },
                Rule { pattern: "format *".into(), perm: Perm::Ask },
                Rule { pattern: "Remove-Item * -Recurse*C:\\".into(), perm: Perm::Ask },
                Rule { pattern: "git push*".into(), perm: Perm::Ask },
            ],
            path_rules: vec![],
            allow_outside_project: Perm::Allow,
            network: Perm::Allow,
            browser: Perm::Allow,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Browser {
    /// chrome | edge | brave | chromium | custom
    pub kind: String,
    pub executable: String,
    pub debug_port: u16,
    /// attach | launch
    pub mode: String,
    /// Empty = xode-managed profile dir.
    pub user_data_dir: String,
    pub profile: String,
    pub headless: bool,
    pub extra_args: Vec<String>,
    pub snapshot_max_tokens: u64,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            kind: if cfg!(windows) { "edge".into() } else { "chrome".into() },
            executable: String::new(),
            debug_port: 9222,
            mode: "attach".into(),
            user_data_dir: String::new(),
            profile: "Default".into(),
            headless: false,
            extra_args: vec![],
            snapshot_max_tokens: 3000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpServer {
    pub name: String,
    /// stdio | http
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub disabled_tools: Vec<String>,
}

impl Default for McpServer {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: "stdio".into(),
            command: String::new(),
            args: vec![],
            env: BTreeMap::new(),
            url: String::new(),
            headers: BTreeMap::new(),
            enabled: true,
            disabled_tools: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Tools {
    pub enabled: BTreeMap<String, bool>,
    /// auto | pwsh | powershell | cmd | bash | zsh | sh
    pub shell: String,
    pub shell_timeout_s: u64,
    pub search_order: Vec<String>,
    pub searxng_url: String,
    pub brave_key: String,
    pub tavily_key: String,
    pub search_results: usize,
    pub fetch_max_tokens: u64,
    pub index_enabled: bool,
    pub index_watch: bool,
    pub index_max_file_kb: u64,
    pub grep_max_results: usize,
    pub glob_max_results: usize,
    pub edit_fuzzy: bool,
    pub goal_judge: bool,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            enabled: BTreeMap::new(),
            shell: "auto".into(),
            shell_timeout_s: 300,
            search_order: vec!["duckduckgo".into(), "searxng".into(), "brave".into(), "tavily".into(), "browser".into()],
            searxng_url: String::new(),
            brave_key: String::new(),
            tavily_key: String::new(),
            search_results: 6,
            fetch_max_tokens: 4000,
            index_enabled: true,
            index_watch: true,
            index_max_file_kb: 512,
            grep_max_results: 80,
            glob_max_results: 200,
            edit_fuzzy: true,
            goal_judge: true,
        }
    }
}

impl Tools {
    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Theme {
    pub preset: String,
    pub blur: bool,
    pub tokens: BTreeMap<String, String>,
    pub font_size: u32,
    pub mono_font: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self { preset: "fluent-blue".into(), blur: true, tokens: BTreeMap::new(), font_size: 14, mono_font: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Selection {
    pub gateway: String,
    pub model: String,
    pub mode: Mode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub gateways: Vec<Gateway>,
    pub selected: Selection,
    pub generation: Generation,
    pub compaction: Compaction,
    pub token_saving: TokenSaving,
    pub permissions: Permissions,
    pub browser: Browser,
    pub mcp: Vec<McpServer>,
    pub tools: Tools,
    pub theme: Theme,
    pub notifications: Notifications,
    pub system_prompt_extra: String,
}

impl Config {
    pub fn load() -> Self {
        let p = config_path();
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("config parse error: {e}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let s = toml::to_string_pretty(self)?;
        let p = config_path();
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    pub fn gateway(&self, id: &str) -> Option<&Gateway> {
        self.gateways.iter().find(|g| g.id == id)
    }

    /// Currently selected gateway + model (falls back to first available).
    pub fn active(&self) -> Option<(Gateway, String)> {
        let g = self
            .gateway(&self.selected.gateway)
            .or_else(|| self.gateways.iter().find(|g| g.enabled))?
            .clone();
        let model = if !self.selected.model.is_empty() {
            self.selected.model.clone()
        } else {
            g.models.first().map(|m| m.id.clone()).unwrap_or_default()
        };
        Some((g, model))
    }

    pub fn context_limit_for(&self, g: &Gateway, model: &str) -> u64 {
        if self.compaction.context_limit > 0 {
            return self.compaction.context_limit;
        }
        g.models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.context)
            .filter(|c| *c > 0)
            .unwrap_or(self.compaction.fallback_context)
    }

    pub fn threshold(&self, limit: u64) -> u64 {
        let c = &self.compaction;
        let t = if c.threshold_tokens > 0 {
            c.threshold_tokens
        } else {
            (limit as f64 * c.threshold_ratio as f64) as u64
        };
        t.min(limit.saturating_sub(c.reserve_tokens))
    }
}
