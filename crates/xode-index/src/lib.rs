//! Structural code index: tree-sitter symbols + identifier references in SQLite,
//! Aider-style repo map, the `code` tool, and the `Outliner` used after compaction.

mod parse;
mod tool;

pub use parse::{Lang, Parsed, Sym};
pub use tool::code_tool;

use anyhow::Context;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use parking_lot::Mutex;
use xode_db::{params, Connection, OptionalExt};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, UNIX_EPOCH};
use xode_core::tool::{hash_bytes, Outliner};

const SCHEMA_VERSION: i64 = 3;
const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".xode", "__pycache__", ".venv", "venv", "out", "bin", "obj",
    ".next", ".idea", ".vs", ".vscode", "vendor", "coverage",
];
const OUTLINE_MAX_LINES: usize = 60;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IndexStats {
    pub files: u64,
    pub symbols: u64,
    pub ready: bool,
}

/// A symbol row as stored in the index.
#[derive(Debug, Clone)]
pub struct SymRow {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
}

pub struct ProjectIndex {
    root: PathBuf,
    /// Canonical root, for matching watcher paths (symlinked temp dirs, `\\?\` prefixes).
    root_canon: Option<PathBuf>,
    max_bytes: u64,
    conn: Mutex<Connection>,
    ready: AtomicBool,
    /// Bumped on every index write; invalidates the cached reference graph.
    generation: AtomicU64,
    graph: Mutex<Option<(u64, Arc<Graph>)>>,
    watcher: Mutex<Option<Debouncer<RecommendedWatcher>>>,
}

struct FileMeta {
    mtime: i64,
    size: i64,
}

fn file_meta(p: &Path) -> Option<FileMeta> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() {
        return None;
    }
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(FileMeta { mtime, size: m.len() as i64 })
}

fn is_binary(b: &[u8]) -> bool {
    b[..b.len().min(8000)].contains(&0)
}

