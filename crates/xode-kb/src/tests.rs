use crate::embed::{EmbedHub, Embedder};
use crate::*;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};
use xode_core::config::Knowledge;

/// Bag-of-words hashing embedder: deterministic, no model download.
struct Stub;

impl Embedder for Stub {
    fn id(&self) -> String {
        "stub:v1".into()
    }
    fn dim(&self) -> usize {
        128
    }
    fn embed(&self, texts: &[String], _q: bool) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; 128];
                for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
                    let h = w.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
                    v[(h % 128) as usize] += 1.0;
                    v[((h >> 20) % 128) as usize] -= 0.5;
                }
                embed::normalize(v)
            })
            .collect())
    }
}

fn write(dir: &Path, rel: &str, s: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn wait(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    kb: Kb,
    lib: Source,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("vault");
    write(&lib, "Index.md", "# Index\n\nStart here. See [[Build Guide]] and [[Style]].\n");
    let mut long = String::from("---\ntags: [build, windows]\n---\n# Build Guide\n\nHow to build the product on every platform.\n\n");
    for (h, body) in [
        ("Windows", "Install the MSVC toolchain and run cargo build in a developer prompt. Use pwsh."),
        ("Linux", "Install gcc and pkg-config, then cargo build. Wayland needs extra libraries."),
        ("macOS", "Xcode command line tools are required. Codesign the bundle for notarization."),
    ] {
        long.push_str(&format!("## {h}\n\n"));
        for _ in 0..12 {
            long.push_str(body);
            long.push('\n');
        }
        long.push('\n');
    }
    write(&lib, "guides/build.md", &long);
    write(&lib, "guides/style.md", "# Style\n\nPrefer small functions. Tabs are forbidden. #conventions\n");
    write(&lib, "templates/pr.md", "# PR template\n\n## Summary\n\n## Test plan\n");
    write(&lib, "notes.txt", "Plain text note about deployment pipelines and release trains.");
    let hub = EmbedHub::fixed(Arc::new(Stub));
    let cfg = Knowledge { outline_tokens: 200, ..Default::default() };
    let g = KbStore::open('g', &tmp.path().join("g.db"), &tmp.path().join("gmem"), Layer::Memory, hub.clone(), &cfg).unwrap();
    let p = KbStore::open('p', &tmp.path().join("proj/.xode/kb.db"), &tmp.path().join("proj/.xode/memory"), Layer::ProjectMemory, hub, &cfg)
        .unwrap();
    let src = g.add_source(Layer::Library, &lib, "Vault").unwrap();
    let kb = Kb { global: Some(g), project: Some(p) };
    wait("index", || kb.sources().iter().find(|s| s.key == src.key).map(|s| s.notes == 5).unwrap_or(false));
    wait("embeddings", || kb.sources().iter().all(|s| s.embedded == s.chunks));
    let lib = kb.sources().into_iter().find(|s| s.key == src.key).unwrap();
    Fixture { _tmp: tmp, kb, lib }
}

fn tool(f: &Fixture, off: &[&str], args: serde_json::Value) -> Result<String, String> {
    let sel = Sel::new(&off.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let cfg = Knowledge { outline_tokens: 200, read_max_tokens: 300, ..Default::default() };
    super::tool::run_for_test(&f.kb, &sel, &cfg, &args)
}

#[test]
fn sources_notes_links_and_layers() {
    let f = fixture();
    assert_eq!(f.lib.layer, Layer::Library);
    let srcs = f.kb.sources();
    assert!(srcs.iter().any(|s| s.layer == Layer::Memory));
    assert!(srcs.iter().any(|s| s.layer == Layer::ProjectMemory));
    assert!(f.lib.chunks >= 5);
    // Links resolve by title and file name.
    let idx = f.kb.search(&Sel::default(), "start here", &SearchOpts { k: 3, ..Default::default() });
    let idx = f.kb.store_for(&idx[0].id).unwrap().note(&idx[0].id).unwrap();
    assert_eq!(idx.title, "Index");
    assert_eq!(idx.links.iter().filter(|l| l.id.is_some()).count(), 2, "{:?}", idx.links);
    let g = f.kb.global.as_ref().unwrap().graph(&[f.lib.id], 100);
    assert_eq!(g.nodes.len(), 5);
    assert_eq!(g.edges.len(), 2);
}

#[test]
fn search_read_outline_and_sections() {
    let f = fixture();
    let out = tool(&f, &[], json!({"action": "search", "q": "MSVC toolchain windows"})).unwrap();
    assert!(out.lines().next().unwrap().contains("Build Guide › Windows"), "{out}");
    let id = out.split_whitespace().next().unwrap().to_string();
    // Long note: outline first.
    let o = tool(&f, &[], json!({"action": "read", "id": id})).unwrap();
    assert!(o.contains("outline:") && o.contains("## Linux"), "{o}");
    assert!(xode_core::tokens::count(&o) < 200, "{o}");
    let s = tool(&f, &[], json!({"action": "read", "id": id, "section": "linux"})).unwrap();
    assert!(s.contains("pkg-config") && !s.contains("MSVC"), "{s}");
    // Capped read continues with lines=.
    let full = tool(&f, &[], json!({"action": "read", "id": id, "lines": "1-200"})).unwrap();
    assert!(full.contains("continue with lines="), "{full}");
    // Tag filter and list.
    let t = tool(&f, &[], json!({"action": "list", "tag": "windows"})).unwrap();
    assert!(t.contains("Build Guide"), "{t}");
    let l = tool(&f, &[], json!({"action": "list"})).unwrap();
    assert!(l.contains("Library: Vault") && l.contains("#build"), "{l}");
    let d = tool(&f, &[], json!({"action": "list", "path": "Vault/guides"})).unwrap();
    assert!(d.contains("Style") && d.contains("Build Guide"), "{d}");
    let links = tool(&f, &[], json!({"action": "links", "id": id})).unwrap();
    assert!(links.contains("← g") && links.contains("Index"), "{links}");
}

#[test]
fn vector_stage_finds_text_without_shared_fts_terms() {
    let f = fixture();
    // "notarization" only appears in the macOS section; the stub embedder shares hash buckets.
    let hits = f.kb.search(&Sel::default(), "notarization codesign", &SearchOpts { k: 3, ..Default::default() });
    assert!(hits[0].heading.ends_with("macOS"), "{hits:?}");
    assert!(f.kb.global.as_ref().unwrap().bits.read().len() > 0);
}

#[test]
fn selection_hides_sources_and_layers() {
    let f = fixture();
    let key = f.lib.key.clone();
    let out = tool(&f, &[&key], json!({"action": "search", "q": "MSVC"})).unwrap();
    assert!(out.starts_with("no matches"), "{out}");
    let out = tool(&f, &["library"], json!({"action": "search", "q": "MSVC"})).unwrap();
    assert!(out.starts_with("no matches"), "{out}");
    let hit = tool(&f, &[], json!({"action": "search", "q": "MSVC"})).unwrap();
    let id = hit.split_whitespace().next().unwrap();
    assert!(tool(&f, &[&key], json!({"action": "read", "id": id})).is_err());
    assert!(!f.kb.prompt_line(&Sel::new(&[key])).unwrap().contains("Library"));
}

#[test]
fn memory_write_update_delete_and_readonly_library() {
    let f = fixture();
    let out = tool(&f, &[], json!({"action": "write", "title": "Deploy notes", "body": "Deploys go through [[Build Guide]] first.", "tags": "deploy"})).unwrap();
    assert!(out.starts_with("saved p"), "{out}");
    let id = out.split_whitespace().nth(1).unwrap().to_string();
    let n = f.kb.store_for(&id).unwrap().note(&id).unwrap();
    assert_eq!(n.layer, Layer::ProjectMemory);
    assert_eq!(n.tags, vec!["deploy"]);
    // Cross-store link resolves through `links`.
    let l = tool(&f, &[], json!({"action": "links", "id": id})).unwrap();
    assert!(l.contains("→ g") && l.contains("Build Guide"), "{l}");
    let g = tool(&f, &[], json!({"action": "write", "title": "Global tip", "body": "Always run tests.", "global": true})).unwrap();
    assert!(g.starts_with("saved g"), "{g}");
    tool(&f, &[], json!({"action": "write", "id": id, "body": "Deploys changed: use the release train."})).unwrap();
    let s = tool(&f, &[], json!({"action": "search", "q": "release train deploys", "layer": "project_memory"})).unwrap();
    assert!(s.starts_with(&id), "{s}");
    // Library notes are read-only; switched-off memory refuses writes.
    let lib_hit = tool(&f, &[], json!({"action": "search", "q": "MSVC"})).unwrap();
    let lib_id = lib_hit.split_whitespace().next().unwrap();
    assert!(tool(&f, &[], json!({"action": "write", "id": lib_id, "body": "x"})).is_err());
    assert!(tool(&f, &[], json!({"action": "delete", "id": lib_id})).is_err());
    assert!(tool(&f, &["project_memory"], json!({"action": "write", "title": "t", "body": "b"})).is_err());
    tool(&f, &[], json!({"action": "delete", "id": id})).unwrap();
    assert!(f.kb.store_for(&id).unwrap().note(&id).is_none());
}

#[test]
fn watcher_picks_up_changes() {
    let f = fixture();
    write(Path::new(&f.lib.path), "guides/new.md", "# Fresh\n\nZebra crossing protocol.\n");
    let text = SearchOpts { text_only: true, ..Default::default() };
    wait("watcher", || !f.kb.search(&Sel::default(), "zebra", &text).is_empty());
    std::fs::remove_file(Path::new(&f.lib.path).join("guides/new.md")).unwrap();
    wait("watcher delete", || f.kb.search(&Sel::default(), "zebra", &text).is_empty());
}

#[test]
fn remove_source_drops_everything() {
    let f = fixture();
    let g = f.kb.global.as_ref().unwrap();
    g.remove_source(f.lib.id).unwrap();
    assert!(f.kb.search(&Sel::default(), "MSVC", &SearchOpts::default()).is_empty());
    assert_eq!(g.bits.read().len(), 0);
    assert!(g.remove_source(g.sources()[0].id).is_err(), "memory source is permanent");
}

/// Downloads the real built-in model: `cargo test -p xode-kb builtin_model -- --ignored --nocapture`.
#[test]
#[ignore]
fn builtin_model_embeds_multilingual() {
    let hub = EmbedHub::new(&xode_core::config::Config::default());
    let t = Instant::now();
    let e = hub.get().unwrap_or_else(|| panic!("{:?}", hub.status()));
    println!("loaded {} in {:?}", e.id(), t.elapsed());
    let docs: Vec<String> = ["Как собрать проект под Windows с MSVC", "Recipe for chocolate cake", "Build the project on Windows using MSVC"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let t = Instant::now();
    let d = e.embed(&docs, false).unwrap();
    let q = e.embed(&["windows build instructions".into()], true).unwrap().pop().unwrap();
    println!("embedded in {:?}", t.elapsed());
    let cos = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let s: Vec<f32> = d.iter().map(|v| cos(v, &q)).collect();
    println!("sims {s:?}");
    assert!(s[0] > s[1] && s[2] > s[1]);
}

/// Scale check: `XODE_KB_MB=100 cargo test --release -p xode-kb scale -- --ignored --nocapture`.
#[test]
#[ignore]
fn scale_ingest_and_search() {
    let mb: usize = std::env::var("XODE_KB_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("big");
    let words = ["alpha", "build", "cache", "deploy", "engine", "flag", "graph", "hash", "index", "join", "kernel", "latency", "memory", "node", "offset", "parser", "query", "render", "socket", "token", "update", "vector", "worker", "yaml", "zone"];
    let mut total = 0usize;
    let mut i = 0usize;
    let t = Instant::now();
    while total < mb << 20 {
        let mut s = format!("# Note {i}\n\nLinks to [[Note {}]] and [[Note {}]].\n\n", i / 2, i / 3);
        for sec in 0..6 {
            s.push_str(&format!("## Section {sec}\n\n"));
            for p in 0..6 {
                let line: Vec<&str> = (0..40).map(|k| words[(i * 7 + sec * 13 + p * 3 + k * 11) % words.len()]).collect();
                s.push_str(&line.join(" "));
                s.push_str("\n\n");
            }
        }
        total += s.len();
        write(&lib, &format!("d{}/n{i}.md", i % 200), &s);
        i += 1;
    }
    println!("generated {i} files, {} MB in {:?}", total >> 20, t.elapsed());
    let hub = EmbedHub::new(&xode_core::config::Config { knowledge: Knowledge { embedder: "off".into(), ..Default::default() }, ..Default::default() });
    let g = KbStore::open('g', &tmp.path().join("g.db"), &tmp.path().join("m"), Layer::Memory, hub, &Knowledge::default()).unwrap();
    let t = Instant::now();
    let src = g.add_source(Layer::Library, &lib, "big").unwrap();
    let kb = Kb { global: Some(g.clone()), project: None };
    let t2 = Instant::now();
    loop {
        let s = kb.sources().into_iter().find(|s| s.key == src.key).unwrap();
        if s.notes as usize == i {
            println!("indexed {} notes / {} chunks in {:?}", s.notes, s.chunks, t.elapsed());
            break;
        }
        assert!(t2.elapsed() < Duration::from_secs(1800));
        std::thread::sleep(Duration::from_millis(500));
    }
    let db = std::fs::metadata(tmp.path().join("g.db")).map(|m| m.len()).unwrap_or(0)
        + std::fs::metadata(tmp.path().join("g.db-wal")).map(|m| m.len()).unwrap_or(0);
    println!("db size {} MB", db >> 20);
    for q in ["kernel latency socket", "vector worker", "note 12345 section"] {
        let t = Instant::now();
        let h = kb.search(&Sel::default(), q, &SearchOpts { k: 8, text_only: true, ..Default::default() });
        println!("search `{q}`: {} hits in {:?}", h.len(), t.elapsed());
    }
    let t = Instant::now();
    let gr = g.graph(&[src.id], 4000);
    println!("graph {} nodes {} edges in {:?}", gr.nodes.len(), gr.edges.len(), t.elapsed());
}
