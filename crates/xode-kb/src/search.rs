//! Hybrid search: FTS (tantivy via Turso) + vectors (1-bit Hamming prefilter in memory, exact
//! cosine rescoring in Turso), fused by reciprocal rank, grouped per note.

use crate::embed::sign_bits;
use crate::store::KbStore;
use crate::{Kb, Layer, Sel};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use xode_db::params;

const FTS_N: usize = 80;
const VEC_PRE: usize = 300;
const VEC_N: usize = 60;
const RRF_K: f64 = 60.0;
/// Cosine-distance band above the best vector hit that still counts as a match.
const VEC_BAND: f64 = 0.06;

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub heading: String,
    pub line_start: u32,
    pub line_end: u32,
    /// Tokens of this chunk / of the whole note.
    pub tokens: u32,
    pub note_tokens: u32,
    pub snippet: String,
    pub score: f64,
    pub layer: Layer,
    pub source: String,
    pub rel: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    pub k: usize,
    pub layer: Option<Layer>,
    pub tag: Option<String>,
    /// Skip the vector stage (no embedding of the query).
    pub text_only: bool,
}

static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{L}\p{N}_]{2,}").unwrap());

fn terms(q: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    WORD.find_iter(q)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| seen.insert(w.clone()))
        .take(16)
        .collect()
}

/// Largest char boundary <= i.
fn fb(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Best ~200 chars of `text` around the densest occurrence of query terms.
fn snippet(text: &str, terms: &[String]) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let mut best = 0usize;
    let mut best_score = 0usize;
    for t in terms {
        for (i, _) in lower.match_indices(t.as_str()).take(20) {
            let end = (i + 200).min(lower.len());
            let win = &lower[i..fb(&lower, end)];
            let s = terms.iter().filter(|x| win.contains(x.as_str())).count();
            if s > best_score {
                best_score = s;
                best = i;
            }
        }
    }
    let start = fb(&flat, best.saturating_sub(40));
    let start = if start == 0 { 0 } else { flat[start..].find(' ').map(|p| start + p + 1).unwrap_or(start) };
    let end = fb(&flat, (start + 220).min(flat.len()));
    let mut s = flat[start..end].to_string();
    if start > 0 {
        s.insert(0, '…');
    }
    if end < flat.len() {
        s.push('…');
    }
    s
}

struct Ranked {
    store: Arc<KbStore>,
    chunk: i64,
    score: f64,
}

impl Kb {
    /// Hybrid search over every store under the selection. Blocking (may embed the query).
    pub fn search(&self, sel: &Sel, q: &str, opts: &SearchOpts) -> Vec<Hit> {
        search(self, sel, q, opts)
    }
}

