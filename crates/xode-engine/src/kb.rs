//! Knowledge base wiring: global + per-project stores, per-chat selection, app API.

use crate::engine::Engine;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use xode_core::store::Project;
use xode_core::AgentEvent;
use xode_kb::embed::EmbedStatus;
use xode_kb::store::{Graph, GraphNode};
use xode_kb::{Hit, Kb, KbStore, Layer, NoteView, SearchOpts, Sel, Source};

#[derive(Debug, Clone, Serialize)]
pub struct KbOverview {
    pub sources: Vec<Source>,
    pub embed: EmbedStatus,
    /// False when another Xode process owns the index (read-only here).
    pub owner: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct KbSearchReq {
    pub q: String,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub k: Option<usize>,
    /// Layer/source keys switched off (e.g. a chat's selection).
    #[serde(default)]
    pub off: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KbNoteRow {
    pub id: String,
    pub title: String,
    pub source: String,
    pub rel: String,
    pub tokens: u32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KbFolder {
    pub folders: Vec<(String, u64)>,
    pub notes: Vec<KbNoteRow>,
}

impl Engine {
    fn kb_progress_fn(&self) -> xode_kb::store::ProgressFn {
        let tx = self.events.clone();
        Arc::new(move |p: xode_kb::Progress| {
            let _ = tx.send(AgentEvent::KbProgress {
                session: String::new(),
                source: p.source,
                stage: p.stage,
                done: p.done,
                total: p.total,
            });
        })
    }

    pub(crate) fn kb_global(&self) -> Option<Arc<KbStore>> {
        let cfg = self.config_arc();
        if !cfg.knowledge.enabled {
            return None;
        }
        let mut g = self.kb_global.lock();
        if let Some(s) = &*g {
            return Some(s.clone());
        }
        let dir = xode_core::config::data_dir();
        match KbStore::open('g', &dir.join("kb.db"), &dir.join("memory"), Layer::Memory, self.embed_hub.clone(), &cfg.knowledge) {
            Ok(s) => {
                s.set_progress(self.kb_progress_fn());
                *g = Some(s.clone());
                Some(s)
            }
            Err(e) => {
                tracing::warn!("global kb open failed: {e:#}");
                None
            }
        }
    }

    pub(crate) fn kb_project(&self, project: &Project) -> Option<Arc<KbStore>> {
        let cfg = self.config_arc();
        if !cfg.knowledge.enabled {
            return None;
        }
        let mut m = self.kb_projects.lock();
        if let Some(s) = m.get(&project.id) {
            return Some(s.clone());
        }
        let x = Path::new(&project.root).join(".xode");
        match KbStore::open('p', &x.join("kb.db"), &x.join("memory"), Layer::ProjectMemory, self.embed_hub.clone(), &cfg.knowledge) {
            Ok(s) => {
                s.set_progress(self.kb_progress_fn());
                m.insert(project.id.clone(), s.clone());
                Some(s)
            }
            Err(e) => {
                tracing::warn!("project kb open failed: {e:#}");
                None
            }
        }
    }

    pub(crate) fn kb_for(&self, project: &Project) -> Kb {
        Kb { global: self.kb_global(), project: self.kb_project(project) }
    }

    pub(crate) fn kb_close_project(&self, project_id: &str) {
        if let Some(s) = self.kb_projects.lock().remove(project_id) {
            s.close();
        }
    }

    /// Push config changes to the embedder and open stores (closes them when disabled).
    pub(crate) fn kb_config_changed(&self) {
        let cfg = self.config_arc();
        self.embed_hub.set_config(&cfg);
        if !cfg.knowledge.enabled {
            if let Some(s) = self.kb_global.lock().take() {
                s.close();
            }
            for (_, s) in self.kb_projects.lock().drain() {
                s.close();
            }
            return;
        }
        for s in self.kb_global.lock().iter().chain(self.kb_projects.lock().values()) {
            s.set_config(&cfg.knowledge);
        }
    }

    fn kb_of(&self, project_id: &str) -> Result<Kb> {
        let p = self.project(project_id)?;
        let kb = self.kb_for(&p);
        if kb.global.is_none() && kb.project.is_none() {
            return Err(anyhow!("knowledge base is disabled"));
        }
        Ok(kb)
    }

    /// Selection for a new chat: sources with `default_on = false` start switched off.
    pub(crate) fn kb_default_off(&self, project: &Project) -> Vec<String> {
        let kb = self.kb_for(project);
        let stores: Vec<&Arc<KbStore>> = kb.stores().collect();
        xode_kb::default_off(&stores)
    }

    // ------------------------------------------------------------ public API

    pub fn kb_overview(&self, project_id: &str) -> Result<KbOverview> {
        let kb = self.kb_of(project_id)?;
        let owner = kb.stores().all(|s| s.is_owner());
        Ok(KbOverview { sources: kb.sources(), embed: self.embed_hub.status(), owner })
    }

    pub fn kb_add_source(&self, project_id: &str, layer: &str, path: &str, name: &str) -> Result<Source> {
        let kb = self.kb_of(project_id)?;
        let layer = Layer::parse(layer).ok_or_else(|| anyhow!("unknown layer `{layer}`"))?;
        let st = if layer.global() { kb.global } else { kb.project };
        st.ok_or_else(|| anyhow!("store unavailable"))?.add_source(layer, Path::new(path.trim()), name)
    }

    fn kb_source_store(&self, project_id: &str, key: &str) -> Result<(Arc<KbStore>, i64)> {
        let kb = self.kb_of(project_id)?;
        let st = kb.store_for(key).cloned().ok_or_else(|| anyhow!("unknown source {key}"))?;
        let id = key.split(':').nth(1).and_then(|x| x.parse().ok()).ok_or_else(|| anyhow!("bad source key {key}"))?;
        Ok((st, id))
    }

    pub fn kb_remove_source(&self, project_id: &str, key: &str) -> Result<()> {
        let (st, id) = self.kb_source_store(project_id, key)?;
        st.remove_source(id)?;
        // Drop the key from every chat selection of this project.
        for mut s in self.store.sessions(Some(project_id))? {
            if s.kb_off.iter().any(|k| k == key) {
                s.kb_off.retain(|k| k != key);
                let _ = self.store.save_session(&s);
            }
        }
        Ok(())
    }

    pub fn kb_update_source(&self, project_id: &str, key: &str, name: Option<String>, default_on: Option<bool>) -> Result<()> {
        let (st, id) = self.kb_source_store(project_id, key)?;
        st.update_source(id, name.as_deref(), default_on)
    }

    pub fn kb_reindex(&self, project_id: &str, key: &str) -> Result<()> {
        let (st, id) = self.kb_source_store(project_id, key)?;
        st.reindex(id, true);
        Ok(())
    }

    /// Hybrid search (runs off the async runtime: may embed the query).
    pub async fn kb_search(&self, project_id: &str, req: KbSearchReq) -> Result<Vec<Hit>> {
        let kb = self.kb_of(project_id)?;
        let k = self.config_arc().knowledge.k as usize;
        tokio::task::spawn_blocking(move || {
            let sel = Sel::new(&req.off);
            let opts = SearchOpts {
                k: req.k.unwrap_or(k.max(20)),
                layer: req.layer.as_deref().and_then(Layer::parse),
                tag: req.tag.filter(|t| !t.trim().is_empty()),
                text_only: false,
            };
            kb.search(&sel, &req.q, &opts)
        })
        .await
        .map_err(|e| anyhow!("{e}"))
    }

    pub fn kb_note(&self, project_id: &str, id: &str) -> Result<NoteView> {
        let kb = self.kb_of(project_id)?;
        kb.store_for(id).and_then(|s| s.note(id)).ok_or_else(|| anyhow!("note {id} not found"))
    }

    /// Notes and subfolders of a source folder (`dir` relative to the source), or all notes with a tag.
    pub fn kb_list(&self, project_id: &str, key: &str, dir: &str, tag: &str) -> Result<KbFolder> {
        let (st, id) = self.kb_source_store(project_id, key)?;
        let (folders, notes) = st.list(&[id], dir, tag, 2000);
        Ok(KbFolder {
            folders,
            notes: notes
                .into_iter()
                .map(|n| KbNoteRow { id: n.id, title: n.title, source: n.source, rel: n.rel, tokens: n.tokens, tags: n.tags })
                .collect(),
        })
    }

    pub fn kb_save_note(&self, project_id: &str, id: &str, text: &str) -> Result<()> {
        let kb = self.kb_of(project_id)?;
        kb.store_for(id).ok_or_else(|| anyhow!("note {id} not found"))?.save_raw(id, text)
    }

    /// New memory note; `global` picks Memory over Project memory. Returns its id.
    pub fn kb_create_note(&self, project_id: &str, global: bool, title: &str, body: &str) -> Result<String> {
        let kb = self.kb_of(project_id)?;
        let st = if global { kb.global } else { kb.project.or(kb.global) };
        let (id, path) = st.ok_or_else(|| anyhow!("store unavailable"))?.write_note(None, title, body, &[])?;
        id.ok_or_else(|| anyhow!("saved {} (indexing in another Xode window)", path.display()))
    }

    pub fn kb_delete_note(&self, project_id: &str, id: &str) -> Result<()> {
        let kb = self.kb_of(project_id)?;
        kb.store_for(id).ok_or_else(|| anyhow!("note {id} not found"))?.delete_note(id)
    }

    /// Link graph over both stores under a selection, with cross-store links resolved by name.
    pub fn kb_graph(&self, project_id: &str, off: Vec<String>, limit: usize) -> Result<Graph> {
        let kb = self.kb_of(project_id)?;
        let sel = Sel::new(&off);
        let limit = limit.clamp(50, 20_000);
        let mut out = Graph::default();
        for st in kb.stores() {
            let g = st.graph(&st.allowed(&sel, None), limit);
            out.hidden += g.hidden;
            out.nodes.extend(g.nodes);
            out.edges.extend(g.edges);
        }
        // Cross-store edges: unresolved [[links]] in one store naming a note in the other.
        if let (Some(g), Some(p)) = (&kb.global, &kb.project) {
            let ids: HashMap<String, ()> = out.nodes.iter().map(|n| (n.id.clone(), ())).collect();
            for (from, to) in [(p, g), (g, p)] {
                for (src, target) in from.dangling_links(&from.allowed(&sel, None), 5000) {
                    if let Some((dst, _)) = to.find_by_key(&target) {
                        if ids.contains_key(&src) && ids.contains_key(&dst) {
                            out.edges.push((src, dst));
                        }
                    }
                }
            }
        }
        if out.nodes.len() > limit {
            out.nodes.sort_by(|a: &GraphNode, b: &GraphNode| b.degree.cmp(&a.degree));
            out.hidden += (out.nodes.len() - limit) as u64;
            out.nodes.truncate(limit);
            let keep: std::collections::HashSet<&str> = out.nodes.iter().map(|n| n.id.as_str()).collect();
            let edges = std::mem::take(&mut out.edges);
            out.edges = edges.into_iter().filter(|(a, b)| keep.contains(a.as_str()) && keep.contains(b.as_str())).collect();
        }
        Ok(out)
    }

    /// Titles for [[link]] autocomplete.
    pub fn kb_titles(&self, project_id: &str, q: &str) -> Result<Vec<(String, String)>> {
        let kb = self.kb_of(project_id)?;
        Ok(kb.stores().flat_map(|s| s.titles(q, 20)).take(20).collect())
    }

    /// Retry loading the embedder (after a failed download / gateway change).
    pub fn kb_embed_retry(&self) {
        self.embed_hub.reset();
        let hub = self.embed_hub.clone();
        let stores: Vec<Arc<KbStore>> = self.kb_global.lock().iter().chain(self.kb_projects.lock().values()).cloned().collect();
        std::thread::spawn(move || {
            hub.get();
            for s in stores {
                s.set_config(&s.config());
            }
        });
    }

    /// Switch knowledge-base layers/sources for a chat.
    pub fn set_session_kb(&self, session_id: &str, off: Vec<String>) -> Result<()> {
        let mut s = self.session(session_id)?;
        s.kb_off = off.clone();
        self.store.save_session(&s)?;
        self.srt(session_id).state.lock().kb_off = off;
        // The system prompt mentions enabled layers: rebuild it next turn.
        self.srt(session_id).cache.lock().key.clear();
        Ok(())
    }
}
