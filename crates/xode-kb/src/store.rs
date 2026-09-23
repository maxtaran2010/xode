//! One knowledge store (global or per project): sources, notes, chunks, links in a Turso DB,
//! kept in sync with the source folders by a scan/watch worker and an embedding worker.

use crate::embed::{bits_to_bytes, bytes_to_bits, sign_bits, EmbedHub};
use crate::md::{self, Kind};
use crate::{Layer, Source};
use anyhow::{anyhow, bail, Result};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult, Debouncer};
use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Weak};
use std::time::{Duration, UNIX_EPOCH};
use xode_core::config::Knowledge;
use xode_db::{params, Connection, OptionalExt};

const SCHEMA_VERSION: i64 = 1;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".obsidian", ".trash", "__pycache__", ".venv", ".xode"];

/// Indexing progress for the UI (`stage`: scan | index | embed | idle).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Progress {
    /// Source key (`g:1`), or the store prefix (`g` / `p`) for store-wide stages (embed).
    pub source: String,
    pub stage: String,
    pub done: u64,
    pub total: u64,
}

pub type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NoteLink {
    /// Resolved note id (`g12`), if the target exists.
    pub id: Option<String>,
    pub title: String,
}

/// A note with its content, as the app and `kb read` see it.
#[derive(Debug, Clone, Serialize)]
pub struct NoteView {
    pub id: String,
    pub title: String,
    pub source: String,
    pub source_name: String,
    pub layer: Layer,
    /// Path relative to the source folder.
    pub rel: String,
    pub abs: String,
    pub tags: Vec<String>,
    pub summary: String,
    pub tokens: u32,
    pub headings: Vec<(u8, String, u32)>,
    pub links: Vec<NoteLink>,
    pub backlinks: Vec<NoteLink>,
    pub text: String,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteRow {
    pub id: String,
    pub title: String,
    pub source: String,
    pub rel: String,
    pub tokens: u32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub layer: Layer,
    pub source: String,
    pub tokens: u32,
    pub degree: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(String, String)>,
    /// Nodes left out by the limit.
    pub hidden: u64,
}

enum Job {
    Scan(i64, bool),
    Path(i64, PathBuf),
}

/// In-memory sign-bit codes of embedded chunks (first, approximate vector stage).
#[derive(Default)]
pub(crate) struct BitIndex {
    words: usize,
    ids: Vec<i64>,
    src: Vec<i64>,
    codes: Vec<u64>,
    pos: HashMap<i64, usize>,
}

impl BitIndex {
    fn insert(&mut self, id: i64, src: i64, code: &[u64]) {
        if self.words == 0 {
            self.words = code.len();
        }
        if code.len() != self.words {
            return;
        }
        if let Some(&p) = self.pos.get(&id) {
            self.codes[p * self.words..(p + 1) * self.words].copy_from_slice(code);
            self.src[p] = src;
            return;
        }
        self.pos.insert(id, self.ids.len());
        self.ids.push(id);
        self.src.push(src);
        self.codes.extend_from_slice(code);
    }

    fn remove(&mut self, id: i64) {
        let Some(p) = self.pos.remove(&id) else { return };
        let last = self.ids.len() - 1;
        if p != last {
            let lid = self.ids[last];
            self.ids.swap(p, last);
            self.src.swap(p, last);
            let w = self.words;
            for i in 0..w {
                self.codes.swap(p * w + i, last * w + i);
            }
            self.pos.insert(lid, p);
        }
        self.ids.pop();
        self.src.pop();
        self.codes.truncate(last * self.words);
    }

    fn clear(&mut self) {
        *self = BitIndex::default();
    }

    pub(crate) fn len(&self) -> usize {
        self.ids.len()
    }

