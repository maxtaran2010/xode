//! Embed all chunks in the real global KB db to completion (built-in model), with progress.
//! Owns the db (close the desktop app first). Usage: cargo run -p xode-kb --example embed_global
use std::io::Write;
use std::time::{Duration, Instant};
use xode_kb::{EmbedHub, KbStore, Layer};
fn main() {
    let dir = xode_core::config::data_dir();
    // Optional GPU path: --url <embeddings endpoint> [--model <name>] uses a gateway embedder.
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let url = arg("--url");
    let model = arg("--model").unwrap_or_else(|| "embed".into());
    let (cfg, full) = match &url {
        Some(u) => {
            let gw = xode_core::config::Gateway { id: "gpu".into(), name: "gpu".into(), url: u.clone(), enabled: true, ..Default::default() };
            let k = xode_core::config::Knowledge { embedder: "gateway".into(), gateway: "gpu".into(), gateway_model: model.clone(), ..Default::default() };
            (k.clone(), xode_core::config::Config { gateways: vec![gw], knowledge: k, ..Default::default() })
        }
        None => {
            let k = xode_core::config::Knowledge::default();
            (k.clone(), xode_core::config::Config { knowledge: k, ..Default::default() })
        }
    };
    println!("embedder: {}", url.as_deref().unwrap_or("builtin (CPU)"));
    let hub = EmbedHub::new(&full);
    let store = KbStore::open('g', &dir.join("kb.db"), &dir.join("memory"), Layer::Memory, hub, &cfg).expect("open");
    if !store.is_owner() {
        eprintln!("Xode holds the KB db; close it first.");
        std::process::exit(1);
    }
    let sum = || -> (u64, u64) { store.sources().iter().fold((0, 0), |(e, c), s| (e + s.embedded, c + s.chunks)) };
    let (_, mut total) = sum();
    println!("embedding {total} chunks (opening the store starts the embed worker)...");
    let start = Instant::now();
    let mut last = u64::MAX;
    let mut stable = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let (done, total) = sum();
        if done != last {
            last = done;
            stable_reset(&mut stable);
            let pct = if total > 0 { done * 100 / total } else { 100 };
            print!("\r  embedded {done} / {total}  ({pct}%)  {:?}        ", start.elapsed());
            std::io::stdout().flush().ok();
        }
        if total > 0 && done >= total {
            println!("\ndone: {done}/{total} embedded in {:?}", start.elapsed());
            break;
        }
        if stable.elapsed() > Duration::from_secs(300) {
            println!("\nstalled at {done}/{total} (no progress 5 min)");
            break;
        }
    }
}
fn stable_reset(s: &mut Instant) { *s = Instant::now(); }
