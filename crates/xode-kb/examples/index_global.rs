//! Open Xode's real global KB db (single writer) and drive indexing of its Library
//! sources to completion, then run a sample search. Close the desktop app first.
//! Usage: cargo run -p xode-kb --example index_global -- [folder]
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use xode_kb::{EmbedHub, Kb, KbStore, Layer, SearchOpts, Sel};

fn main() {
    let dir = xode_core::config::data_dir();
    let cfg = xode_core::config::Knowledge { embedder: "off".into(), ..Default::default() };
    let hub = EmbedHub::new(&xode_core::config::Config { knowledge: cfg.clone(), ..Default::default() });
    let store = KbStore::open('g', &dir.join("kb.db"), &dir.join("memory"), Layer::Memory, hub, &cfg).expect("open kb.db");
    if !store.is_owner() {
        eprintln!("Xode is running and holds the KB database. Close it, then re-run.");
        std::process::exit(1);
    }
    if let Some(folder) = std::env::args().nth(1) {
        if !store.sources().iter().any(|s| Path::new(&s.path) == Path::new(&folder)) {
            let _ = store.add_source(Layer::Library, Path::new(&folder), "security");
        }
    }
    let libs: Vec<_> = store.sources().into_iter().filter(|s| s.layer == Layer::Library).collect();
    println!("indexing {} Library source(s)...", libs.len());
    // Opening the store already queued a scan per source. Poll until note counts stop growing.
    let start = Instant::now();
    let mut last = 0u64;
    let mut stable = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(3));
        let now: Vec<_> = store.sources().into_iter().filter(|s| s.layer == Layer::Library).collect();
        let notes: u64 = now.iter().map(|s| s.notes).sum();
        let chunks: u64 = now.iter().map(|s| s.chunks).sum();
        if notes != last {
            last = notes;
            stable = Instant::now();
            print!("\r  {notes} notes / {chunks} chunks   ({:?})        ", start.elapsed());
            std::io::stdout().flush().ok();
        } else if last > 0 && stable.elapsed() > Duration::from_secs(15) {
            println!("\ndone: {last} notes indexed in {:?}", start.elapsed());
            break;
        }
        if start.elapsed() > Duration::from_secs(5400) {
            println!("\ntimeout at {last} notes");
            break;
        }
    }
    let kb = Kb { global: Some(store.clone()), project: None };
    for q in ["authentication bypass rce", "sql injection wordpress", "path traversal apache"] {
        let h = kb.search(&Sel::default(), q, &SearchOpts { k: 3, text_only: true, ..Default::default() });
        println!("  \"{q}\": {}", h.iter().map(|x| x.title.clone()).collect::<Vec<_>>().join(", "));
    }
}