    /// Nearest `n` chunk ids by Hamming distance among allowed sources.
    pub(crate) fn top(&self, q: &[u64], allowed: &HashSet<i64>, n: usize) -> Vec<i64> {
        if self.words == 0 || q.len() != self.words || n == 0 {
            return vec![];
        }
        let w = self.words;
        let mut heap: std::collections::BinaryHeap<(u32, usize)> = std::collections::BinaryHeap::with_capacity(n + 1);
        for i in 0..self.ids.len() {
            if !allowed.contains(&self.src[i]) {
                continue;
            }
            let c = &self.codes[i * w..(i + 1) * w];
            let d: u32 = c.iter().zip(q).map(|(a, b)| (a ^ b).count_ones()).sum();
            if heap.len() < n {
                heap.push((d, i));
            } else if d < heap.peek().map(|x| x.0).unwrap_or(u32::MAX) {
                heap.pop();
                heap.push((d, i));
            }
        }
        let mut v = heap.into_vec();
        v.sort();
        v.into_iter().map(|(_, i)| self.ids[i]).collect()
    }
}

pub struct KbStore {
    /// `g` (global) or `p` (project).
    pub prefix: char,
    memory_dir: PathBuf,
    memory_layer: Layer,
    pub(crate) conn: Mutex<Connection>,
    owner: bool,
    pub(crate) bits: RwLock<BitIndex>,
    pub(crate) hub: Arc<EmbedHub>,
    pub(crate) cfg: RwLock<Knowledge>,
    progress: RwLock<Option<ProgressFn>>,
    jobs: Mutex<Option<Sender<Job>>>,
    wake: (Mutex<bool>, Condvar),
    watchers: Mutex<HashMap<i64, Debouncer<RecommendedWatcher>>>,
    generation: AtomicU64,
    stop: AtomicBool,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn file_meta(p: &Path) -> Option<(i64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_file() {
        return None;
    }
    let mt = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some((mt, m.len() as i64))
}

fn rel_of(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let s = r.to_string_lossy().replace('\\', "/");
    (!s.is_empty()).then_some(s)
}

fn skipped(rel: &str) -> bool {
    rel.split('/').any(|c| SKIP_DIRS.contains(&c) || (c.starts_with('.') && c.len() > 1))
}

fn tags_col(tags: &[String]) -> String {
    if tags.is_empty() {
        String::new()
    } else {
        format!(",{},", tags.join(","))
    }
}

fn tags_vec(s: &str) -> Vec<String> {
    s.split(',').filter(|t| !t.is_empty()).map(String::from).collect()
}

impl KbStore {
    /// Open (or create) a store. `memory_dir` holds the agent-written notes of `memory_layer`.
    pub fn open(
        prefix: char,
        db_path: &Path,
        memory_dir: &Path,
        memory_layer: Layer,
        hub: Arc<EmbedHub>,
        cfg: &Knowledge,
    ) -> Result<Arc<Self>> {
        if let Some(d) = db_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let conn = match Connection::open_shared(db_path) {
            Err(e) if !e.is_locked() => {
                tracing::warn!("kb db unreadable, rebuilding: {e}");
                let name = db_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                for ext in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(db_path.with_file_name(format!("{name}{ext}")));
                }
                Connection::open_shared(db_path)?
            }
            r => r?,
        };
        let owner = !conn.is_read_only();
        if owner {
            init_db(&conn)?;
        }
        let st = Arc::new(KbStore {
            prefix,
            memory_dir: memory_dir.to_path_buf(),
            memory_layer,
            conn: Mutex::new(conn),
            owner,
            bits: RwLock::new(BitIndex::default()),
            hub,
            cfg: RwLock::new(cfg.clone()),
            progress: RwLock::new(None),
            jobs: Mutex::new(None),
            wake: (Mutex::new(false), Condvar::new()),
            watchers: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });
        if owner {
            std::fs::create_dir_all(memory_dir).ok();
            st.ensure_memory_source()?;
        }
        st.load_bits();
        if owner {
            st.start_workers()?;
            for s in st.sources() {
                st.watch(s.id, Path::new(&s.path));
                st.queue(Job::Scan(s.id, false));
            }
        }
        Ok(st)
    }

    /// Stop background work (store is being dropped / closed).
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        *self.jobs.lock() = None;
        self.watchers.lock().clear();
        self.poke();
    }

    pub fn is_owner(&self) -> bool {
        self.owner
    }

    pub fn set_progress(&self, f: ProgressFn) {
        *self.progress.write() = Some(f);
    }

    pub fn set_config(&self, cfg: &Knowledge) {
        *self.cfg.write() = cfg.clone();
        self.poke();
    }