fn skip_component(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

impl ProjectIndex {
    /// Open (or create) `<root>/.xode/index.db` and start background indexing (+ watcher).
    pub fn open(root: &Path, cfg: &xode_core::config::Tools) -> anyhow::Result<Arc<ProjectIndex>> {
        let dir = root.join(".xode");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let db = dir.join("index.db");
        // The index is derived data: an unreadable file (older format, corruption) is rebuilt.
        let conn = match Connection::open_shared(&db) {
            Err(e) if !e.is_locked() => {
                tracing::warn!("index db unreadable, rebuilding: {e}");
                for ext in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(dir.join(format!("index.db{ext}")));
                }
                Connection::open_shared(&db)?
            }
            r => r?,
        };
        // Another Xode process owns the file: query it read-only, it keeps it fresh.
        let owner = !conn.is_read_only();
        if owner {
            Self::init_db(&conn)?;
        }
        let idx = Arc::new(ProjectIndex {
            root: root.to_path_buf(),
            root_canon: std::fs::canonicalize(root).ok(),
            max_bytes: cfg.index_max_file_kb.max(1) * 1024,
            conn: Mutex::new(conn),
            ready: AtomicBool::new(false),
            generation: AtomicU64::new(1),
            graph: Mutex::new(None),
            watcher: Mutex::new(None),
        });
        if cfg.index_enabled && owner {
            let bg = idx.clone();
            std::thread::Builder::new()
                .name("xode-index".into())
                .spawn(move || {
                    if let Err(e) = bg.full_scan() {
                        tracing::warn!("index scan failed: {e:#}");
                    }
                    bg.ready.store(true, Ordering::SeqCst);
                })?;
            if cfg.index_watch {
                if let Err(e) = idx.start_watcher() {
                    tracing::warn!("index watcher failed: {e:#}");
                }
            }
        } else {
            idx.ready.store(true, Ordering::SeqCst);
        }
        Ok(idx)
    }

    fn init_db(conn: &Connection) -> anyhow::Result<()> {
        if conn.user_version() != SCHEMA_VERSION {
            conn.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS symbols; DROP TABLE IF EXISTS refs;")?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY, mtime INTEGER, size INTEGER, hash TEXT, lang TEXT);
             CREATE TABLE IF NOT EXISTS symbols(file TEXT, name TEXT COLLATE NOCASE, kind TEXT, parent TEXT, depth INTEGER,
                 line_start INTEGER, line_end INTEGER, signature TEXT);
             CREATE TABLE IF NOT EXISTS refs(name TEXT NOT NULL, file TEXT NOT NULL, n INTEGER, lines TEXT,
                 PRIMARY KEY(name, file));
             CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);
             CREATE INDEX IF NOT EXISTS symbols_file ON symbols(file);
             CREATE INDEX IF NOT EXISTS refs_file ON refs(file);",
        )?;
        conn.set_user_version(SCHEMA_VERSION)?;
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Relative `/`-separated path if `p` is inside the root.
    pub fn rel(&self, p: &Path) -> Option<String> {
        let p = if p.is_absolute() { p.to_path_buf() } else { self.root.join(p) };
        let r = p
            .strip_prefix(&self.root)
            .ok()
            .map(Path::to_path_buf)
            .or_else(|| self.root_canon.as_ref().and_then(|c| p.strip_prefix(c).ok().map(Path::to_path_buf)))
            .or_else(|| {
                let c = std::fs::canonicalize(&p).ok()?;
                let rc = self.root_canon.as_ref()?;
                c.strip_prefix(rc).ok().map(Path::to_path_buf)
            })?;
        let mut parts = vec![];
        for c in r.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
                Component::CurDir => {}
                Component::ParentDir => {
                    parts.pop()?;
                }
                _ => return None,
            }
        }
        Some(parts.join("/"))
    }

    pub fn abs(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        for s in rel.split('/') {
            p.push(s);
        }
        p
    }

    fn excluded_rel(rel: &str) -> bool {
        rel.is_empty() || rel.split('/').any(|c| skip_component(c) || (c.starts_with('.') && c.len() > 1))
    }

    // ------------------------------------------------------------------ indexing

    fn full_scan(&self) -> anyhow::Result<()> {
        let known: HashMap<String, (i64, i64, String)> = {
            let c = self.conn.lock();
            c.query_map("SELECT path, mtime, size, hash FROM files", (), |r| Ok((r.get::<String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
                .into_iter()
                .collect()
        };
        let mut seen: HashSet<String> = HashSet::new();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(false)
            .require_git(false)
            .add_custom_ignore_filename(".xodeignore")
            .filter_entry(|e| !e.file_name().to_str().map(skip_component).unwrap_or(false))
            .build();
        // Stat pass: only changed/new files go to the parse workers.
        let mut todo: Vec<(String, PathBuf, Lang, FileMeta)> = vec![];
        for ent in walker.flatten() {
            if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let Some(lang) = Lang::from_path(ent.path()) else { continue };
            let Some(rel) = self.rel(ent.path()) else { continue };
            let Some(meta) = file_meta(ent.path()) else { continue };
            if meta.size as u64 > self.max_bytes {
                continue;
            }
            seen.insert(rel.clone());
            if let Some((m, s, _)) = known.get(&rel) {
                if *m == meta.mtime && *s == meta.size {
                    continue;
                }
            }
            todo.push((rel, ent.into_path(), lang, meta));
        }
        enum Job {
            Parsed(String, FileMeta, String, Lang, Parsed),
            Touch(String, FileMeta),
            Drop(String),
        }
        let workers = std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).clamp(1, 4);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(256);
        let todo_ref = &todo;
        let known_ref = &known;
        let next_ref = &next;
        let mut touch_only: Vec<(String, FileMeta)> = vec![];
        std::thread::scope(|sc| -> anyhow::Result<()> {
            for _ in 0..workers {
                let tx = tx.clone();
                sc.spawn(move || loop {
                    let i = next_ref.fetch_add(1, Ordering::Relaxed);
                    let Some((rel, abs, lang, meta)) = todo_ref.get(i) else { break };
                    let meta = FileMeta { mtime: meta.mtime, size: meta.size };
                    let job = match std::fs::read(abs) {
                        Ok(b) if !is_binary(&b) => {
                            let hash = hash_bytes(&b);
                            if known_ref.get(rel).map(|p| p.2 == hash).unwrap_or(false) {
                                Job::Touch(rel.clone(), meta)
                            } else {
                                let parsed = parse::parse(*lang, &String::from_utf8_lossy(&b));
                                Job::Parsed(rel.clone(), meta, hash, *lang, parsed)
                            }
                        }
                        _ => Job::Drop(rel.clone()),
                    };
                    if tx.send(job).is_err() {
                        break;
                    }
                });
            }
            drop(tx);
            let mut batch = vec![];
            for job in rx {
                match job {
                    Job::Parsed(r, m, h, l, p) => batch.push((r, m, h, l, p)),
                    Job::Touch(r, m) => touch_only.push((r, m)),
                    Job::Drop(r) => {
                        seen.remove(&r);
                    }
                }
                if batch.len() >= 128 {
                    self.write_batch(std::mem::take(&mut batch), known_ref)?;
                }
            }
            self.write_batch(batch, known_ref)?;
            Ok(())
        })?;
        self.conn.lock().transaction(|tx| {
            for (rel, m) in touch_only {
                tx.execute("UPDATE files SET mtime=?2, size=?3 WHERE path=?1", params![rel, m.mtime, m.size])?;
            }
            for rel in known.keys().filter(|k| !seen.contains(*k)) {
                Self::delete_rows(tx, rel)?;
            }
            Ok(())
        })?;
        self.bump();
        Ok(())
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn write_batch(
        &self,
        batch: Vec<(String, FileMeta, String, Lang, Parsed)>,
        known: &HashMap<String, (i64, i64, String)>,
    ) -> anyhow::Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        self.conn.lock().transaction(|tx| {
            for (rel, meta, hash, lang, parsed) in batch {
                // Skip if someone (watcher / file_changed) updated the row since our snapshot.
                let cur: Option<(i64, i64)> = tx
                    .query_row("SELECT mtime, size FROM files WHERE path=?1", params![rel], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let snap = known.get(&rel).map(|k| (k.0, k.1));
                if cur != snap {
                    continue;
                }
                Self::store(tx, &rel, &meta, &hash, lang, &parsed)?;
            }
            Ok(())
        })?;
        self.bump();
        Ok(())
    }

    fn delete_rows(c: &Connection, rel: &str) -> xode_db::Result<()> {
        c.execute("DELETE FROM files WHERE path=?1", params![rel])?;
        c.execute("DELETE FROM symbols WHERE file=?1", params![rel])?;
        c.execute("DELETE FROM refs WHERE file=?1", params![rel])?;
        Ok(())
    }

    fn store(c: &Connection, rel: &str, meta: &FileMeta, hash: &str, lang: Lang, p: &Parsed) -> xode_db::Result<()> {
        Self::delete_rows(c, rel)?;
        c.execute(
            "INSERT INTO files(path, mtime, size, hash, lang) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![rel, meta.mtime, meta.size, hash, lang.name()],
        )?;
        for s in &p.syms {
            c.execute(
                "INSERT INTO symbols(file, name, kind, parent, depth, line_start, line_end, signature) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![rel, s.name, s.kind, s.parent, s.depth, s.line_start, s.line_end, s.signature],
            )?;
        }
        let mut agg: HashMap<&str, Vec<u32>> = HashMap::new();
        for (n, l) in &p.refs {
            agg.entry(n.as_str()).or_default().push(*l);
        }
        for (n, ls) in agg {
            let lines = ls.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(",");
            c.execute("INSERT OR REPLACE INTO refs(name, file, n, lines) VALUES (?1,?2,?3,?4)", params![n, rel, ls.len() as i64, lines])?;
        }
        Ok(())
    }

    /// Reindex one file now (removes it from the index if gone / unindexable). Returns true if indexed.
    pub fn reindex(&self, rel: &str, force: bool) -> bool {
        let abs = self.abs(rel);
        let lang = Lang::from_path(&abs);
        let meta = file_meta(&abs);
        let (Some(lang), Some(meta)) = (lang, meta) else {
            let c = self.conn.lock();
            let _ = Self::delete_rows(&c, rel);
            // A deleted directory: drop everything below it.
            let like = format!("{}/%", rel.replace('%', "\\%").replace('_', "\\_"));
            let _ = c.execute("DELETE FROM symbols WHERE file LIKE ?1 ESCAPE '\\'", params![like]);
            let _ = c.execute("DELETE FROM refs WHERE file LIKE ?1 ESCAPE '\\'", params![like]);
            let _ = c.execute("DELETE FROM files WHERE path LIKE ?1 ESCAPE '\\'", params![like]);
            drop(c);
            self.bump();
            return false;
        };
        let prev: Option<(i64, i64, String)> = self
            .conn
            .lock()
            .query_row("SELECT mtime, size, hash FROM files WHERE path=?1", params![rel], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .ok()
            .flatten();
        if !force {
            if let Some((m, s, _)) = &prev {
                if *m == meta.mtime && *s == meta.size {
                    return true;
                }
            }
        }
        let bytes = match std::fs::read(&abs) {
            Ok(b) if meta.size as u64 <= self.max_bytes && !is_binary(&b) => b,
            _ => {
                let _ = Self::delete_rows(&self.conn.lock(), rel);
                self.bump();
                return false;
            }
        };
        let hash = hash_bytes(&bytes);
        if !force && prev.as_ref().map(|p| p.2 == hash).unwrap_or(false) {
            let _ = self.conn.lock().execute(
                "UPDATE files SET mtime=?2, size=?3 WHERE path=?1",
                params![rel, meta.mtime, meta.size],
            );
            return true;
        }
        let parsed = parse::parse(lang, &String::from_utf8_lossy(&bytes));
        let r = self.conn.lock().transaction(|tx| Self::store(tx, rel, &meta, &hash, lang, &parsed));
        self.bump();
        r.is_ok()
    }

    fn start_watcher(self: &Arc<Self>) -> anyhow::Result<()> {
        let weak: Weak<ProjectIndex> = Arc::downgrade(self);
        let gi = {
            let mut b = ignore::gitignore::GitignoreBuilder::new(&self.root);
            b.add(self.root.join(".gitignore"));
            b.add(self.root.join(".xodeignore"));
            b.build().ok()
        };
        let gi2 = gi.clone();
        let mut deb = new_debouncer(Duration::from_millis(400), move |res: DebounceEventResult| {
            let Some(idx) = weak.upgrade() else { return };
            let Ok(events) = res else { return };
            let mut done = HashSet::new();
            for ev in events {
                let Some(rel) = idx.rel(&ev.path) else { continue };
                if Self::excluded_rel(&rel) || !done.insert(rel.clone()) {
                    continue;
                }
                if let Some(g) = &gi {
                    let is_dir = ev.path.is_dir();
                    if g.matched_path_or_any_parents(&rel, is_dir).is_ignore() {
                        continue;
                    }
                }
                if ev.path.is_dir() {
                    continue;
                }
                idx.reindex(&rel, false);
            }
        })?;
        // Watch root shallowly and each non-skipped top-level dir recursively (avoids huge
        // inotify sets under node_modules/target on Linux).
        deb.watcher().watch(&self.root, RecursiveMode::NonRecursive)?;
        if let Ok(rd) = std::fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) || skip_component(&name) || name.starts_with('.') {
                    continue;
                }
                if let Some(g) = &gi2 {
                    if g.matched(e.path(), true).is_ignore() {
                        continue;
                    }
                }
                let _ = deb.watcher().watch(&e.path(), RecursiveMode::Recursive);
            }
        }
        *self.watcher.lock() = Some(deb);
        Ok(())
    }

    // ------------------------------------------------------------------ queries

    pub fn stats(&self) -> IndexStats {
        let c = self.conn.lock();
        let files: i64 = c.scalar("SELECT COUNT(*) FROM files", ()).ok().flatten().unwrap_or(0);
        let symbols: i64 = c.scalar("SELECT COUNT(*) FROM symbols", ()).ok().flatten().unwrap_or(0);
        IndexStats { files: files as u64, symbols: symbols as u64, ready: self.is_ready() }
    }

    fn row(r: &xode_db::Row) -> xode_db::Result<SymRow> {
        Ok(SymRow {
            file: r.get(0)?,
            name: r.get(1)?,
            kind: r.get(2)?,
            parent: r.get(3)?,
            depth: r.get(4)?,
            line_start: r.get(5)?,
            line_end: r.get(6)?,
            signature: r.get(7)?,
        })
    }

    /// Symbols of an indexed file, in line order.
    pub fn file_symbols(&self, rel: &str) -> Vec<SymRow> {
        self.conn
            .lock()
            .query_map(
                "SELECT file,name,kind,parent,depth,line_start,line_end,signature FROM symbols WHERE file=?1 ORDER BY line_start, depth",
                params![rel],
                Self::row,
            )
            .unwrap_or_default()
    }

    /// Definitions whose name matches `q`: exact (case-sensitive) first, then case-insensitive
    /// exact, prefix, substring.
    pub fn find(&self, q: &str, limit: usize) -> Vec<SymRow> {
        let q = q.trim();
        if q.is_empty() {
            return vec![];
        }
        let esc = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        self.conn
            .lock()
            .query_map(
                "SELECT file,name,kind,parent,depth,line_start,line_end,signature FROM symbols
             WHERE name LIKE ?2 ESCAPE '\\'
             ORDER BY (name = ?1 COLLATE BINARY) DESC, (name = ?1) DESC, (name LIKE ?3 ESCAPE '\\') DESC,
                      length(name), depth, file, line_start LIMIT ?4",
                params![q, format!("%{esc}%"), format!("{esc}%"), limit as i64],
                Self::row,
            )
            .unwrap_or_default()
    }

    /// Exact-name definitions (case-insensitive), optionally filtered by parent.
    pub fn defs(&self, name: &str, parent: Option<&str>) -> Vec<SymRow> {
        let rows: Vec<SymRow> = self
            .conn
            .lock()
            .query_map(
                "SELECT file,name,kind,parent,depth,line_start,line_end,signature FROM symbols WHERE name = ?1
             ORDER BY (name = ?1 COLLATE BINARY) DESC, file, line_start LIMIT 200",
                params![name],
                Self::row,
            )
            .unwrap_or_default();
        match parent {
            Some(p) => rows.into_iter().filter(|r| r.parent.as_deref().map(|x| x.eq_ignore_ascii_case(p)).unwrap_or(false)).collect(),
            None => rows,
        }
    }

    /// Reference locations (file, line), exact name; falls back to case-insensitive.
    pub fn refs(&self, name: &str) -> Vec<(String, u32)> {
        let c = self.conn.lock();
        let q = |sql: &str| -> Vec<(String, String)> {
            c.query_map(sql, params![name], |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or_default()
        };
        let mut v = q("SELECT file, lines FROM refs WHERE name = ?1 ORDER BY file");
        if v.is_empty() {
            v = q("SELECT file, lines FROM refs WHERE name = ?1 COLLATE NOCASE ORDER BY file");
        }
        let mut out = vec![];
        for (f, lines) in v {
            for l in lines.split(',').filter_map(|x| x.parse::<u32>().ok()) {
                out.push((f.clone(), l));
            }
        }
        out
    }

    /// Indexed files under a directory prefix ("" = all).
    pub fn files_under(&self, dir: &str) -> Vec<String> {
        let c = self.conn.lock();
        let dir = dir.trim_matches('/');
        let like = if dir.is_empty() {
            "%".to_string()
        } else {
            format!("{}/%", dir.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
        };
        c.query_map("SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\' ORDER BY path", params![like], |r| r.get(0))
            .unwrap_or_default()
    }

    /// Symbol names starting with `prefix` (case-insensitive), shortest first. For @-mention autocomplete.
    pub fn symbol_names(&self, prefix: &str, limit: usize) -> Vec<String> {
        let esc = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let c = self.conn.lock();
        c.query_map(
            "SELECT name FROM symbols WHERE name LIKE ?1 ESCAPE '\\' GROUP BY name ORDER BY length(name), name LIMIT ?2",
            params![format!("{esc}%"), limit as i64],
            |r| r.get(0),
        )
        .unwrap_or_default()
    }

    /// Indexed file paths matching `prefix` (path prefix or file-name prefix first, then substring).
    pub fn file_paths(&self, prefix: &str, limit: usize) -> Vec<String> {
        let p = prefix.replace('\\', "/");
        let esc = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let c = self.conn.lock();
        c.query_map(
            "SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\'
             ORDER BY (path LIKE ?2 ESCAPE '\\') DESC, (path LIKE ?3 ESCAPE '\\') DESC, length(path), path LIMIT ?4",
            params![format!("%{esc}%"), format!("{esc}%"), format!("%/{esc}%"), limit as i64],
            |r| r.get(0),
        )
        .unwrap_or_default()
    }

    /// Parse a file from disk right now (no DB).
    pub fn parse_file(path: &Path) -> Option<(Lang, String, Parsed)> {
        let lang = Lang::from_path(path)?;
        let bytes = std::fs::read(path).ok()?;
        if is_binary(&bytes) {
            return None;
        }
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let p = parse::parse(lang, &src);
        Some((lang, src, p))
    }

    /// Compact outline text for symbols.
    pub fn render_outline<'a>(syms: impl Iterator<Item = (&'a str, u32, u32, u32)>, max: usize) -> String {
        let all: Vec<_> = syms.collect();
        let mut out = String::new();
        for (sig, d, a, b) in all.iter().take(max) {
            let ind = "  ".repeat((*d).min(6) as usize);
            if a == b {
                out.push_str(&format!("{ind}L{a} {sig}\n"));
            } else {
                out.push_str(&format!("{ind}L{a}-{b} {sig}\n"));
            }
        }
        if all.len() > max {
            out.push_str(&format!("…+{} more\n", all.len() - max));
        }
        out.trim_end().to_string()
    }

    /// Outline of a file path (absolute or root-relative). Indexes on demand.
    pub fn outline_file(&self, path: &Path) -> Option<String> {
        let abs = if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) };
        Lang::from_path(&abs)?;
        match self.rel(&abs).filter(|r| !Self::excluded_rel(r)) {
            Some(rel) => {
                if !self.reindex(&rel, false) {
                    return self.outline_direct(&abs);
                }
                let syms = self.file_symbols(&rel);
                if syms.is_empty() {
                    return None;
                }
                Some(Self::render_outline(
                    syms.iter().map(|s| (s.signature.as_str(), s.depth, s.line_start, s.line_end)),
                    OUTLINE_MAX_LINES,
                ))
            }
            None => self.outline_direct(&abs),
        }
    }

    fn outline_direct(&self, abs: &Path) -> Option<String> {
        let (_, _, p) = Self::parse_file(abs)?;
        if p.syms.is_empty() {
            return None;
        }
        Some(Self::render_outline(
            p.syms.iter().map(|s| (s.signature.as_str(), s.depth, s.line_start, s.line_end)),
            OUTLINE_MAX_LINES,
        ))
    }

    // ------------------------------------------------------------------ repo map

    fn graph(&self) -> Arc<Graph> {
        let gen = self.generation.load(Ordering::SeqCst);
        if let Some((g, gr)) = &*self.graph.lock() {
            if *g == gen {
                return gr.clone();
            }
        }
        let gr = Arc::new(self.build_graph());
        *self.graph.lock() = Some((gen, gr.clone()));
        gr
    }

    /// File reference graph: edge src→dst for each identifier referenced in src and defined in dst.
    fn build_graph(&self) -> Graph {
        let mut g = Graph::default();
        let c = self.conn.lock();
        // name -> defining file ids
        let mut defs: HashMap<String, Vec<u32>> = HashMap::new();
        let _ = c.for_each("SELECT file, name FROM symbols", (), |r| {
            if let (Ok(f), Ok(n)) = (r.get::<String>(0), r.get::<String>(1)) {
                let fid = g.file_id(&f);
                let v = defs.entry(n).or_default();
                if !v.contains(&fid) {
                    v.push(fid);
                }
            }
            true
        });
        g.with_syms = g.files.len();
        let _ = c.for_each("SELECT name, file, n FROM refs", (), |r| {
            let (Ok(name), Ok(file)) = (r.get::<String>(0), r.get::<String>(1)) else { return true };
            let Some(dsts) = defs.get(&name) else { return true };
            if dsts.len() > 20 {
                return true;
            }
            let cnt: i64 = r.get(2).unwrap_or(1);
            let nd = dsts.len() as f32;
            let mut w = (cnt as f32).sqrt() / nd;
            if name.len() < 3 || name.starts_with('_') {
                w *= 0.1;
            } else if name.bytes().all(|b| b.is_ascii_lowercase()) {
                // Single lowercase words (set, take, insert…) are often calls on std types.
                w *= 0.25;
            } else if name.len() >= 8 && (name.contains('_') || name.bytes().skip(1).any(|b| b.is_ascii_uppercase())) {
                w *= 2.0;
            }
            let dsts = dsts.clone();
            let src = g.file_id(&file);
            let nid = g.name_id(&name);
            for d in dsts {
                if d != src {
                    g.edges.push((src, d, nid, w));
                }
            }
            true
        });
        g
    }

    /// Aider-style repo map: files ranked by cross-file references (PageRank over the
    /// file reference graph, personalized towards `focus`), each with its most-referenced
    /// symbol signatures, within `budget_tokens`.
    pub fn repo_map(&self, budget_tokens: u64, focus: &[String]) -> String {
        let g = self.graph();
        let n = g.files.len();
        if g.with_syms == 0 {
            return String::new();
        }
        let focus_ids: HashSet<u32> = focus
            .iter()
            .filter_map(|f| g.ids.get(f.trim_start_matches("./").replace('\\', "/").as_str()).copied())
            .collect();
        let fw = |s: u32| if focus_ids.contains(&s) { 10.0f64 } else { 1.0 };
        let mut outw = vec![0f64; n];
        for (s, _, _, w) in &g.edges {
            outw[*s as usize] += *w as f64 * fw(*s);
        }
        // Personalized PageRank.
        let mut pers = vec![1.0f64; n];
        for f in &focus_ids {
            pers[*f as usize] += n as f64 / focus_ids.len().max(1) as f64;
        }
        let ps: f64 = pers.iter().sum();
        pers.iter_mut().for_each(|x| *x /= ps);
        let mut rank = pers.clone();
        for _ in 0..20 {
            let mut next = vec![0f64; n];
            let dangling: f64 = (0..n).filter(|i| outw[*i] == 0.0).map(|i| rank[i]).sum();
            for (s, d, _, w) in &g.edges {
                let s = *s as usize;
                next[*d as usize] += 0.85 * rank[s] * (*w as f64 * fw(s as u32)) / outw[s];
            }
            for i in 0..n {
                next[i] += (0.15 + 0.85 * dangling) * pers[i];
            }
            rank = next;
        }
        let mut def_rank: HashMap<(u32, u32), f64> = HashMap::new();
        for (s, d, nm, w) in &g.edges {
            let s = *s as usize;
            let r = rank[s] * (*w as f64 * fw(s as u32)) / outw[s];
            *def_rank.entry((*d, *nm)).or_default() += r;
        }
        let score: Vec<f64> = (0..n)
            .map(|i| if is_aux_path(&g.files[i]) && !focus_ids.contains(&(i as u32)) { rank[i] * 0.1 } else { rank[i] })
            .collect();
        let mut files: Vec<usize> = (0..g.with_syms).collect();
        files.sort_by(|a, b| {
            score[*b].partial_cmp(&score[*a]).unwrap_or(std::cmp::Ordering::Equal).then(g.files[*a].cmp(&g.files[*b]))
        });

        let mut out = String::new();
        let mut used = 0u64;
        let mut skipped = 0usize;
        for (pos, f) in files.iter().enumerate() {
            let fs = self.file_symbols(&g.files[*f]);
            if fs.is_empty() {
                continue;
            }
            let per_file = if pos < 5 { 10 } else if pos < 20 { 6 } else { 3 };
            let dr = |s: &SymRow| {
                g.name_ids.get(s.name.as_str()).and_then(|nm| def_rank.get(&(*f as u32, *nm))).copied().unwrap_or(0.0)
            };
            // Top symbols by rank (shallower first on ties), one per name, emitted in line order.
            let mut cand: Vec<(&SymRow, f64)> = fs
                .iter()
                .filter(|s| s.depth <= 2 && !matches!(s.kind.as_str(), "key" | "heading" | "table") || s.depth == 0)
                .filter(|s| !(s.kind == "mod" && s.line_start == s.line_end))
                .map(|s| (s, if s.depth > 0 { dr(s) * 0.5 } else { dr(s) }))
                .collect();
            cand.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.depth.cmp(&b.0.depth))
                    .then(a.0.line_start.cmp(&b.0.line_start))
            });
            let mut seen_names = HashSet::new();
            cand.retain(|(s, _)| seen_names.insert(s.name.as_str()));
            let mut k = per_file.min(cand.len());
            loop {
                let mut pick: Vec<&SymRow> = cand[..k].iter().map(|x| x.0).collect();
                // Add enclosing containers so nested members have context.
                let mut i = 0;
                while i < pick.len() {
                    let s = pick[i];
                    if s.depth > 0 {
                        let par = fs
                            .iter()
                            .filter(|p| p.depth + 1 == s.depth && p.line_start <= s.line_start && p.line_end >= s.line_end)
                            .last();
                        if let Some(p) = par {
                            if !pick.iter().any(|x| std::ptr::eq(*x, p)) {
                                pick.push(p);
                            }
                        }
                    }
                    i += 1;
                }
                pick.sort_by_key(|s| (s.line_start, s.depth));
                let mut chunk = format!("{}:\n", g.files[*f]);
                for s in &pick {
                    chunk.push_str(&"  ".repeat(s.depth.min(3) as usize + 1));
                    chunk.push_str(&parse::clip(&s.signature, 100));
                    chunk.push('\n');
                }
                let t = xode_core::tokens::count(&chunk);
                if used + t <= budget_tokens {
                    used += t;
                    out.push_str(&chunk);
                    break;
                }
                if k == 0 {
                    skipped += 1;
                    break;
                }
                k /= 2;
            }
            if skipped > 3 {
                break;
            }
        }
        out.trim_end().to_string()
    }
}

