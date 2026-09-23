//! Knowledge base: four layers of markdown notes, indexed in Turso (FTS + vectors), searched
//! compactly by the `kb` tool and browsed / graphed by the app.
//!
//! Layers:
//! - `library`         global, user-provided folders (read-only for the agent)
//! - `docs`            per project, user-provided folders (read-only for the agent)
//! - `memory`          global, written by the agent (and the user)
//! - `project_memory`  per project, written by the agent (and the user)
//!
//! Files on disk are the source of truth; each store's DB (`<appdata>/kb.db`,
//! `<project>/.xode/kb.db`) is derived and rebuilt when unreadable.

pub mod embed;
pub mod md;
mod search;
pub mod store;
mod tool;

pub use embed::{EmbedHub, Embedder};
pub use search::{Hit, SearchOpts};
pub use store::{KbStore, NoteView, Progress};
pub use tool::kb_tool;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Library,
    Docs,
    Memory,
    ProjectMemory,
}

impl Layer {
    pub const ALL: [Layer; 4] = [Layer::Library, Layer::Docs, Layer::Memory, Layer::ProjectMemory];

    pub fn key(self) -> &'static str {
        match self {
            Layer::Library => "library",
            Layer::Docs => "docs",
            Layer::Memory => "memory",
            Layer::ProjectMemory => "project_memory",
        }
    }

    pub fn parse(s: &str) -> Option<Layer> {
        Some(match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "library" | "lib" => Layer::Library,
            "docs" | "project_docs" => Layer::Docs,
            "memory" | "global_memory" => Layer::Memory,
            "project_memory" | "pmemory" => Layer::ProjectMemory,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Layer::Library => "Library",
            Layer::Docs => "Project docs",
            Layer::Memory => "Memory",
            Layer::ProjectMemory => "Project memory",
        }
    }

    /// Written by the agent (and editable in the app).
    pub fn writable(self) -> bool {
        matches!(self, Layer::Memory | Layer::ProjectMemory)
    }

    pub fn global(self) -> bool {
        matches!(self, Layer::Library | Layer::Memory)
    }
}

/// A folder indexed into a store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    /// Stable key across stores: `g:3` (global) / `p:1` (project).
    pub key: String,
    pub id: i64,
    pub layer: Layer,
    pub name: String,
    pub path: String,
    /// On for new chats.
    pub default_on: bool,
    pub notes: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub bytes: u64,
}

/// Per-chat selection: layer keys and source keys switched off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sel {
    pub off: HashSet<String>,
}

impl Sel {
    pub fn new(off: &[String]) -> Self {
        Sel { off: off.iter().cloned().collect() }
    }

    pub fn allows(&self, layer: Layer, source_key: &str) -> bool {
        !self.off.contains(layer.key()) && !self.off.contains(source_key)
    }

    pub fn layer_on(&self, layer: Layer) -> bool {
        !self.off.contains(layer.key())
    }
}

/// Selection for a new chat: every source with `default_on = false` starts off.
pub fn default_off(stores: &[&Arc<KbStore>]) -> Vec<String> {
    stores.iter().flat_map(|s| s.sources()).filter(|s| !s.default_on).map(|s| s.key).collect()
}

/// A session's view: the global store plus the project's store.
#[derive(Clone)]
pub struct Kb {
    pub global: Option<Arc<KbStore>>,
    pub project: Option<Arc<KbStore>>,
}

impl Kb {
    pub fn stores(&self) -> impl Iterator<Item = &Arc<KbStore>> {
        self.global.iter().chain(self.project.iter())
    }

    /// Store owning a note / source id (`g12`, `p:3` …).
    pub fn store_for(&self, id: &str) -> Option<&Arc<KbStore>> {
        match id.chars().next()? {
            'g' => self.global.as_ref(),
            'p' => self.project.as_ref(),
            _ => None,
        }
    }

    pub fn sources(&self) -> Vec<Source> {
        self.stores().flat_map(|s| s.sources()).collect()
    }

    /// Anything searchable under this selection?
    pub fn any_enabled(&self, sel: &Sel) -> bool {
        self.sources().iter().any(|s| sel.allows(s.layer, &s.key) && (s.notes > 0 || s.layer.writable()))
    }

    /// One line for the system prompt: enabled layers with note counts.
    pub fn prompt_line(&self, sel: &Sel) -> Option<String> {
        let srcs = self.sources();
        let mut parts = vec![];
        for l in Layer::ALL {
            let n: u64 = srcs.iter().filter(|s| s.layer == l && sel.allows(l, &s.key)).map(|s| s.notes).sum();
            let any = srcs.iter().any(|s| s.layer == l && sel.allows(l, &s.key));
            if any && (n > 0 || l.writable()) {
                parts.push(format!("{} {n}", l.label()));
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "Knowledge base (kb tool; notes: {}). Search it before guessing about conventions, prior decisions or reference material; save durable findings with kb write.",
            parts.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests;