    /// Bumped whenever notes change (graph caches, UI refresh).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn emit(&self, source: String, stage: &str, done: u64, total: u64) {
        if let Some(f) = &*self.progress.read() {
            f(Progress { source, stage: stage.into(), done, total });
        }
    }

    pub(crate) fn key(&self, id: i64) -> String {
        format!("{}{id}", self.prefix)
    }

    pub(crate) fn src_key(&self, id: i64) -> String {
        format!("{}:{id}", self.prefix)
    }

    /// Parse `g12` → 12 (only for this store).
    pub(crate) fn parse_id(&self, id: &str) -> Option<i64> {
        let id = id.trim();
        let rest = id.strip_prefix(self.prefix)?;
        rest.trim_start_matches(':').parse().ok()
    }

    fn ensure_memory_source(&self) -> Result<()> {
        let c = self.conn.lock();
        let have: Option<i64> =
            c.scalar("SELECT id FROM sources WHERE layer=?1", params![self.memory_layer.key()])?;
        if have.is_none() {
            c.execute(
                "INSERT INTO sources(layer, name, path, default_on, added) VALUES (?1,?2,?3,1,?4)",
                params![self.memory_layer.key(), self.memory_layer.label(), self.memory_dir.to_string_lossy().to_string(), now()],
            )?;
        } else {
            c.execute(
                "UPDATE sources SET path=?2 WHERE layer=?1",
                params![self.memory_layer.key(), self.memory_dir.to_string_lossy().to_string()],
            )?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ sources

    pub fn sources(&self) -> Vec<Source> {
        let c = self.conn.lock();
        let mut stats: HashMap<i64, (u64, u64)> = HashMap::new();
        let _ = c.for_each("SELECT source, COUNT(*), SUM(size) FROM notes GROUP BY source", (), |r| {
            stats.insert(r.get(0).unwrap_or(0), (r.get(1).unwrap_or(0), r.get(2).unwrap_or(0)));
            true
        });
        let mut ch: HashMap<i64, (u64, u64)> = HashMap::new();
        let _ = c.for_each("SELECT source, COUNT(*), SUM(emb IS NOT NULL) FROM chunks GROUP BY source", (), |r| {
            ch.insert(r.get(0).unwrap_or(0), (r.get(1).unwrap_or(0), r.get(2).unwrap_or(0)));
            true
        });
        c.query_map("SELECT id, layer, name, path, default_on FROM sources ORDER BY id", (), |r| {
            let id: i64 = r.get(0)?;
            let layer = Layer::parse(&r.get::<String>(1)?).unwrap_or(Layer::Library);
            let (notes, bytes) = stats.get(&id).copied().unwrap_or_default();
            let (chunks, embedded) = ch.get(&id).copied().unwrap_or_default();
            Ok(Source {
                key: self.src_key(id),
                id,
                layer,
                name: r.get(2)?,
                path: r.get(3)?,
                default_on: r.get(4)?,
                notes,
                chunks,
                embedded,
                bytes,
            })
        })
        .unwrap_or_default()
    }

    fn source(&self, id: i64) -> Option<(Layer, String, PathBuf)> {
        self.conn
            .lock()
            .query_row("SELECT layer, name, path FROM sources WHERE id=?1", params![id], |r| {
                Ok((Layer::parse(&r.get::<String>(0)?).unwrap_or(Layer::Library), r.get(1)?, PathBuf::from(r.get::<String>(2)?)))
            })
            .ok()
    }

    pub fn add_source(self: &Arc<Self>, layer: Layer, path: &Path, name: &str) -> Result<Source> {
        if !self.owner {
            bail!("knowledge base is managed by another Xode window");
        }
        if layer.writable() {
            bail!("{} has a single built-in folder", layer.label());
        }
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !path.is_dir() {
            bail!("not a folder: {}", path.display());
        }
        let ps = path.to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let name = if name.trim().is_empty() {
            path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| ps.clone())
        } else {
            name.trim().to_string()
        };
        let id = {
            let c = self.conn.lock();
            if c.scalar::<i64>("SELECT id FROM sources WHERE path=?1", params![ps])?.is_some() {
                bail!("already added: {ps}");
            }
            c.execute(
                "INSERT INTO sources(layer, name, path, default_on, added) VALUES (?1,?2,?3,1,?4)",
                params![layer.key(), name, ps, now()],
            )?;
            c.last_insert_rowid()
        };
        self.watch(id, &path);
        self.queue(Job::Scan(id, false));
        self.sources().into_iter().find(|s| s.id == id).ok_or_else(|| anyhow!("source vanished"))
    }

    pub fn remove_source(&self, id: i64) -> Result<()> {
        if !self.owner {
            bail!("knowledge base is managed by another Xode window");
        }
        let (layer, _, _) = self.source(id).ok_or_else(|| anyhow!("no such source"))?;
        if layer.writable() {
            bail!("the memory folder cannot be removed");
        }
        self.watchers.lock().remove(&id);
        let ids: Vec<i64> = self.conn.lock().query_map("SELECT id FROM chunks WHERE source=?1", params![id], |r| r.get(0))?;
        {
            let mut b = self.bits.write();
            ids.iter().for_each(|i| b.remove(*i));
        }
        self.conn.lock().transaction(|c| {
            c.execute("DELETE FROM links WHERE src IN (SELECT id FROM notes WHERE source=?1)", params![id])?;
            c.execute("UPDATE links SET dst=NULL WHERE dst IN (SELECT id FROM notes WHERE source=?1)", params![id])?;
            c.execute("DELETE FROM chunks WHERE source=?1", params![id])?;
            c.execute("DELETE FROM notes WHERE source=?1", params![id])?;
            c.execute("DELETE FROM sources WHERE id=?1", params![id])?;
            Ok(())
        })?;
        self.bump();
        Ok(())
    }

    pub fn update_source(&self, id: i64, name: Option<&str>, default_on: Option<bool>) -> Result<()> {
        if !self.owner {
            bail!("knowledge base is managed by another Xode window");
        }
        let c = self.conn.lock();
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            c.execute("UPDATE sources SET name=?2 WHERE id=?1", params![id, n.trim()])?;
        }
        if let Some(d) = default_on {
            c.execute("UPDATE sources SET default_on=?2 WHERE id=?1", params![id, d])?;
        }
        Ok(())
    }

    /// Re-read every file of a source (`force`: even unchanged ones).
    pub fn reindex(&self, id: i64, force: bool) {
        self.queue(Job::Scan(id, force));
    }

    // ------------------------------------------------------------------ workers

    fn queue(&self, j: Job) {
        if let Some(tx) = &*self.jobs.lock() {
            let _ = tx.send(j);
        }
    }

    fn poke(&self) {
        *self.wake.0.lock() = true;
        self.wake.1.notify_all();
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    fn start_workers(self: &Arc<Self>) -> Result<()> {
        let (tx, rx) = channel::<Job>();
        *self.jobs.lock() = Some(tx);
        let weak: Weak<KbStore> = Arc::downgrade(self);
        std::thread::Builder::new().name("xode-kb-index".into()).spawn(move || {
            while let Ok(job) = rx.recv() {
                let Some(st) = weak.upgrade() else { break };
                if st.stop.load(Ordering::SeqCst) {
                    break;
                }
                let r = match job {
                    Job::Scan(id, force) => st.scan(id, force),
                    Job::Path(id, p) => st.sync_path(id, &p),
                };
                if let Err(e) = r {
                    tracing::warn!("kb index: {e:#}");
                }
                st.poke();
            }
        })?;
        let weak: Weak<KbStore> = Arc::downgrade(self);
        std::thread::Builder::new().name("xode-kb-embed".into()).spawn(move || loop {
            let Some(st) = weak.upgrade() else { break };
            if st.stop.load(Ordering::SeqCst) {
                break;
            }
            if let Err(e) = st.embed_pending() {
                tracing::warn!("kb embed: {e:#}");
            }
            {
                let mut w = st.wake.0.lock();
                if !*w {
                    st.wake.1.wait_for(&mut w, Duration::from_secs(30));
                }
                *w = false;
            }
                })?;
        Ok(())
    }

    fn watch(self: &Arc<Self>, id: i64, root: &Path) {
        let weak: Weak<KbStore> = Arc::downgrade(self);
        let r = new_debouncer(Duration::from_millis(500), move |res: DebounceEventResult| {
            let Some(st) = weak.upgrade() else { return };
            let Ok(events) = res else { return };
            let mut seen = HashSet::new();
            for ev in events {
                if seen.insert(ev.path.clone()) {
                    st.queue(Job::Path(id, ev.path));
                }
            }
        });
        match r {
            Ok(mut d) => {
                if let Err(e) = d.watcher().watch(root, RecursiveMode::Recursive) {
                    tracing::warn!("kb watch {}: {e}", root.display());
                }
                self.watchers.lock().insert(id, d);
            }
            Err(e) => tracing::warn!("kb watcher: {e}"),
        }
    }

    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .require_git(false)
            .add_custom_ignore_filename(".xodeignore")
            .filter_entry(|e| !e.file_name().to_str().map(|n| SKIP_DIRS.contains(&n)).unwrap_or(false))
            .build();
        for e in walker.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) && md::kind_of(&e.path().to_string_lossy()).is_some() {
                out.push(e.into_path());
            }
        }
        out
    }

    fn scan(&self, id: i64, force: bool) -> Result<()> {
        let Some((_, _, root)) = self.source(id) else { return Ok(()) };
        let key = self.src_key(id);
        self.emit(key.clone(), "scan", 0, 0);
        let known: HashMap<String, (i64, i64, String)> = self
            .conn
            .lock()
            .query_map("SELECT path, mtime, size, hash FROM notes WHERE source=?1", params![id], |r| {
                Ok((r.get::<String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
            })?
            .into_iter()
            .collect();
        let files = Self::walk(&root);
        let mut seen: HashSet<String> = HashSet::new();
        let mut todo: Vec<(String, PathBuf, (i64, i64))> = vec![];
        for p in files {
            let Some(rel) = rel_of(&root, &p) else { continue };
            let Some(meta) = file_meta(&p) else { continue };
            if meta.1 as u64 > MAX_FILE_BYTES {
                continue;
            }
            seen.insert(rel.clone());
            if !force {
                if let Some((m, s, _)) = known.get(&rel) {
                    if *m == meta.0 && *s == meta.1 {
                        continue;
                    }
                }
            }
            todo.push((rel, p, meta));
        }
        let total = todo.len() as u64;
        let chunk_tokens = self.cfg.read().chunk_tokens;
        let workers = std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).clamp(1, 6);
        let mut done = 0u64;
        for batch in todo.chunks(256) {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            let next = std::sync::atomic::AtomicUsize::new(0);
            let results: Mutex<Vec<(String, (i64, i64), String, Option<md::Parsed>)>> = Mutex::new(vec![]);
            std::thread::scope(|sc| {
                for _ in 0..workers {
                    sc.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((rel, abs, meta)) = batch.get(i) else { break };
                        let Ok(bytes) = std::fs::read(abs) else { continue };
                        if bytes[..bytes.len().min(8000)].contains(&0) {
                            continue;
                        }
                        let hash = xode_core::tool::hash_bytes(&bytes);
                        let same = !force && known.get(rel).map(|k| k.2 == hash).unwrap_or(false);
                        let parsed = (!same).then(|| {
                            let kind = md::kind_of(rel).unwrap_or(Kind::Text);
                            md::parse(rel, &String::from_utf8_lossy(&bytes), kind, chunk_tokens)
                        });
                        results.lock().push((rel.clone(), *meta, hash, parsed));
                    });
                }
            });
            let results = results.into_inner();
            let removed = self.write_notes(id, results)?;
            self.bits_remove(&removed);
            done += batch.len() as u64;
            self.emit(key.clone(), "index", done, total);
            self.bump();
        }
        let gone: Vec<String> = known.keys().filter(|k| !seen.contains(*k)).cloned().collect();
        for rel in gone {
            self.delete_note_path(id, &rel)?;
        }
        self.resolve_links()?;
        self.bump();
        self.emit(key, "idle", total, total);
        Ok(())
    }

    /// Reflect one changed path (file or folder, created / modified / deleted).
    fn sync_path(&self, id: i64, p: &Path) -> Result<()> {
        let Some((_, _, root)) = self.source(id) else { return Ok(()) };
        let Some(rel) = rel_of(&root, p) else { return Ok(()) };
        if skipped(&rel) {
            return Ok(());
        }
        if p.is_dir() {
            return self.scan(id, false);
        }
        if !p.exists() {
            self.delete_note_path(id, &rel)?;
            // Maybe a folder: drop notes below it.
            let below: Vec<String> = self.conn.lock().query_map(
                "SELECT path FROM notes WHERE source=?1 AND path LIKE ?2",
                params![id, format!("{rel}/%")],
                |r| r.get(0),
            )?;
            for r in below {
                self.delete_note_path(id, &r)?;
            }
            self.bump();
            return Ok(());
        }
        self.index_file(id, &root, &rel)?;
        self.resolve_links()?;
        self.bump();
        Ok(())
    }

    fn index_file(&self, id: i64, root: &Path, rel: &str) -> Result<Option<i64>> {
        let abs = root.join(rel);
        let Some(kind) = md::kind_of(rel) else { return Ok(None) };
        let Some(meta) = file_meta(&abs) else { return Ok(None) };
        if meta.1 as u64 > MAX_FILE_BYTES {
            return Ok(None);
        }
        let bytes = std::fs::read(&abs)?;
        let hash = xode_core::tool::hash_bytes(&bytes);
        let prev: Option<(i64, String)> = self
            .conn
            .lock()
            .query_row("SELECT id, hash FROM notes WHERE source=?1 AND path=?2", params![id, rel], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let parsed = match &prev {
            Some((_, h)) if *h == hash => None,
            _ => Some(md::parse(rel, &String::from_utf8_lossy(&bytes), kind, self.cfg.read().chunk_tokens)),
        };
        let removed = self.write_notes(id, vec![(rel.to_string(), meta, hash, parsed)])?;
        self.bits_remove(&removed);
        Ok(self
            .conn
            .lock()
            .scalar::<i64>("SELECT id FROM notes WHERE source=?1 AND path=?2", params![id, rel])?)
    }

    /// Upsert parsed notes (None = unchanged content, only refresh mtime/size). Returns removed chunk ids.
    fn write_notes(&self, source: i64, items: Vec<(String, (i64, i64), String, Option<md::Parsed>)>) -> Result<Vec<i64>> {
        let mut removed = vec![];
        self.conn.lock().transaction(|c| {
            for (rel, (mtime, size), hash, parsed) in items {
                let prev: Option<i64> = c.scalar("SELECT id FROM notes WHERE source=?1 AND path=?2", params![source, rel])?;
                let Some(p) = parsed else {
                    if let Some(nid) = prev {
                        c.execute("UPDATE notes SET mtime=?2, size=?3 WHERE id=?1", params![nid, mtime, size])?;
                    }
                    continue;
                };
                let outline = p
                    .headings
                    .iter()
                    .map(|h| format!("{}\t{}\t{}", h.level, h.line, h.text.replace(['\t', '\n'], " ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                let nid = match prev {
                    Some(nid) => {
                        removed.extend(c.query_map("SELECT id FROM chunks WHERE note=?1", params![nid], |r| r.get::<i64>(0))?);
                        c.execute("DELETE FROM chunks WHERE note=?1", params![nid])?;
                        c.execute("DELETE FROM links WHERE src=?1", params![nid])?;
                        c.execute(
                            "UPDATE notes SET title=?2, key=?3, tags=?4, summary=?5, tokens=?6, mtime=?7, size=?8, hash=?9, outline=?10 WHERE id=?1",
                            params![nid, p.title, md::link_key(&rel), tags_col(&p.tags), p.summary, p.tokens, mtime, size, hash, outline],
                        )?;
                        nid
                    }
                    None => {
                        c.execute(
                            "INSERT INTO notes(source, path, title, key, tags, summary, tokens, mtime, size, hash, outline)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                            params![source, rel, p.title, md::link_key(&rel), tags_col(&p.tags), p.summary, p.tokens, mtime, size, hash, outline],
                        )?;
                        c.last_insert_rowid()
                    }
                };
                for (i, ch) in p.chunks.iter().enumerate() {
                    c.execute(
                        "INSERT INTO chunks(note, source, ord, heading, line_start, line_end, tokens, text) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![nid, source, i as i64, ch.heading, ch.line_start, ch.line_end, ch.tokens, ch.text],
                    )?;
                }
                for l in &p.links {
                    c.execute("INSERT INTO links(src, target) VALUES (?1,?2)", params![nid, l])?;
                }
            }
            Ok(())
        })?;
        Ok(removed)
    }

    fn delete_note_path(&self, source: i64, rel: &str) -> Result<()> {
        let nid: Option<i64> = self.conn.lock().scalar("SELECT id FROM notes WHERE source=?1 AND path=?2", params![source, rel])?;
        if let Some(nid) = nid {
            self.delete_note_id(nid)?;
        }
        Ok(())
    }

    fn delete_note_id(&self, nid: i64) -> Result<()> {
        let ids: Vec<i64> = self.conn.lock().query_map("SELECT id FROM chunks WHERE note=?1", params![nid], |r| r.get(0))?;
        self.bits_remove(&ids);
        self.conn.lock().transaction(|c| {
            c.execute("DELETE FROM chunks WHERE note=?1", params![nid])?;
            c.execute("DELETE FROM links WHERE src=?1", params![nid])?;
            c.execute("UPDATE links SET dst=NULL WHERE dst=?1", params![nid])?;
            c.execute("DELETE FROM notes WHERE id=?1", params![nid])?;
            Ok(())
        })?;
        Ok(())
    }

    fn bits_remove(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let mut b = self.bits.write();
        ids.iter().for_each(|i| b.remove(*i));
    }

    /// Point unresolved links at notes whose file name or title matches.
    fn resolve_links(&self) -> Result<()> {
        let c = self.conn.lock();
        let pending: Vec<(i64, String)> =
            c.query_map("SELECT rowid, target FROM links WHERE dst IS NULL", (), |r| Ok((r.get(0)?, r.get(1)?)))?;
        if pending.is_empty() {
            return Ok(());
        }
        let mut keys: HashMap<String, i64> = HashMap::new();
        c.for_each("SELECT id, key, title FROM notes ORDER BY id DESC", (), |r| {
            if let (Ok(id), Ok(k), Ok(t)) = (r.get::<i64>(0), r.get::<String>(1), r.get::<String>(2)) {
                keys.insert(t.to_lowercase(), id);
                keys.insert(k, id);
            }
            true
        })?;
        c.transaction(|c| {
            for (rid, t) in pending {
                if let Some(d) = keys.get(&t) {
                    c.execute("UPDATE links SET dst=?2 WHERE rowid=?1", params![rid, *d])?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    // ------------------------------------------------------------------ embeddings

    fn load_bits(&self) {
        let c = self.conn.lock();
        let mut b = self.bits.write();
        b.clear();
        let _ = c.for_each("SELECT id, source, bits FROM chunks WHERE bits IS NOT NULL", (), |r| {
            if let (Ok(id), Ok(src), Ok(bytes)) = (r.get::<i64>(0), r.get::<i64>(1), r.get::<Vec<u8>>(2)) {
                b.insert(id, src, &bytes_to_bits(&bytes));
            }
            true
        });
    }

    fn meta(&self, k: &str) -> Option<String> {
        self.conn.lock().scalar("SELECT v FROM meta WHERE k=?1", params![k]).ok().flatten()
    }

    fn set_meta(&self, k: &str, v: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM meta WHERE k=?1", params![k])?;
        c.execute("INSERT INTO meta(k, v) VALUES (?1,?2)", params![k, v])?;
        Ok(())
    }

    /// Embed chunks that have no vector yet, in batches, until none are left.
    fn embed_pending(&self) -> Result<()> {
        if !self.owner || self.hub.wanted_id().is_none() || !self.cfg.read().enabled {
            return Ok(());
        }
        let pending: u64 = self.conn.lock().scalar("SELECT COUNT(*) FROM chunks WHERE emb IS NULL", ())?.unwrap_or(0);
        let model_changed = self.meta("embed_model") != self.hub.wanted_id();
        if pending == 0 && !model_changed {
            return Ok(());
        }
        let Some(e) = self.hub.get() else { return Ok(()) };
        if self.meta("embed_model").as_deref() != Some(e.id().as_str()) {
            self.conn.lock().execute("UPDATE chunks SET emb=NULL, bits=NULL", ())?;
            self.bits.write().clear();
            self.set_meta("embed_model", &e.id())?;
        }
        let total: u64 = self.conn.lock().scalar("SELECT COUNT(*) FROM chunks WHERE emb IS NULL", ())?.unwrap_or(0);
        let key = self.prefix.to_string();
        let mut done = 0u64;
        let mut cursor = 0i64;
        self.emit(key.clone(), "embed", 0, total);
        loop {
            if self.stop.load(Ordering::SeqCst) || self.hub.wanted_id().as_deref() != Some(e.id().as_str()) {
                break;
            }
            let rows: Vec<(i64, i64, String, String, String)> = self.conn.lock().query_map(
                "SELECT c.id, c.source, n.title, c.heading, c.text FROM chunks c JOIN notes n ON n.id = c.note
                 WHERE c.id > ?1 AND c.emb IS NULL ORDER BY c.id LIMIT 32",
                params![cursor],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
            if rows.is_empty() {
                break;
            }
            cursor = rows.last().map(|r| r.0).unwrap_or(cursor);
            let texts: Vec<String> = rows
                .iter()
                .map(|(_, _, title, head, text)| {
                    let t: String = text.chars().take(2000).collect();
                    if head.is_empty() {
                        format!("{title}\n{t}")
                    } else {
                        format!("{title} › {head}\n{t}")
                    }
                })
                .collect();
            let vecs = e.embed(&texts, false)?;
            {
                let c = self.conn.lock();
                c.transaction(|c| {
                    for ((id, _, _, _, _), v) in rows.iter().zip(&vecs) {
                        let bits = sign_bits(v);
                        c.execute(
                            "UPDATE chunks SET emb=vector32(?2), bits=?3 WHERE id=?1",
                            params![*id, xode_db::f32_blob(v), bits_to_bytes(&bits)],
                        )?;
                    }
                    Ok(())
                })?;
            }
            {
                let mut b = self.bits.write();
                for ((id, src, _, _, _), v) in rows.iter().zip(&vecs) {
                    b.insert(*id, *src, &sign_bits(v));
                }
            }
            done += rows.len() as u64;
            self.emit(key.clone(), "embed", done, total.max(done));
        }
        self.emit(key, "idle", done, total.max(done));
        Ok(())
    }

    // ------------------------------------------------------------------ notes

    pub fn note(&self, id: &str) -> Option<NoteView> {
        let nid = self.parse_id(id)?;
        let c = self.conn.lock();
        let (source, rel, title, tags, summary, tokens, outline): (i64, String, String, String, String, u32, String) = c
            .query_row(
                "SELECT source, path, title, tags, summary, tokens, outline FROM notes WHERE id=?1",
                params![nid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .ok()?;
        let links: Vec<NoteLink> = c
            .query_map(
                "SELECT l.dst, l.target, n.title FROM links l LEFT JOIN notes n ON n.id = l.dst WHERE l.src=?1",
                params![nid],
                |r| {
                    let dst: Option<i64> = r.get(0)?;
                    let title: Option<String> = r.get(2)?;
                    Ok(NoteLink { id: dst.map(|d| self.key(d)), title: title.unwrap_or(r.get(1)?) })
                },
            )
            .unwrap_or_default();
        let backlinks: Vec<NoteLink> = c
            .query_map(
                "SELECT n.id, n.title FROM links l JOIN notes n ON n.id = l.src WHERE l.dst=?1 GROUP BY n.id ORDER BY n.title LIMIT 200",
                params![nid],
                |r| Ok(NoteLink { id: Some(self.key(r.get(0)?)), title: r.get(1)? }),
            )
            .unwrap_or_default();
        drop(c);
        let (layer, source_name, root) = self.source(source)?;
        let abs = root.join(&rel);
        let text = std::fs::read(&abs).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|_| {
            // File gone or unreadable: rebuild from chunks.
            self.conn
                .lock()
                .query_map("SELECT text FROM chunks WHERE note=?1 ORDER BY ord", params![nid], |r| r.get::<String>(0))
                .unwrap_or_default()
                .join("\n\n")
        });
        let headings = outline
            .lines()
            .filter_map(|l| {
                let mut it = l.splitn(3, '\t');
                let lv: u8 = it.next()?.parse().ok()?;
                let line: u32 = it.next()?.parse().ok()?;
                Some((lv, it.next().unwrap_or("").to_string(), line))
            })
            .collect();
        Some(NoteView {
            id: self.key(nid),
            title,
            source: self.src_key(source),
            source_name,
            layer,
            rel,
            abs: abs.to_string_lossy().to_string(),
            tags: tags_vec(&tags),
            summary,
            tokens,
            headings,
            links,
            backlinks,
            text,
            writable: layer.writable(),
        })
    }

    /// Notes under a folder of a source and/or with a tag.
    pub fn list(&self, sources: &[i64], dir: &str, tag: &str, limit: usize) -> (Vec<(String, u64)>, Vec<NoteRow>) {
        if sources.is_empty() {
            return (vec![], vec![]);
        }
        let ids = sources.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        let dir = dir.trim().trim_matches('/');
        let like = if dir.is_empty() { "%".to_string() } else { format!("{}/%", dir.replace('%', "")) };
        let tagl = if tag.trim().is_empty() {
            "%".to_string()
        } else {
            format!("%,{},%", tag.trim().trim_start_matches('#').to_lowercase().replace('%', ""))
        };
        let c = self.conn.lock();
        let rows: Vec<(i64, i64, String, String, u32, String)> = c
            .query_map(
                &format!(
                    "SELECT id, source, path, title, tokens, tags FROM notes WHERE source IN ({ids}) AND path LIKE ?1 AND tags LIKE ?2 ORDER BY path"
                ),
                params![like, tagl],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap_or_default();
        drop(c);
        let prefix_len = if dir.is_empty() { 0 } else { dir.len() + 1 };
        let mut folders: HashMap<String, u64> = HashMap::new();
        let mut notes = vec![];
        for (id, src, path, title, tokens, tags) in rows {
            let rest = &path[prefix_len.min(path.len())..];
            if tag.trim().is_empty() {
                if let Some((f, _)) = rest.split_once('/') {
                    let key = if dir.is_empty() { f.to_string() } else { format!("{dir}/{f}") };
                    *folders.entry(key).or_default() += 1;
                    continue;
                }
            }
            if notes.len() < limit {
                notes.push(NoteRow { id: self.key(id), title, source: self.src_key(src), rel: path, tokens, tags: tags_vec(&tags) });
            }
        }
        let mut folders: Vec<(String, u64)> = folders.into_iter().collect();
        folders.sort();
        (folders, notes)
    }

    /// Most used tags among the given sources.
    pub fn top_tags(&self, sources: &[i64], n: usize) -> Vec<(String, u64)> {
        if sources.is_empty() {
            return vec![];
        }
        let ids = sources.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        let mut counts: HashMap<String, u64> = HashMap::new();
        let _ = self.conn.lock().for_each(&format!("SELECT tags FROM notes WHERE source IN ({ids}) AND tags != ''"), (), |r| {
            if let Ok(t) = r.get::<String>(0) {
                for x in tags_vec(&t) {
                    *counts.entry(x).or_default() += 1;
                }
            }
            true
        });
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// Link graph over the given sources, keeping the `limit` best-connected notes.
    pub fn graph(&self, sources: &[i64], limit: usize) -> Graph {
        if sources.is_empty() {
            return Graph::default();
        }
        let ids = sources.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        let c = self.conn.lock();
        let mut nodes: HashMap<i64, GraphNode> = HashMap::new();
        let _ = c.for_each(&format!("SELECT n.id, n.title, n.source, n.tokens, s.layer FROM notes n JOIN sources s ON s.id = n.source WHERE n.source IN ({ids})"), (), |r| {
            if let (Ok(id), Ok(title), Ok(src), Ok(tokens), Ok(layer)) =
                (r.get::<i64>(0), r.get::<String>(1), r.get::<i64>(2), r.get::<u32>(3), r.get::<String>(4))
            {
                nodes.insert(
                    id,
                    GraphNode {
                        id: self.key(id),
                        title,
                        layer: Layer::parse(&layer).unwrap_or(Layer::Library),
                        source: self.src_key(src),
                        tokens,
                        degree: 0,
                    },
                );
            }
            true
        });
        let mut edges: Vec<(i64, i64)> = vec![];
        let _ = c.for_each("SELECT src, dst FROM links WHERE dst IS NOT NULL", (), |r| {
            if let (Ok(a), Ok(b)) = (r.get::<i64>(0), r.get::<i64>(1)) {
                if a != b && nodes.contains_key(&a) && nodes.contains_key(&b) {
                    edges.push((a, b));
                }
            }
            true
        });
        drop(c);
        edges.sort();
        edges.dedup();
        for (a, b) in &edges {
            if let Some(n) = nodes.get_mut(a) {
                n.degree += 1;
            }
            if let Some(n) = nodes.get_mut(b) {
                n.degree += 1;
            }
        }
        let total = nodes.len();
        let mut keep: Vec<GraphNode> = nodes.into_values().collect();
        keep.sort_by(|a, b| b.degree.cmp(&a.degree).then(a.title.cmp(&b.title)));
        keep.truncate(limit);
        let kept: HashSet<String> = keep.iter().map(|n| n.id.clone()).collect();
        let edges = edges
            .into_iter()
            .map(|(a, b)| (self.key(a), self.key(b)))
            .filter(|(a, b)| kept.contains(a) && kept.contains(b))
            .collect();
        Graph { hidden: (total - keep.len()) as u64, nodes: keep, edges }
    }

    /// Source ids of this store allowed by a selection (optionally one layer).
    pub fn allowed(&self, sel: &crate::Sel, layer: Option<Layer>) -> Vec<i64> {
        self.sources()
            .into_iter()
            .filter(|s| sel.allows(s.layer, &s.key) && layer.map(|l| l == s.layer).unwrap_or(true))
            .map(|s| s.id)
            .collect()
    }

    /// Resolve a link target by file name or title (for cross-store links).
    pub fn find_by_key(&self, target: &str) -> Option<(String, String)> {
        let k = md::link_key(target);
        let c = self.conn.lock();
        c.query_row(
            "SELECT id, title FROM notes WHERE key=?1 OR lower(title)=?1 LIMIT 1",
            params![k],
            |r| Ok((self.key(r.get(0)?), r.get(1)?)),
        )
        .ok()
    }

    /// Titles (and ids) matching a prefix, for [[link]] autocomplete.
    pub fn titles(&self, q: &str, limit: usize) -> Vec<(String, String)> {
        let q = q.trim().to_lowercase().replace('%', "");
        self.conn
            .lock()
            .query_map(
                "SELECT id, title FROM notes WHERE lower(title) LIKE ?1 ORDER BY length(title) LIMIT ?2",
                params![format!("%{q}%"), limit as i64],
                |r| Ok((self.key(r.get(0)?), r.get(1)?)),
            )
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------ writing

    pub fn memory_layer(&self) -> Layer {
        self.memory_layer
    }

    fn memory_source(&self) -> Option<i64> {
        self.conn.lock().scalar("SELECT id FROM sources WHERE layer=?1", params![self.memory_layer.key()]).ok().flatten()
    }

    /// Create or replace a note in this store's memory folder. `id` (a memory note of this
    /// store) replaces that note's file. Returns the note id once indexed (None while another
    /// window owns the index and has not picked the file up yet).
    pub fn write_note(&self, id: Option<&str>, title: &str, body: &str, tags: &[String]) -> Result<(Option<String>, PathBuf)> {
        let title = title.trim();
        let path = match id {
            Some(id) => {
                let n = self.note(id).ok_or_else(|| anyhow!("no note {id}"))?;
                if !n.writable {
                    bail!("{id} is in {} (read-only); write a new memory note instead", n.layer.label());
                }
                PathBuf::from(n.abs)
            }
            None => {
                if title.is_empty() {
                    bail!("title required");
                }
                let base = md::slug(title);
                let mut p = self.memory_dir.join(format!("{base}.md"));
                let mut i = 2;
                while p.exists() {
                    p = self.memory_dir.join(format!("{base}-{i}.md"));
                    i += 1;
                }
                p
            }
        };
        let title = if title.is_empty() { id.and_then(|i| self.note(i)).map(|n| n.title).unwrap_or_default() } else { title.to_string() };
        std::fs::create_dir_all(&self.memory_dir)?;
        std::fs::write(&path, md::render_note(&title, tags, body))?;
        if !self.owner {
            return Ok((None, path));
        }
        let src = self.memory_source().ok_or_else(|| anyhow!("memory source missing"))?;
        let rel = rel_of(&self.memory_dir, &path).ok_or_else(|| anyhow!("bad memory path"))?;
        let nid = self.index_file(src, &self.memory_dir, &rel)?;
        self.resolve_links()?;
        self.bump();
        self.poke();
        Ok((nid.map(|n| self.key(n)), path))
    }

    /// Save raw markdown to a writable note (app editor).
    pub fn save_raw(&self, id: &str, text: &str) -> Result<()> {
        let n = self.note(id).ok_or_else(|| anyhow!("no note {id}"))?;
        if !n.writable {
            bail!("read-only note");
        }
        std::fs::write(&n.abs, text)?;
        if self.owner {
            let src = self.memory_source().ok_or_else(|| anyhow!("memory source missing"))?;
            self.index_file(src, &self.memory_dir, &n.rel)?;
            self.resolve_links()?;
            self.bump();
            self.poke();
        }
        Ok(())
    }

    pub fn delete_note(&self, id: &str) -> Result<()> {
        let n = self.note(id).ok_or_else(|| anyhow!("no note {id}"))?;
        if !n.writable {
            bail!("{id} is in {} (read-only)", n.layer.label());
        }
        let _ = std::fs::remove_file(&n.abs);
        if self.owner {
            if let Some(nid) = self.parse_id(id) {
                self.delete_note_id(nid)?;
            }
            self.bump();
        }
        Ok(())
    }
}

impl Drop for KbStore {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn init_db(c: &Connection) -> Result<()> {
    if c.user_version() != SCHEMA_VERSION {
        c.execute_batch(
            "DROP TABLE IF EXISTS chunks; DROP TABLE IF EXISTS notes; DROP TABLE IF EXISTS links;
             DROP TABLE IF EXISTS meta;",
        )?;
    }
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sources(id INTEGER PRIMARY KEY, layer TEXT NOT NULL, name TEXT NOT NULL,
             path TEXT NOT NULL, default_on INTEGER NOT NULL DEFAULT 1, added INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, source INTEGER NOT NULL, path TEXT NOT NULL,
             title TEXT NOT NULL, key TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '', summary TEXT NOT NULL DEFAULT '',
             tokens INTEGER NOT NULL DEFAULT 0, mtime INTEGER, size INTEGER, hash TEXT, outline TEXT NOT NULL DEFAULT '');
         CREATE INDEX IF NOT EXISTS notes_src_path ON notes(source, path);
         CREATE INDEX IF NOT EXISTS notes_key ON notes(key);
         CREATE TABLE IF NOT EXISTS chunks(id INTEGER PRIMARY KEY, note INTEGER NOT NULL, source INTEGER NOT NULL,
             ord INTEGER NOT NULL, heading TEXT NOT NULL DEFAULT '', line_start INTEGER, line_end INTEGER,
             tokens INTEGER, text TEXT NOT NULL, emb BLOB, bits BLOB);
         CREATE INDEX IF NOT EXISTS chunks_note ON chunks(note);
         CREATE TABLE IF NOT EXISTS links(src INTEGER NOT NULL, target TEXT NOT NULL, dst INTEGER);
         CREATE INDEX IF NOT EXISTS links_src ON links(src);
         CREATE INDEX IF NOT EXISTS links_dst ON links(dst);
         CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);",
    )?;
    // Full-text indexes (tantivy-backed index method).
    let has_fts: Option<String> = c.scalar("SELECT name FROM sqlite_master WHERE type='index' AND name='chunks_fts'", ())?;
    if has_fts.is_none() {
        c.execute_batch(
            "CREATE INDEX chunks_fts ON chunks USING fts(heading, text);
             CREATE INDEX notes_fts ON notes USING fts(title, tags);",
        )?;
    }
    c.set_user_version(SCHEMA_VERSION)?;
    Ok(())
}
