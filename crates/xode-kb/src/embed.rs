//! Text embeddings: built-in ONNX models (fastembed, CPU) or a gateway's `/v1/embeddings`.
//! Vectors are L2-normalized; a sign-bit code of each vector backs the fast first search stage.

use anyhow::{anyhow, Context, Result};
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::sync::Arc;
use xode_core::config::{Config, Knowledge};

pub trait Embedder: Send + Sync {
    /// Identity stored with the vectors; changing it re-embeds everything.
    fn id(&self) -> String;
    fn dim(&self) -> usize;
    /// `query`: embedding a search query (vs. a passage), for models with asymmetric prefixes.
    fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>>;
}

/// Built-in model names shown in settings → fastembed model.
pub const BUILTIN_MODELS: &[(&str, &str)] = &[
    ("multilingual-e5-small", "Multilingual E5 small · 384"),
    ("multilingual-e5-base", "Multilingual E5 base · 768"),
    ("paraphrase-multilingual-minilm", "Paraphrase multilingual MiniLM (quantized) · 384"),
    ("bge-small-en", "BGE small EN (quantized) · 384"),
];

#[cfg(feature = "builtin-embed")]
fn fastembed_model(name: &str) -> Option<(fastembed::EmbeddingModel, usize, bool)> {
    use fastembed::EmbeddingModel as M;
    Some(match name {
        "multilingual-e5-small" => (M::MultilingualE5Small, 384, true),
        "multilingual-e5-base" => (M::MultilingualE5Base, 768, true),
        "paraphrase-multilingual-minilm" => (M::ParaphraseMLMiniLML12V2Q, 384, false),
        "bge-small-en" => (M::BGESmallENV15Q, 384, false),
        _ => return None,
    })
}

#[cfg(feature = "builtin-embed")]
struct Builtin {
    name: String,
    dim: usize,
    e5: bool,
    model: Mutex<fastembed::TextEmbedding>,
}

#[cfg(feature = "builtin-embed")]
impl Embedder for Builtin {
    fn id(&self) -> String {
        format!("builtin:{}", self.name)
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = if self.e5 {
            let p = if query { "query: " } else { "passage: " };
            texts.iter().map(|t| format!("{p}{t}")).collect()
        } else {
            texts.to_vec()
        };
        let out = self.model.lock().embed(&inputs, Some(32)).map_err(|e| anyhow!("{e}"))?;
        Ok(out.into_iter().map(normalize).collect())
    }
}

struct Gateway {
    url: String,
    key: String,
    model: String,
    dim: usize,
    http: reqwest::blocking::Client,
}

impl Gateway {
    fn call(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut req = self
            .http
            .post(format!("{}/v1/embeddings", self.url.trim_end_matches('/').trim_end_matches("/v1")))
            .json(&serde_json::json!({ "model": self.model, "input": texts }));
        if !self.key.is_empty() {
            req = req.bearer_auth(&self.key);
        }
        let v: serde_json::Value = req.send()?.error_for_status()?.json()?;
        let data = v["data"].as_array().ok_or_else(|| anyhow!("no data in embeddings response"))?;
        let mut out = vec![vec![]; texts.len()];
        for (i, d) in data.iter().enumerate() {
            let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
            let e: Vec<f32> = d["embedding"]
                .as_array()
                .ok_or_else(|| anyhow!("bad embedding"))?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            if idx < out.len() {
                out[idx] = normalize(e);
            }
        }
        if out.iter().any(|e| e.is_empty()) {
            return Err(anyhow!("embeddings response missing items"));
        }
        Ok(out)
    }
}

impl Embedder for Gateway {
    fn id(&self) -> String {
        format!("gateway:{}", self.model)
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn embed(&self, texts: &[String], _query: bool) -> Result<Vec<Vec<f32>>> {
        self.call(texts)
    }
}

pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// Sign bits packed little-endian into u64 words.
pub fn sign_bits(v: &[f32]) -> Vec<u64> {
    let mut out = vec![0u64; v.len().div_ceil(64)];
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            out[i / 64] |= 1 << (i % 64);
        }
    }
    out
}

pub fn bits_to_bytes(b: &[u64]) -> Vec<u8> {
    b.iter().flat_map(|w| w.to_le_bytes()).collect()
}

