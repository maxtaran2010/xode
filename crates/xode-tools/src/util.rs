//! Small helpers shared by the tools.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use xode_core::tool::ToolCtx;
use xode_core::types::ToolSpec;

/// Build a compact JSON schema. `props` are `(name, type, description)`; empty description = none.
pub fn spec(name: &str, desc: &str, props: &[(&str, &str, &str)], required: &[&str]) -> ToolSpec {
    let mut p = serde_json::Map::new();
    for (k, ty, d) in props {
        let mut o = json!({ "type": ty });
        if !d.is_empty() {
            o["description"] = Value::String((*d).into());
        }
        p.insert((*k).into(), o);
    }
    ToolSpec {
        name: name.into(),
        description: desc.into(),
        parameters: json!({ "type": "object", "properties": p, "required": required }),
    }
}

/// Resolve a path for an existing file: cwd-relative first, then project-relative
/// (paths shown to the model are project-relative, while the shell cwd may have moved).
pub fn resolve_existing(ctx: &ToolCtx, p: &str) -> PathBuf {
    let a = ctx.resolve(p);
    if a.exists() {
        return a;
    }
    let pb = PathBuf::from(p.trim().trim_matches('"'));
    if pb.is_relative() {
        let b = ctx.project_root.join(&pb);
        if b.exists() {
            return b;
        }
    }
    a
}

/// Binary heuristic: NUL byte in the first 8 KiB.
pub fn is_binary(b: &[u8]) -> bool {
    b[..b.len().min(8192)].contains(&0)
}

/// Decode bytes as UTF-8 (lossy), stripping a UTF-8 BOM.
pub fn decode(b: &[u8]) -> String {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    String::from_utf8_lossy(b).into_owned()
}

/// True if the text predominantly uses CRLF line endings.
pub fn is_crlf(s: &str) -> bool {
    let crlf = s.matches("\r\n").count();
    let lf = s.matches('\n').count();
    crlf > 0 && crlf * 2 >= lf
}

pub fn to_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
}

pub fn to_crlf(s: &str) -> String {
    to_lf(s).replace('\n', "\r\n")
}

/// Number of lines as an editor would show it.
pub fn line_count(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        s.lines().count()
    }
}

/// Truncate to at most `max` chars, appending a marker.
pub fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((i, _)) => format!("{}…[+{}ch]", &s[..i], s[i..].chars().count()),
    }
}

pub fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Directories never worth walking.
pub const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", ".xode", ".venv", "__pycache__", ".mypy_cache", ".pytest_cache", ".gradle",
    ".idea", ".vs", ".hg", ".svn",
];

pub fn walker(base: &Path) -> ignore::WalkBuilder {
    let mut w = ignore::WalkBuilder::new(base);
    w.hidden(false).require_git(false).follow_links(false).sort_by_file_name(|a, b| a.cmp(b)).filter_entry(|e| {
        !(e.file_type().is_some_and(|t| t.is_dir()) && e.depth() > 0 && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
    });
    w
}

pub fn build_glob(pat: &str) -> Result<globset::GlobMatcher, String> {
    globset::GlobBuilder::new(pat.trim())
        .literal_separator(false)
        .case_insensitive(cfg!(windows))
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| format!("bad glob: {e}"))
}

#[cfg(test)]
pub mod test {
    use parking_lot::Mutex;
    use std::path::Path;
    use std::sync::Arc;
    use xode_core::config::Config;
    use xode_core::tool::{SessionState, ToolCtx};

    pub fn ctx(root: &Path) -> ToolCtx {
        ctx_with(root, Config::default())
    }

    pub fn ctx_with(root: &Path, cfg: Config) -> ToolCtx {
        ToolCtx {
            session_id: "test".into(),
            project_root: root.to_path_buf(),
            extra_roots: vec![],
            config: Arc::new(cfg),
            state: Arc::new(Mutex::new(SessionState::default())),
            cancel: tokio_util::sync::CancellationToken::new(),
            outliner: None,
        }
    }
}
