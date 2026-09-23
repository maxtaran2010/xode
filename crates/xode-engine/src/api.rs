//! Types exchanged between the engine and frontends (Tauri + CLI). All serde-serializable.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attachment {
    /// Absolute path on disk (for files) — or empty when `data` is set.
    #[serde(default)]
    pub path: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// MIME type ("image/png", "text/plain" ...). Empty = infer from path.
    #[serde(default)]
    pub mime: String,
    /// Base64 payload for pasted images / dropped blobs.
    #[serde(default)]
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextSection {
    /// system | tools | brief | path | state | working_set | messages | tool_results | thinking | attachments
    pub key: String,
    pub label: String,
    pub tokens: u64,
    /// Exact text currently sent for this section.
    pub content: String,
    /// Sub-breakdown (tool results per tool). Not counted again in the total.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ContextSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextView {
    pub used: u64,
    pub limit: u64,
    pub threshold: u64,
    pub sections: Vec<ContextSection>,
    pub segments: Vec<xode_core::store::Segment>,
    pub path_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayTest {
    pub ok: bool,
    pub latency_ms: u64,
    pub models: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub name: String,
    /// Absolute path.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub args: String,
    pub description: String,
    /// builtin | custom
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PermDecision {
    Once,
    Always,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RewindResult {
    /// Text of the removed user message (to put back into the composer).
    pub text: String,
    pub removed: usize,
    pub files: usize,
}

/// Result of a slash command executed by the engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandResult {
    /// Nothing to show; state changed (events were emitted).
    Done,
    /// Text to show as a notice.
    Notice { text: String },
    /// Frontend should switch to this session.
    SwitchSession { session_id: String },
    /// Frontend should open a UI panel: settings[:page] | context | sessions | project
    Open { panel: String },
    /// Export produced this markdown.
    Export { markdown: String, path: String },
    /// Frontend should exit (CLI).
    Exit,
}
