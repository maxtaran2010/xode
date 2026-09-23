//! Embed all chunks in the real global KB db to completion (built-in model), with progress.
//! Owns the db (close the desktop app first). Usage: cargo run -p xode-kb --example embed_global
use std::io::Write;
use std::time::{Duration, Instant};
use xode_kb::{EmbedHub, KbStore, Layer};
fn main() {
    let dir = xode_core::config::data_dir();
    let cfg = xode_core::config::Knowledge::default(); // built-in embedder
    let hub = EmbedHub::new(&xode_core::config::Config { knowledge: cfg.clone(), ..Default::default() });
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