pub fn bytes_to_bits(b: &[u8]) -> Vec<u64> {
    b.chunks(8)
        .map(|c| {
            let mut a = [0u8; 8];
            a[..c.len()].copy_from_slice(c);
            u64::from_le_bytes(a)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EmbedStatus {
    /// off | loading | ready | error
    pub state: String,
    pub model: String,
    pub error: String,
}

/// Shared, lazily-built embedder following the current config.
pub struct EmbedHub {
    cfg: RwLock<(Knowledge, Vec<xode_core::config::Gateway>)>,
    cur: Mutex<Option<(String, Arc<dyn Embedder>)>>,
    status: RwLock<EmbedStatus>,
    cache_dir: PathBuf,
    /// Serializes model construction (downloads).
    building: Mutex<()>,
}

impl EmbedHub {
    pub fn new(cfg: &Config) -> Arc<Self> {
        Arc::new(Self {
            cfg: RwLock::new((cfg.knowledge.clone(), cfg.gateways.clone())),
            cur: Mutex::new(None),
            status: RwLock::new(EmbedStatus { state: "off".into(), model: String::new(), error: String::new() }),
            cache_dir: xode_core::config::data_dir().join("models"),
            building: Mutex::new(()),
        })
    }

    /// Hub with a fixed embedder (tests).
    pub fn fixed(e: Arc<dyn Embedder>) -> Arc<Self> {
        let h = Self::new(&Config::default());
        *h.cur.lock() = Some((e.id(), e.clone()));
        h.cfg.write().0.embedder = "fixed".into();
        *h.status.write() = EmbedStatus { state: "ready".into(), model: e.id(), error: String::new() };
        h
    }

    pub fn set_config(&self, cfg: &Config) {
        let mut c = self.cfg.write();
        if c.0.embedder == "fixed" {
            return;
        }
        *c = (cfg.knowledge.clone(), cfg.gateways.clone());
    }

    pub fn status(&self) -> EmbedStatus {
        self.status.read().clone()
    }

    /// Identity the current config would produce (without building it).
    pub fn wanted_id(&self) -> Option<String> {
        let (k, _) = &*self.cfg.read();
        match k.embedder.as_str() {
            "builtin" => Some(format!("builtin:{}", k.builtin_model)),
            "gateway" if !k.gateway_model.is_empty() => Some(format!("gateway:{}", k.gateway_model)),
            "fixed" => self.cur.lock().as_ref().map(|c| c.0.clone()),
            _ => None,
        }
    }

    /// The embedder for the current config, building it (and downloading a model) if needed.
    /// Blocking; call off the async runtime.
    pub fn get(&self) -> Option<Arc<dyn Embedder>> {
        let want = self.wanted_id()?;
        if let Some((id, e)) = &*self.cur.lock() {
            if *id == want {
                return Some(e.clone());
            }
        }
        let _g = self.building.lock();
        if let Some((id, e)) = &*self.cur.lock() {
            if *id == want {
                return Some(e.clone());
            }
        }
        if self.status.read().state == "error" && self.status.read().model == want {
            return None;
        }
        *self.status.write() = EmbedStatus { state: "loading".into(), model: want.clone(), error: String::new() };
        let (k, gws) = self.cfg.read().clone();
        let built: Result<Arc<dyn Embedder>> = match k.embedder.as_str() {
            "builtin" => self.build_builtin(&k.builtin_model),
            "gateway" => build_gateway(&k, &gws),
            _ => Err(anyhow!("embeddings off")),
        };
        match built {
            Ok(e) => {
                *self.status.write() = EmbedStatus { state: "ready".into(), model: e.id(), error: String::new() };
                *self.cur.lock() = Some((want, e.clone()));
                Some(e)
            }
            Err(err) => {
                tracing::warn!("embedder unavailable: {err:#}");
                *self.status.write() = EmbedStatus { state: "error".into(), model: want, error: format!("{err:#}") };
                None
            }
        }
    }

    /// Forget a failed build so the next `get` retries (settings "Test").
    pub fn reset(&self) {
        *self.status.write() = EmbedStatus { state: "off".into(), model: String::new(), error: String::new() };
        if self.cfg.read().0.embedder != "fixed" {
            *self.cur.lock() = None;
        }
    }

    /// Currently loaded embedder, without building one.
    pub fn loaded(&self) -> Option<Arc<dyn Embedder>> {
        let want = self.wanted_id()?;
        self.cur.lock().as_ref().filter(|c| c.0 == want).map(|c| c.1.clone())
    }

    #[cfg(feature = "builtin-embed")]
    fn build_builtin(&self, name: &str) -> Result<Arc<dyn Embedder>> {
        let (m, dim, e5) = fastembed_model(name).ok_or_else(|| anyhow!("unknown built-in model `{name}`"))?;
        std::fs::create_dir_all(&self.cache_dir).ok();
        let opts = fastembed::TextInitOptions::new(m)
            .with_cache_dir(self.cache_dir.clone())
            .with_show_download_progress(false)
            .with_max_length(512);
        let model = fastembed::TextEmbedding::try_new(opts).map_err(|e| anyhow!("{e}")).context("load embedding model")?;
        Ok(Arc::new(Builtin { name: name.to_string(), dim, e5, model: Mutex::new(model) }))
    }

    #[cfg(not(feature = "builtin-embed"))]
    fn build_builtin(&self, _name: &str) -> Result<Arc<dyn Embedder>> {
        Err(anyhow!("this build has no built-in embeddings; set the embedder to a gateway in Settings → Knowledge"))
    }
}

fn build_gateway(k: &Knowledge, gws: &[xode_core::config::Gateway]) -> Result<Arc<dyn Embedder>> {
    let gw = gws
        .iter()
        .find(|g| g.id == k.gateway || g.name == k.gateway)
        .or_else(|| gws.iter().find(|g| g.enabled))
        .ok_or_else(|| anyhow!("no gateway for embeddings"))?;
    let mut g = Gateway {
        url: gw.url.clone(),
        key: gw.api_key.clone(),
        model: k.gateway_model.clone(),
        dim: 0,
        http: reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(120)).build()?,
    };
    let probe = g.call(&["dimension probe".to_string()]).context("gateway /v1/embeddings")?;
    g.dim = probe[0].len();
    Ok(Arc::new(g))
}
