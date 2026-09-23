//! Register a folder as a global Library source in Xode's real KB db, then exit.
//! The desktop app indexes it on next launch. Close the app before running (single writer).
//! Usage: cargo run -p xode-kb --example add -- <folder> [name]
use std::path::Path;
use xode_kb::{EmbedHub, KbStore, Layer};

fn main() {
    let folder = std::env::args().nth(1).expect("folder arg");
    let name = std::env::args().nth(2).unwrap_or_else(|| "security".into());
    let dir = xode_core::config::data_dir(); // same paths as the engine's global store
    let cfg = xode_core::config::Knowledge { embedder: "off".into(), ..Default::default() };
    let hub = EmbedHub::new(&xode_core::config::Config { knowledge: cfg.clone(), ..Default::default() });
    let store = KbStore::open('g', &dir.join("kb.db"), &dir.join("memory"), Layer::Memory, hub, &cfg)
        .expect("open global kb.db");
    if !store.is_owner() {
        eprintln!("Xode is running and holds the KB database. Close the desktop app, then re-run.");
        std::process::exit(1);
    }
    match store.add_source(Layer::Library, Path::new(&folder), &name) {
        Ok(s) => println!("added source {} ({}) -> {}\nLaunch Xode; it will index it.", s.key, s.name, s.path),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("already added") {
                println!("source already registered: {folder}");
            } else {
                eprintln!("add failed: {msg}");
                std::process::exit(1);
            }
        }
    }
}