/// Tests, examples, benches, fixtures: rarely what the map should lead with.
fn is_aux_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    let name = l.rsplit('/').next().unwrap_or(&l);
    l.split('/').any(|c| {
        matches!(c, "test" | "tests" | "__tests__" | "spec" | "specs" | "examples" | "example" | "benches" | "bench" | "fixtures" | "testdata" | "third_party")
    }) || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_spec.")
}

#[derive(Default)]
struct Graph {
    files: Vec<String>,
    ids: HashMap<String, u32>,
    /// Files `0..with_syms` define symbols.
    with_syms: usize,
    names: Vec<String>,
    name_ids: HashMap<String, u32>,
    /// (src file, dst file, name, weight)
    edges: Vec<(u32, u32, u32, f32)>,
}

impl Graph {
    fn file_id(&mut self, f: &str) -> u32 {
        if let Some(i) = self.ids.get(f) {
            return *i;
        }
        self.files.push(f.to_string());
        self.ids.insert(f.to_string(), self.files.len() as u32 - 1);
        self.files.len() as u32 - 1
    }
    fn name_id(&mut self, n: &str) -> u32 {
        if let Some(i) = self.name_ids.get(n) {
            return *i;
        }
        self.names.push(n.to_string());
        self.name_ids.insert(n.to_string(), self.names.len() as u32 - 1);
        self.names.len() as u32 - 1
    }
}

impl Outliner for ProjectIndex {
    fn outline(&self, path: &Path) -> Option<String> {
        self.outline_file(path)
    }

    fn file_changed(&self, path: &Path) {
        if let Some(rel) = self.rel(path) {
            if !Self::excluded_rel(&rel) {
                self.reindex(&rel, true);
            }
        }
    }
}

#[cfg(test)]
mod tests;

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProjectIndex>();
};