/// Search every store of `kb` under the selection.
pub fn search(kb: &Kb, sel: &Sel, q: &str, opts: &SearchOpts) -> Vec<Hit> {
    let k = if opts.k == 0 { 8 } else { opts.k.min(30) };
    let ts = terms(q);
    // Query vector, if an embedder is loaded (or loadable) and any store has vectors.
    let want_vec = !opts.text_only && kb.stores().any(|s| s.bits.read().len() > 0);
    // Never block a search on loading/downloading a model: load it in the background and
    // search text-only until it is ready.
    let qv = want_vec
        .then(|| kb.stores().next().map(|s| s.hub.clone()))
        .flatten()
        .and_then(|h| {
            h.loaded().or_else(|| {
                if h.status().state != "loading" {
                    std::thread::spawn(move || {
                        h.get();
                    });
                }
                None
            })
        })
        .and_then(|e| e.embed(&[q.to_string()], true).ok())
        .and_then(|mut v| v.pop());

    let mut ranked: Vec<Ranked> = vec![];
    for st in kb.stores() {
        let allowed = st.allowed(sel, opts.layer);
        if allowed.is_empty() {
            continue;
        }
        let ids = allowed.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
        let tag_filter = opts
            .tag
            .as_deref()
            .map(|t| t.trim().trim_start_matches('#').to_lowercase().replace(['\'', '%'], ""))
            .filter(|t| !t.is_empty())
            .map(|t| format!(" AND note IN (SELECT id FROM notes WHERE tags LIKE '%,{t},%')"));
        let tf = tag_filter.clone().unwrap_or_default();
        let mut lists: Vec<(f64, Vec<i64>)> = vec![];
        if !ts.is_empty() {
            let fq = ts.join(" ");
            let c = st.conn.lock();
            let fts: Vec<i64> = c
                .query_map(
                    &format!(
                        "SELECT id FROM chunks WHERE fts_match(heading, text, ?1) AND source IN ({ids}){tf}
                         ORDER BY fts_score(heading, text, ?1) DESC LIMIT {FTS_N}"
                    ),
                    params![fq],
                    |r| r.get(0),
                )
                .unwrap_or_else(|e| {
                    tracing::debug!("kb fts: {e}");
                    vec![]
                });
            lists.push((1.0, fts));
            // Title / tag hits map to the note's first chunk.
            let titles: Vec<i64> = c
                .query_map(
                    &format!(
                        "SELECT (SELECT id FROM chunks WHERE chunks.note = notes.id ORDER BY ord LIMIT 1) FROM notes
                         WHERE fts_match(title, tags, ?1) AND source IN ({ids})
                         ORDER BY fts_score(title, tags, ?1) DESC LIMIT 20"
                    ),
                    params![fq],
                    |r| r.get::<Option<i64>>(0),
                )
                .unwrap_or_default()
                .into_iter()
                .flatten()
                .filter(|id| tag_filter.is_none() || chunk_has_tag(st, *id, opts.tag.as_deref().unwrap_or("")))
                .collect();
            // Title / tag matches weigh less than body text and vectors.
            lists.push((0.6, titles));
        }
        if let Some(qv) = &qv {
            let set: HashSet<i64> = allowed.iter().copied().collect();
            let pre = st.bits.read().top(&sign_bits(qv), &set, VEC_PRE);
            if !pre.is_empty() {
                let idl = pre.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
                let v: Vec<(i64, f64)> = st
                    .conn
                    .lock()
                    .query_map(
                        &format!(
                            "SELECT id, vector_distance_cos(emb, vector32(?1)) d FROM chunks WHERE id IN ({idl}) AND emb IS NOT NULL{tf}
                             ORDER BY d LIMIT {VEC_N}"
                        ),
                        params![xode_db::f32_blob(qv)],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or_default();
                // Nearest neighbours always exist: keep only those close to the best one.
                let best = v.first().map(|x| x.1).unwrap_or(0.0);
                lists.push((1.0, v.into_iter().filter(|(_, d)| *d <= best + VEC_BAND).map(|(id, _)| id).collect()));
            }
        }
        let mut fused: HashMap<i64, f64> = HashMap::new();
        for (w, l) in &lists {
            for (r, id) in l.iter().enumerate() {
                *fused.entry(*id).or_default() += w / (RRF_K + r as f64 + 1.0);
            }
        }
        ranked.extend(fused.into_iter().map(|(chunk, score)| Ranked { store: st.clone(), chunk, score }));
    }
    ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut hits: Vec<Hit> = vec![];
    let mut per_note: HashMap<String, usize> = HashMap::new();
    for r in ranked {
        if hits.len() >= k {
            break;
        }
        let Some(h) = hit(&r.store, r.chunk, r.score, &ts) else { continue };
        let n = per_note.entry(h.id.clone()).or_default();
        if *n >= 2 {
            continue;
        }
        *n += 1;
        hits.push(h);
    }
    hits
}

fn chunk_has_tag(st: &KbStore, chunk: i64, tag: &str) -> bool {
    let t = tag.trim().trim_start_matches('#').to_lowercase();
    st.conn
        .lock()
        .scalar::<i64>(
            "SELECT 1 FROM chunks c JOIN notes n ON n.id = c.note WHERE c.id=?1 AND n.tags LIKE ?2",
            params![chunk, format!("%,{t},%")],
        )
        .ok()
        .flatten()
        .is_some()
}

fn hit(st: &Arc<KbStore>, chunk: i64, score: f64, ts: &[String]) -> Option<Hit> {
    let c = st.conn.lock();
    let (note, heading, a, b, tokens, text, title, nt, src, rel, layer): (
        i64,
        String,
        u32,
        u32,
        u32,
        String,
        String,
        u32,
        i64,
        String,
        String,
    ) = c
        .query_row(
            "SELECT c.note, c.heading, c.line_start, c.line_end, c.tokens, c.text, n.title, n.tokens, n.source, n.path, s.layer
             FROM chunks c JOIN notes n ON n.id = c.note JOIN sources s ON s.id = n.source WHERE c.id=?1",
            params![chunk],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ))
            },
        )
        .ok()?;
    drop(c);
    Some(Hit {
        id: st.key(note),
        title,
        heading,
        line_start: a,
        line_end: b,
        tokens,
        note_tokens: nt,
        snippet: snippet(&text, ts),
        score,
        layer: Layer::parse(&layer).unwrap_or(Layer::Library),
        source: st.src_key(src),
        rel,
    })
}
